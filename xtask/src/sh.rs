use std::ffi::OsString;
use std::process::{Command, ExitStatus, Output};

use anyhow::{bail, Context, Result};

pub trait Sh {
    fn status(&mut self, cmd: &mut Command) -> std::io::Result<ExitStatus>;
    fn output(&mut self, cmd: &mut Command) -> std::io::Result<Output>;

    fn statuses(&mut self, cmds: &mut [Command]) -> Vec<std::io::Result<ExitStatus>> {
        cmds.iter_mut().map(|c| self.status(c)).collect()
    }
}

pub struct Real;

impl Sh for Real {
    fn status(&mut self, cmd: &mut Command) -> std::io::Result<ExitStatus> {
        cmd.status()
    }

    fn output(&mut self, cmd: &mut Command) -> std::io::Result<Output> {
        cmd.output()
    }

    fn statuses(&mut self, cmds: &mut [Command]) -> Vec<std::io::Result<ExitStatus>> {
        let n = std::thread::available_parallelism().unwrap_or(std::num::NonZeroUsize::MIN).get();
        cmds.chunks_mut(n)
            .flat_map(|chunk| {
                let children: Vec<_> = chunk.iter_mut().map(Command::spawn).collect();
                children.into_iter().map(|c| c.and_then(|mut c| c.wait())).collect::<Vec<_>>()
            })
            .collect()
    }
}

pub type Env<'a> = &'a dyn Fn(&str) -> Option<OsString>;

pub fn process_env(k: &str) -> Option<OsString> {
    std::env::var_os(k)
}

pub fn var(env: Env, k: &str) -> Option<String> {
    env(k).map(|v| v.to_string_lossy().into_owned())
}

fn checked(cmd: &Command, st: std::io::Result<ExitStatus>) -> Result<()> {
    let st = st.with_context(|| format!("{cmd:?}"))?;
    if !st.success() {
        bail!("{:?} failed ({st})", cmd.get_program());
    }
    Ok(())
}

pub fn run(sh: &mut dyn Sh, cmd: &mut Command) -> Result<()> {
    let st = sh.status(cmd);
    checked(cmd, st)
}

pub fn run_all(sh: &mut dyn Sh, cmds: &mut [Command]) -> Result<()> {
    let st = sh.statuses(cmds);
    cmds.iter().zip(st).try_for_each(|(cmd, st)| checked(cmd, st))
}

pub fn output(sh: &mut dyn Sh, cmd: &mut Command) -> Result<String> {
    let o = sh.output(cmd).with_context(|| format!("{cmd:?}"))?;
    if !o.status.success() {
        bail!("{:?} failed: {}", cmd.get_program(), String::from_utf8_lossy(&o.stderr));
    }
    Ok(String::from_utf8_lossy(&o.stdout).into_owned())
}

#[cfg(test)]
pub mod fake {
    use std::collections::BTreeMap;
    use std::ffi::OsString;
    use std::os::unix::process::ExitStatusExt;
    use std::process::{Command, ExitStatus, Output};

    pub fn describe(cmd: &Command) -> String {
        let mut s = String::new();
        if let Some(d) = cmd.get_current_dir() {
            s += &format!("cd {} && ", d.display());
        }
        for (k, v) in cmd.get_envs() {
            s += &match v {
                Some(v) => format!("{}={} ", k.to_string_lossy(), v.to_string_lossy()),
                None => format!("-u {} ", k.to_string_lossy()),
            };
        }
        s += &cmd.get_program().to_string_lossy();
        for a in cmd.get_args() {
            s.push(' ');
            s += &a.to_string_lossy();
        }
        s
    }

    pub fn program(cmd: &Command) -> String {
        std::path::Path::new(cmd.get_program()).file_name().unwrap_or_default().to_string_lossy().into_owned()
    }

    pub struct Fake<'a> {
        pub log: Vec<String>,
        pub on: Box<dyn FnMut(&Command) -> (i32, String) + 'a>,
    }

    impl<'a> Fake<'a> {
        pub fn new(on: impl FnMut(&Command) -> (i32, String) + 'a) -> Fake<'a> {
            Fake { log: Vec::new(), on: Box::new(on) }
        }

        fn run(&mut self, cmd: &Command) -> (ExitStatus, String) {
            self.log.push(describe(cmd));
            let (code, out) = (self.on)(cmd);
            (ExitStatus::from_raw(code << 8), out)
        }
    }

    impl super::Sh for Fake<'_> {
        fn status(&mut self, cmd: &mut Command) -> std::io::Result<ExitStatus> {
            Ok(self.run(cmd).0)
        }

        fn output(&mut self, cmd: &mut Command) -> std::io::Result<Output> {
            let (status, out) = self.run(cmd);
            let (stdout, stderr) = if status.success() { (out, String::new()) } else { (String::new(), out) };
            Ok(Output { status, stdout: stdout.into_bytes(), stderr: stderr.into_bytes() })
        }
    }

    pub struct Tmp(pub std::path::PathBuf);

    impl Tmp {
        pub fn new(name: &str) -> Tmp {
            static N: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
            let n = N.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            let d = std::env::temp_dir().join(format!("xtask-test-{}-{n}-{name}", std::process::id()));
            let _ = std::fs::remove_dir_all(&d);
            std::fs::create_dir_all(&d).unwrap();
            Tmp(d)
        }

        pub fn path(&self, f: &str) -> std::path::PathBuf {
            self.0.join(f)
        }

        pub fn put(&self, f: &str, data: impl AsRef<[u8]>) -> std::path::PathBuf {
            let p = self.path(f);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(&p, data).unwrap();
            p
        }
    }

    impl Drop for Tmp {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    pub fn env(vars: &[(&str, &str)]) -> impl Fn(&str) -> Option<OsString> {
        let m: BTreeMap<String, OsString> = vars.iter().map(|(k, v)| (k.to_string(), OsString::from(v))).collect();
        move |k| m.get(k).cloned()
    }
}

