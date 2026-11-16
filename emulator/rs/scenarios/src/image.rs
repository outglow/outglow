use std::collections::HashMap;
use std::path::{Path, PathBuf};

use tc32emu::isa::to_thumb;

#[derive(Clone, Debug)]
pub struct Image {
    pub label: String,
    pub path: PathBuf,
    pub link_shift: Option<i64>,
}

impl Image {
    pub fn parse(spec: &str) -> Result<Image, String> {
        let (label, rest) = spec.split_once('=').ok_or_else(|| format!("image spec {spec:?}: label=path expected"))?;
        let (path, shift) = match rest.rsplit_once('@') {
            Some((p, s)) => {
                let v = i64::from_str_radix(s.trim_start_matches("0x").trim_start_matches("0X"), 16)
                    .map_err(|e| format!("image spec {spec:?}: offset: {e}"))?;
                (p, Some(v))
            }
            None => (rest, None),
        };
        let mut p = PathBuf::from(path);
        if p.is_relative() && !p.exists() {
            let r = crate::runner::repo_root().join(&p);
            if r.exists() {
                p = r;
            } else if let Some(d) = crate::sims::stock_dir_path(path) {
                p = d;
            }
        }
        Ok(Image { label: label.to_string(), path: p, link_shift: shift })
    }

    pub fn bytes(&self) -> Result<Vec<u8>, String> {
        std::fs::read(&self.path).map_err(|e| format!("{}: {e}", self.path.display()))
    }

    pub fn shift(&self, img: &[u8]) -> i64 {
        self.link_shift.unwrap_or_else(|| link_shift(img, Some(&self.path)))
    }

    pub fn node(&self, cpi: i64) -> Result<zbnode::Node, String> {
        let bytes = self.bytes()?;
        let shift = self.shift(&bytes);
        let mut node = zbnode::Node::new(&bytes, cpi, shift)?;
        if std::env::var_os("ZBNODE_NO_IDLE").is_none() {
            if let Some((at, size)) = self.symbol_sized("bulb_idle") {
                node.idle = vec![tc32emu::Range { lo: at & !1, hi: (at & !1) + size }];
            }
        }
        Ok(node)
    }

    pub fn symbol(&self, name: &str) -> Option<u32> {
        self.symbol_sized(name).map(|(v, _)| v)
    }

    pub fn symbol_sized(&self, name: &str) -> Option<(u32, u32)> {
        let s = self.path.to_string_lossy();
        let stem = s.strip_suffix(".bin").unwrap_or(&s).to_string();
        let cands = [format!("{stem}.elf"), format!("{}.elf", stem.strip_suffix("-ota").unwrap_or(&stem))];
        cands.iter().find_map(|elf| std::fs::read(elf).ok().and_then(|d| elf_symbol_sized(&d, name)))
    }
}

pub fn link_shift(img: &[u8], path: Option<&Path>) -> i64 {
    if let Some(p) = path {
        let s = p.to_string_lossy();
        let stem = s.strip_suffix(".bin").unwrap_or(&s).to_string();
        let cands = [format!("{stem}.elf"), format!("{}.elf", stem.strip_suffix("-ota").unwrap_or(&stem))];
        for elf in cands {
            if let Ok(data) = std::fs::read(&elf) {
                for name in ["FW_OFFSET", "__FW_OFFSET"] {
                    if let Some(v) = elf_symbol(&data, name) {
                        return v as i64;
                    }
                }
            }
        }
    }
    guess_shift(img)
}

fn guess_shift(img: &[u8]) -> i64 {
    let n = img.len();
    if n < 0x10 {
        return 0;
    }
    let ram = (u32::from_le_bytes(img[0x0C..0x10].try_into().unwrap()) & 0xFFFF) as usize * 16;
    let mut push = vec![false; n];
    let mut a = ram;
    while a + 1 < n {
        let hw = u16::from_le_bytes([img[a], img[a + 1]]);
        if to_thumb(hw) & 0xFE00 == 0xB400 {
            push[a] = true;
        }
        a += 2;
    }
    let mut order: Vec<(i64, u32)> = Vec::new();
    let mut idx: HashMap<i64, usize> = HashMap::new();
    let mut a = 0;
    while a + 3 < n {
        let w = (u32::from_le_bytes(img[a..a + 4].try_into().unwrap()) & !1) as i64;
        if ram as i64 <= w && w < 0x100000 {
            let mut d: i64 = 0;
            while d < 0x100000 - n as i64 {
                let x = w - d;
                if x >= 0 && (x as usize) < n && push[x as usize] {
                    match idx.get(&d) {
                        Some(&i) => order[i].1 += 1,
                        None => {
                            idx.insert(d, order.len());
                            order.push((d, 1));
                        }
                    }
                }
                d += 0x1000;
            }
        }
        a += 4;
    }
    let mut best = (0i64, 0u32);
    for &(d, c) in &order {
        if c > best.1 {
            best = (d, c);
        }
    }
    let at0 = idx.get(&0).map(|&i| order[i].1).unwrap_or(0);
    if best.1 > 20.max(2 * at0) {
        best.0
    } else {
        0
    }
}

pub fn elf_symbol(d: &[u8], name: &str) -> Option<u32> {
    elf_symbol_sized(d, name).map(|(v, _)| v)
}

pub fn elf_symbol_sized(d: &[u8], name: &str) -> Option<(u32, u32)> {
    if d.get(..4)? != b"\x7fELF" || *d.get(4)? != 1 || *d.get(5)? != 1 {
        return None;
    }
    let u16_at = |i: usize| -> Option<u16> { Some(u16::from_le_bytes(d.get(i..i + 2)?.try_into().ok()?)) };
    let u32_at = |i: usize| -> Option<u32> { Some(u32::from_le_bytes(d.get(i..i + 4)?.try_into().ok()?)) };
    let shoff = u32_at(0x20)? as usize;
    let shentsize = u16_at(0x2E)? as usize;
    let shnum = u16_at(0x30)? as usize;
    let sec = |k: usize| shoff + k * shentsize;
    for k in 0..shnum {
        if u32_at(sec(k) + 4)? != 2 {
            continue;
        }
        let off = u32_at(sec(k) + 16)? as usize;
        let size = u32_at(sec(k) + 20)? as usize;
        let link = u32_at(sec(k) + 24)? as usize;
        let str_off = u32_at(sec(link) + 16)? as usize;
        let mut s = off;
        while s + 16 <= off + size {
            let nm = u32_at(s)? as usize;
            let start = str_off + nm;
            let end = start + d.get(start..)?.iter().position(|&c| c == 0)?;
            if &d[start..end] == name.as_bytes() {
                return Some((u32_at(s + 4)?, u32_at(s + 8)?));
            }
            s += 16;
        }
    }
    None
}
