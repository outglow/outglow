use std::path::{Path, PathBuf};

use anyhow::{bail, Result};
use ra_ap_rustc_lexer::{tokenize, DocStyle, FrontmatterAllowed, TokenKind};

#[derive(clap::Args)]
#[command(about = "Rust sources carry no comments: list them, or remove them with --fix")]
pub struct Args {
    #[arg(long, help = "Remove the comments (in files that use clap, `///` becomes `#[doc = \"...\"]`, which clap reads as help)")]
    fix: bool,
}

#[derive(Debug, PartialEq, Eq)]
pub struct Comment {
    pub line: usize,
    pub text: String,
}

pub fn run(a: &Args) -> Result<()> {
    let root = crate::root();
    let mut found = 0;
    let mut fixed = 0;
    for path in rust_files(&root) {
        let src = std::fs::read_to_string(&path)?;
        let comments = find(&src);
        if comments.is_empty() {
            continue;
        }
        if a.fix {
            std::fs::write(&path, strip(&src, uses_clap(&src)))?;
            fixed += 1;
            continue;
        }
        let rel = path.strip_prefix(&root).unwrap_or(&path).display().to_string();
        for c in &comments {
            println!("{rel}:{}: {}", c.line, c.text.lines().next().unwrap_or(""));
        }
        found += comments.len();
    }
    if a.fix {
        println!("comments removed from {fixed} files");
        return Ok(());
    }
    if found > 0 {
        bail!("{found} comments in Rust sources (cargo xtask comments --fix removes them)");
    }
    println!("no comments in Rust sources");
    Ok(())
}

pub fn rust_files(root: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else { continue };
        for e in entries.flatten() {
            let path = e.path();
            let name = e.file_name().to_string_lossy().to_string();
            if path.is_dir() {
                let skip = name.starts_with('.') || name.starts_with("target") || name == "node_modules" || name == "dist";
                if !skip {
                    stack.push(path);
                }
            } else if name.ends_with(".rs") {
                out.push(path);
            }
        }
    }
    out.sort();
    out
}

fn uses_clap(src: &str) -> bool {
    src.contains("clap::")
}

fn is_comment(kind: TokenKind) -> bool {
    matches!(kind, TokenKind::LineComment { .. } | TokenKind::BlockComment { .. })
}

pub fn find(src: &str) -> Vec<Comment> {
    let mut out = Vec::new();
    let mut pos = 0usize;
    let mut line = 1usize;
    for t in tokenize(src, FrontmatterAllowed::No) {
        let text = &src[pos..pos + t.len as usize];
        if is_comment(t.kind) {
            out.push(Comment { line, text: text.to_string() });
        }
        line += text.matches('\n').count();
        pos += t.len as usize;
    }
    out
}

fn doc_attribute(text: &str) -> String {
    let body = text.strip_prefix("///").unwrap_or(text);
    let escaped = body.replace('\\', "\\\\").replace('"', "\\\"");
    format!("#[doc = \"{escaped}\"]")
}

pub fn strip(src: &str, docs_as_attributes: bool) -> String {
    let mut lines: Vec<String> = Vec::new();
    let mut cur = String::new();
    let mut touched = false;
    let finish = |cur: &mut String, touched: &mut bool, lines: &mut Vec<String>| {
        if *touched {
            if !cur.trim().is_empty() {
                lines.push(cur.trim_end().to_string());
            }
        } else {
            lines.push(cur.clone());
        }
        cur.clear();
        *touched = false;
    };
    let mut pos = 0usize;
    for t in tokenize(src, FrontmatterAllowed::No) {
        let text = &src[pos..pos + t.len as usize];
        pos += t.len as usize;
        match t.kind {
            TokenKind::LineComment { doc_style: Some(DocStyle::Outer) } if docs_as_attributes => {
                cur.push_str(&doc_attribute(text));
            }
            k if is_comment(k) => {
                touched = true;
                for _ in text.matches('\n') {
                    finish(&mut cur, &mut touched, &mut lines);
                    touched = true;
                }
            }
            _ => {
                for ch in text.chars() {
                    if ch == '\n' {
                        finish(&mut cur, &mut touched, &mut lines);
                    } else {
                        cur.push(ch);
                    }
                }
            }
        }
    }
    if !cur.is_empty() || touched {
        finish(&mut cur, &mut touched, &mut lines);
    }
    let mut out = String::with_capacity(src.len());
    let mut blank_run = true;
    for l in &lines {
        let blank = l.trim().is_empty();
        if blank && blank_run {
            continue;
        }
        blank_run = blank;
        out.push_str(l);
        out.push('\n');
    }
    while out.ends_with("\n\n") {
        out.pop();
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn line_block_and_doc_comments_are_found_but_not_slashes_in_strings() {
        let src = "//! m\n/// d\nfn f() -> &'static str { /* b */ \"http://x\" } // t\nlet r = r#\"// no\"#;\n";
        let lines: Vec<usize> = find(src).iter().map(|c| c.line).collect();
        assert_eq!(lines, vec![1, 2, 3, 3]);
    }

    #[test]
    fn strip_removes_comment_lines_and_trailing_comments_and_keeps_code() {
        let src = "//! m\n\nuse a;\n\n// why\n/// doc\nfn f() {\n    g(); // later\n    /* a\n       b */\n    h(\"//\");\n}\n";
        assert_eq!(strip(src, false), "use a;\n\nfn f() {\n    g();\n    h(\"//\");\n}\n");
    }

    #[test]
    fn in_clap_files_doc_comments_become_doc_attributes() {
        let src = "enum T {\n    /// Build \"it\"\n    Build,\n}\n";
        assert_eq!(strip(src, true), "enum T {\n    #[doc = \" Build \\\"it\\\"\"]\n    Build,\n}\n");
        assert!(find(&strip(src, true)).is_empty());
    }

    #[test]
    fn the_rust_sources_have_no_comments() {
        let root = crate::root();
        let mut bad = Vec::new();
        for path in rust_files(&root) {
            let src = std::fs::read_to_string(&path).unwrap();
            for c in find(&src) {
                bad.push(format!("{}:{}", path.strip_prefix(&root).unwrap_or(&path).display(), c.line));
            }
        }
        assert!(bad.is_empty(), "{} comments (cargo xtask comments --fix): {}", bad.len(), bad.iter().take(20).cloned().collect::<Vec<_>>().join(", "));
    }
}
