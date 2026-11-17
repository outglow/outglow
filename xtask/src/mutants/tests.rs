use std::os::unix::fs::PermissionsExt;

use super::*;
use crate::sh::fake::Tmp;

static DEMO: &[Mutation] = &[
    Mutation { name: "caught: one is two", file: "lib.rs", old: "one() -> u8 { 1 }", new: "one() -> u8 { 2 }" },
    Mutation { name: "missed: a harmless space", file: "lib.rs", old: "fn one", new: "fn  one" },
    Mutation { name: "stale: no such file", file: "gone.rs", old: "x", new: "y" },
    Mutation { name: "stale: twice", file: "lib.rs", old: "u8", new: "u16" },
    Mutation { name: "broken: does not build", file: "lib.rs", old: "{ 1 }", new: "{ BROKEN }" },
    Mutation { name: "caught: the process fails", file: "lib.rs", old: "fn one", new: "fn PANIC_one" },
];

static SLOW: &[Mutation] = &[Mutation { name: "slow: never ends", file: "lib.rs", old: "fn one", new: "fn SLEEP_one" }];

static CAUGHT: &[Mutation] = &[Mutation { name: "caught: one is two", file: "lib.rs", old: "one() -> u8 { 1 }", new: "one() -> u8 { 2 }" }];

static RESTORE: &[Mutation] = &[Mutation { name: "locks the sources", file: "lib.rs", old: "fn one", new: "fn LOCK_one" }];

static LISTS_T: &[List] = &[List { krate: "demo", mutations: DEMO }, List { krate: "zigbee", mutations: CAUGHT }];

const SOURCE: &str = "pub fn one() -> u8 { 1 }\npub fn two() -> u8 { 2 }\n";

const CARGO: &str = "#!/bin/sh\n\
    f=crates/$3/src/lib.rs\n\
    if grep -q LOCK $f; then chmod 555 crates/$3/src; echo 'test t ... FAILED'; exit 101; fi\n\
    if grep -q SLEEP $f; then exec sleep 30; fi\n\
    if grep -q BROKEN $f; then echo 'error[E0425]: cannot find value'; exit 101; fi\n\
    if grep -q PANIC $f; then echo 'thread main panicked' >&2; exit 101; fi\n\
    if grep -q '{ 2 }' $f && grep -q 'one() -> u8 { 2 }' $f; then echo 'test one_is_one ... FAILED'; echo 'test two ... FAILED'; exit 101; fi\n\
    echo 'test one_is_one ... ok'\n";

struct Rig {
    t: Tmp,
    sp: Space,
}

fn rig(name: &str) -> Rig {
    let t = Tmp::new(name);
    t.put("crates/demo/src/lib.rs", SOURCE);
    t.put("crates/zigbee/src/lib.rs", SOURCE);
    t.put("crates/demo/Cargo.toml", "[package]\nname = \"demo\"\n");
    t.put("crates/demo/target/debug/junk", "x");
    t.put("Cargo.lock", "# lock\n");
    let cargo = t.put("bin/cargo", CARGO);
    std::fs::set_permissions(&cargo, std::fs::Permissions::from_mode(0o755)).unwrap();
    let sp = Space { root: t.0.clone(), crates: &["crates/demo", "crates/zigbee"], lists: LISTS_T, cargo: cargo.into_os_string() };
    Rig { t, sp }
}

fn args(extra: &[&str]) -> Args {
    use clap::Parser;
    #[derive(Parser)]
    struct Cli {
        #[command(flatten)]
        a: Args,
    }
    let mut v = vec!["mutants"];
    v.extend_from_slice(extra);
    Cli::try_parse_from(v).unwrap().a
}