#[cfg(test)]
mod tests {
    use super::fake::{describe, env, program, Fake};
    use super::*;

    #[test]
    fn real_commands() {
        run(&mut Real, Command::new("sh").args(["-c", "exit 0"])).unwrap();
        let e = run(&mut Real, Command::new("sh").args(["-c", "exit 3"])).unwrap_err().to_string();
        assert_eq!(e, "\"sh\" failed (exit status: 3)");
        assert_eq!(output(&mut Real, Command::new("sh").args(["-c", "echo hi"])).unwrap(), "hi\n");
        let e = output(&mut Real, Command::new("sh").args(["-c", "echo no >&2; exit 1"])).unwrap_err().to_string();
        assert_eq!(e, "\"sh\" failed: no\n");
        let e = run(&mut Real, &mut Command::new("/nonexistent/tool")).unwrap_err().to_string();
        assert_eq!(e, "\"/nonexistent/tool\"");
        let e = output(&mut Real, &mut Command::new("/nonexistent/tool")).unwrap_err().to_string();
        assert_eq!(e, "\"/nonexistent/tool\"");
    }

    #[test]
    fn real_commands_side_by_side() {
        let n = std::thread::available_parallelism().unwrap().get();
        let t = super::fake::Tmp::new("par");
        let mark = |i: usize| {
            let mut c = Command::new("sh");
            c.args(["-c", &format!("echo $$ > {}/{i}; exit $(( {i} % 3 ))", t.0.display())]);
            c
        };
        let mut cmds: Vec<Command> = (0..n + 2).map(mark).collect();
        let st = Real.statuses(&mut cmds);
        let codes: Vec<i32> = st.iter().map(|s| s.as_ref().unwrap().code().unwrap()).collect();
        assert_eq!(codes, (0..n + 2).map(|i| (i % 3) as i32).collect::<Vec<_>>());
        assert!((0..n + 2).all(|i| t.path(&i.to_string()).exists()));
        let e = run_all(&mut Real, &mut cmds).unwrap_err().to_string();
        assert_eq!(e, "\"sh\" failed (exit status: 1)");
        let mut gone = [Command::new("/nonexistent/tool"), Command::new("true")];
        assert_eq!(run_all(&mut Real, &mut gone).unwrap_err().to_string(), "\"/nonexistent/tool\"");
        let mut fine = [Command::new("true"), Command::new("true")];
        run_all(&mut Real, &mut fine).unwrap();
    }

    #[test]
    fn fake_commands_are_described_and_answered() {
        let mut f = Fake::new(|c| if program(c) == "ok" { (0, "out".into()) } else { (2, "err".into()) });
        let mut c = Command::new("/bin/ok");
        c.current_dir("/w").env("B", "2").env("A", "1").env_remove("C").args(["x", "y"]);
        assert_eq!(output(&mut f, &mut c).unwrap(), "out");
        assert_eq!(output(&mut f, &mut Command::new("bad")).unwrap_err().to_string(), "\"bad\" failed: err");
        assert_eq!(run(&mut f, &mut Command::new("bad")).unwrap_err().to_string(), "\"bad\" failed (exit status: 2)");
        assert_eq!(f.log, ["cd /w && A=1 B=2 -u C /bin/ok x y", "bad", "bad"]);
        assert_eq!(describe(&Command::new("p")), "p");
    }

    #[test]
    fn scratch_directories() {
        let t = super::fake::Tmp::new("tmp");
        let p = t.put("a/b.txt", "x");
        assert_eq!(std::fs::read_to_string(&p).unwrap(), "x");
        let d = t.0.clone();
        drop(t);
        assert!(!d.exists());
    }

    #[test]
    fn variables() {
        let e = env(&[("A", "1")]);
        assert_eq!(var(&e, "A").as_deref(), Some("1"));
        assert_eq!(var(&e, "B"), None);
        assert_eq!(process_env("OUTGLOW_SURELY_NOT_SET_42"), None);
        assert!(process_env("PATH").is_some());
    }
}