fn refs(ms: &'static [Mutation]) -> Vec<&'static Mutation> {
    ms.iter().collect()
}

#[test]
fn every_kind_of_verdict() {
    let r = rig("verdicts");
    let scratch = r.t.path("target/mutants");
    let o = run_list(&r.sp, "demo", &refs(DEMO), &scratch, 2, false, None).unwrap();
    assert_eq!(o.ran, 4);
    assert_eq!(o.missed, ["missed: a harmless space"]);
    assert_eq!(o.stale, ["stale: no such file", "stale: twice", "broken: does not build"]);
    assert_eq!(std::fs::read_to_string(scratch.join("w0/ws/crates/demo/src/lib.rs")).unwrap(), SOURCE);
    assert!(!scratch.join("w0/ws/crates/demo/target").exists());
    let manifest = std::fs::read_to_string(scratch.join("w1/ws/Cargo.toml")).unwrap();
    assert!(manifest.ends_with("[workspace]\nresolver = \"2\"\nmembers = [\"crates/demo\", \"crates/zigbee\"]\n"), "{manifest}");
    assert_eq!(std::fs::read_to_string(scratch.join("w1/ws/Cargo.lock")).unwrap(), "# lock\n");
    assert!(!scratch.join("w1/ws/rust-toolchain.toml").exists());
}

#[test]
fn the_verdict_of_a_test_run() {
    let v = |out: &str, ok: bool, slow: bool| match verdict("m", out, ok, slow) {
        Verdict::Caught(w) => format!("caught {w}"),
        Verdict::Missed => "missed".into(),
        Verdict::Stale(w) => w,
    };
    assert_eq!(v("test a ... FAILED\ntest b::c ... FAILED  \ntest d ... ok\n", false, false), "caught 2 test(s), e.g. a");
    assert_eq!(v("error: could not compile `x`", false, false), "BROKEN  m: does not compile");
    assert_eq!(v("test a ... FAILED\nerror[E1]", false, false), "BROKEN  m: does not compile");
    assert_eq!(v("", false, true), "caught the tests did not finish in time");
    assert_eq!(v("", false, false), "caught a test process failed without a test result");
    assert_eq!(v("test a ... ok\nnot a test ... FAILED\n", true, false), "missed");
    assert_eq!(v("test a ... FAILED", true, true), "caught 1 test(s), e.g. a");
}

#[test]
fn a_test_run_that_hangs_is_killed() {
    let r = rig("timeout");
    let scratch = r.t.path("target/mutants");
    let t0 = Instant::now();
    let o = run_list(&r.sp, "demo", &refs(SLOW), &scratch, 1, false, Some(Duration::from_millis(300))).unwrap();
    assert!(t0.elapsed() < Duration::from_secs(10));
    assert_eq!((o.ran, o.missed.len(), o.stale.len()), (1, 0, 0));
}

#[test]
fn fail_fast_stops_at_the_first_miss() {
    let r = rig("fast");
    let scratch = r.t.path("target/mutants");
    let ms = refs(&DEMO[1..]);
    let o = run_list(&r.sp, "demo", &ms, &scratch, 1, true, None).unwrap();
    assert_eq!((o.ran, o.missed.len(), o.stale.len()), (1, 1, 0));
    let o = run_list(&r.sp, "demo", &refs(&DEMO[2..3]), &scratch, 1, true, None).unwrap();
    assert_eq!((o.ran, o.stale.len()), (0, 1));
}

#[test]
fn the_sources_are_restored_or_the_run_fails() {
    let r = rig("restore");
    let scratch = r.t.path("target/mutants");
    let e = run_list(&r.sp, "demo", &refs(RESTORE), &scratch, 1, false, None).err().unwrap();
    let src = scratch.join("w0/ws/crates/demo/src");
    std::fs::set_permissions(&src, std::fs::Permissions::from_mode(0o755)).unwrap();
    assert_eq!(e.to_string(), format!("restoring {}", src.join("lib.rs").display()));
    let mut sp = Space { crates: &["crates/missing"], ..r.sp };
    let e = run_list(&sp, "demo", &refs(CAUGHT), &scratch, 1, false, None).err().unwrap().to_string();
    assert_eq!(e, format!("mirroring the workspace to {}", scratch.join("w0/ws").display()));
    sp.crates = &["crates/demo"];
    sp.cargo = "/nonexistent/cargo".into();
    let e = run_list(&sp, "demo", &refs(CAUGHT), &scratch, 1, false, None).err().unwrap().to_string();
    assert_eq!(e, "starting cargo test");
}

#[test]
fn the_lists_by_suite_and_filter() {
    let r = rig("lists");
    run_in(&r.sp, &args(&["--list"]), None).unwrap();
    run_in(&r.sp, &args(&["zigbee", "--list", "-f", "one|x"]), Some("ignored".into())).unwrap();
    assert!(chosen(Suite::All, "x") && chosen(Suite::Zigbee, "zigbee") && !chosen(Suite::Zigbee, "ble"));
    assert!(chosen(Suite::Ble, "ble") && !chosen(Suite::Ble, "zigbee"));
}

#[test]
fn a_run_of_the_lists() {
    let r = rig("run");
    run_in(&r.sp, &args(&["zigbee", "--timeout", "0"]), None).unwrap();
    run_in(&r.sp, &args(&["--filter", "one is two"]), None).unwrap();
    run_in(&r.sp, &args(&["ble"]), None).unwrap();
    let e = run_in(&r.sp, &args(&["-j", "0", "--timeout", "60"]), Some("harmless".into())).err().unwrap().to_string();
    assert_eq!(e, "mutations missed or stale");
    let e = run_in(&r.sp, &args(&["--filter", "stale"]), None).err().unwrap().to_string();
    assert_eq!(e, "mutations missed or stale");
    let e = run_in(&r.sp, &args(&["--fail-fast"]), Some(String::new().into())).err().unwrap().to_string();
    assert_eq!(e, "mutations missed or stale");
    let mut sp = Space { cargo: "/nonexistent".into(), ..r.sp };
    assert_eq!(run_in(&sp, &args(&["zigbee"]), None).err().unwrap().to_string(), "starting cargo test");
    sp.lists = &[];
    run_in(&sp, &args(&[]), None).unwrap();
}

#[test]
fn a_mirror_follows_its_source() {
    let t = Tmp::new("mirror");
    t.put("src/a.txt", "a");
    t.put("src/d/b.txt", "b");
    t.put("src/f", "now a file");
    t.put("src/target/x", "skipped");
    t.put("dst/old.txt", "gone");
    t.put("dst/olddir/x", "gone");
    t.put("dst/f/inner", "was a dir");
    t.put("dst/d", "was a file");
    t.put("dst/a.txt", "a");
    mirror_dir(&t.path("src"), &t.path("dst")).unwrap();
    let mut all = vec![];
    let mut stack = vec![t.path("dst")];
    while let Some(d) = stack.pop() {
        for e in std::fs::read_dir(d).unwrap() {
            let p = e.unwrap().path();
            if p.is_dir() {
                stack.push(p);
            } else {
                all.push((p.strip_prefix(t.path("dst")).unwrap().display().to_string(), std::fs::read_to_string(&p).unwrap()));
            }
        }
    }
    all.sort();
    assert_eq!(all, [("a.txt".to_string(), "a".to_string()), ("d/b.txt".into(), "b".into()), ("f".into(), "now a file".into())]);
    assert!(mirror_dir(&t.path("nonexistent"), &t.path("dst2")).is_err());
}

#[test]
fn whole_writes() {
    let t = Tmp::new("writes");
    let p = t.path("a/b.txt");
    put(&p, b"x").unwrap();
    put(&p, b"x").unwrap();
    assert_eq!(std::fs::read(&p).unwrap(), b"x");
    put(&p, b"y").unwrap();
    assert_eq!(std::fs::read(&p).unwrap(), b"y");
    let bad = t.path("nonexistent/c.txt");
    assert_eq!(write_whole(&bad, b"z").err().unwrap().to_string(), format!("writing {}", bad.display()));
    std::fs::create_dir_all(t.path("dir/sub")).unwrap();
    let e = write_whole(&t.path("dir"), b"z").err().unwrap().to_string();
    assert_eq!(e, format!("writing {}", t.path("dir").display()));
    assert!(!t.path("dir.mutants-tmp").exists());
}

#[test]
fn a_process_tree_is_killed() {
    let mut child = Command::new("sh").args(["-c", "sleep 30 & sleep 30; wait"]).spawn().unwrap();
    std::thread::sleep(Duration::from_millis(200));
    let t0 = Instant::now();
    kill_tree(child.id());
    child.wait().unwrap();
    assert!(t0.elapsed() < Duration::from_secs(5));
}
