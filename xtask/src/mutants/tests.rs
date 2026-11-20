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

static LISTS_T: &[List] = &[List { krate: "demo", mutations: DEMO }, List { krate: "zigbee", mutations: CAUGHT }, List { krate: "ble", mutations: SLOW }];

const SOURCE: &str = "pub fn one() -> u8 { 1 }\npub fn two() -> u8 { 2 }\n";

const CARGO: &str = "#!/bin/sh\n\
    if [ \"$1\" = metadata ]; then cat metadata.json; exit 0; fi\n\
    f=crates/$3/src/lib.rs\n\
    echo '     Running unittests src/lib.rs' >&2\n\
    echo \"$3\" >> ../../../../ran.log\n\
    if grep -q LOCK $f; then chmod 555 crates/$3/src; echo 'test t ... FAILED'; exit 101; fi\n\
    if grep -q SLEEP $f; then exec sleep 5; fi\n\
    if grep -q BROKEN $f; then echo 'error[E0425]: cannot find value'; exit 101; fi\n\
    if grep -q PANIC $f; then echo 'thread main panicked' >&2; exit 101; fi\n\
    if grep -q '{ 2 }' $f && grep -q 'one() -> u8 { 2 }' $f; then echo 'test one_is_one ... FAILED'; echo 'test two ... FAILED'; exit 101; fi\n\
    echo 'test one_is_one ... ok'\n";

const ROOT_MANIFEST: &str = "[workspace]\nresolver = \"2\"\nmembers = [\"crates/demo\", \"xtask\"]\n\n[profile.dev.package.demo]\nopt-level = 3\n";

fn package(t: &Tmp, krate: &str, deps: &[&str]) -> String {
    let dir = t.path(krate);
    let mut deps: Vec<String> = deps.iter().map(|d| format!("{{\"name\":\"x\",\"path\":\"{}\"}}", dir.join(d).display())).collect();
    deps.push("{\"name\":\"anyhow\"}".into());
    format!("{{\"manifest_path\":\"{}\",\"dependencies\":[{}]}}", dir.join("Cargo.toml").display(), deps.join(","))
}

fn metadata(t: &Tmp, extra: &[String]) {
    let mut ps = vec![
        package(t, "crates/demo", &["../dep", "."]),
        package(t, "crates/zigbee", &[]),
        package(t, "crates/ble", &["../dep"]),
        package(t, "crates/dep", &["../dep2"]),
        package(t, "crates/dep2", &[]),
        package(t, "crates/unused", &[]),
    ];
    ps.extend_from_slice(extra);
    t.put("metadata.json", &format!("{{\"packages\":[{}]}}", ps.join(",")));
}

struct Rig {
    t: Tmp,
    sp: Space,
}

fn rig(name: &str) -> Rig {
    let t = Tmp::new(name);
    t.put("crates/demo/src/lib.rs", SOURCE);
    t.put("crates/zigbee/src/lib.rs", SOURCE);
    t.put("crates/ble/src/lib.rs", SOURCE);
    t.put("crates/demo/Cargo.toml", "[package]\nname = \"demo\"\n");
    t.put("crates/demo/target/debug/junk", "x");
    t.put("Cargo.lock", "# lock\n");
    t.put("Cargo.toml", ROOT_MANIFEST);
    t.put("crates/dep/src/lib.rs", "pub fn dep() {}\n");
    t.put("crates/dep2/src/lib.rs", "pub fn dep2() {}\n");
    t.put("crates/unused/src/lib.rs", "");
    metadata(&t, &[]);
    let cargo = t.put("bin/cargo", CARGO);
    std::fs::set_permissions(&cargo, std::fs::Permissions::from_mode(0o755)).unwrap();
    let sp = Space { root: t.0.clone(), crates: &["crates/demo", "crates/zigbee", "crates/ble"], lists: LISTS_T, cargo: cargo.into_os_string() };
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
    assert_eq!(o.caught, 2);
    assert_eq!(o.missed, ["missed: a harmless space"]);
    assert_eq!(o.unviable, ["broken: does not build"]);
    assert_eq!(o.stale, ["stale: no such file", "stale: twice"]);
    assert_eq!(o.summary("demo"), "demo: 4 mutations run: 2 caught, 1 missed, 1 unviable, 2 stale");
    assert_eq!(std::fs::read_to_string(scratch.join("w0/ws/crates/demo/src/lib.rs")).unwrap(), SOURCE);
    assert!(!scratch.join("w0/ws/crates/demo/target").exists());
    let manifest = std::fs::read_to_string(scratch.join("w1/ws/Cargo.toml")).unwrap();
    let members = "members = [\"crates/demo\", \"crates/zigbee\", \"crates/ble\", \"crates/dep\", \"crates/dep2\"]";
    assert!(manifest.ends_with(&ROOT_MANIFEST.replace("members = [\"crates/demo\", \"xtask\"]", members)), "{manifest}");
    assert!(manifest.starts_with("# Scratch copy"), "{manifest}");
    assert_eq!(std::fs::read_to_string(scratch.join("w1/ws/crates/dep2/src/lib.rs")).unwrap(), "pub fn dep2() {}\n");
    assert!(!scratch.join("w1/ws/crates/unused").exists());
    assert_eq!(std::fs::read_to_string(scratch.join("w1/ws/Cargo.lock")).unwrap(), "# lock\n");
    assert!(!scratch.join("w1/ws/rust-toolchain.toml").exists());
}

#[test]
fn the_verdict_of_a_test_run() {
    let v = |out: &str, ok: bool, slow: bool| match verdict(out, ok, slow) {
        Verdict::Caught(w) => format!("caught {w}"),
        Verdict::Missed => "missed".into(),
        Verdict::Unviable(w) => format!("unviable {w}"),
        Verdict::Stale(w) => w,
    };
    let ran = "     Running unittests src/lib.rs\n";
    assert_eq!(v("test a ... FAILED\ntest b::c ... FAILED  \ntest d ... ok\n", false, false), "caught 2 test(s), e.g. a");
    assert_eq!(v("error: could not compile `x`", false, false), "unviable does not compile");
    assert_eq!(v("test a ... FAILED\nerror[E1]", false, false), "unviable does not compile");
    assert_eq!(v("", false, true), "caught the tests did not finish in time");
    assert_eq!(v(ran, false, false), "caught a test process failed without a test result");
    assert_eq!(v("", false, false), "unviable cargo failed before running a test: no output");
    let lost = "error: failed to load manifest for workspace member\n  failed to read crates/memcap/Cargo.toml\n";
    assert_eq!(v(lost, false, false), "unviable cargo failed before running a test: error: failed to load manifest for workspace member");
    assert_eq!(v("warning: odd\n\n", false, false), "unviable cargo failed before running a test: warning: odd");
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
    assert_eq!((o.caught, o.missed.len(), o.unviable.len(), o.stale.len()), (1, 0, 0, 0));
}

#[test]
fn fail_fast_stops_at_the_first_miss() {
    let r = rig("fast");
    let scratch = r.t.path("target/mutants");
    let ms = refs(&DEMO[1..]);
    let o = run_list(&r.sp, "demo", &ms, &scratch, 1, true, None).unwrap();
    assert_eq!((o.caught, o.missed.len(), o.stale.len()), (0, 1, 0));
    let o = run_list(&r.sp, "demo", &refs(&DEMO[2..3]), &scratch, 1, true, None).unwrap();
    assert_eq!((o.caught, o.stale.len()), (0, 1));
    let o = run_list(&r.sp, "demo", &refs(&DEMO[4..]), &scratch, 1, true, None).unwrap();
    assert_eq!((o.caught, o.unviable.len()), (0, 1));
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
    assert_eq!(e, "no crate crates/missing");
    sp.crates = &["crates/demo"];
    r.t.put("Cargo.toml", "[workspace]\n");
    let e = run_list(&sp, "demo", &refs(CAUGHT), &scratch, 1, false, None).err().unwrap().to_string();
    assert_eq!(e, format!("mirroring the workspace to {}", scratch.join("w0/ws").display()));
    sp.cargo = "/nonexistent/cargo".into();
    let e = run_list(&sp, "demo", &refs(CAUGHT), &scratch, 1, false, None).err().unwrap().to_string();
    assert_eq!(e, "running cargo metadata");
}

#[test]
fn the_path_dependencies_are_mirrored_too() {
    let r = rig("closure");
    let fake = r.t.path("bin/cargo").into_os_string();
    let all = |sp: &Space| path_closure(sp).map_err(|e| e.to_string());
    assert_eq!(all(&r.sp).unwrap(), ["crates/demo", "crates/zigbee", "crates/ble", "crates/dep", "crates/dep2"]);
    let sp = Space { crates: &["crates/zigbee"], ..r.sp };
    assert_eq!(all(&sp).unwrap(), ["crates/zigbee"]);
    let sp = Space { crates: &["crates/dep", "crates/demo"], ..sp };
    assert_eq!(all(&sp).unwrap(), ["crates/dep", "crates/demo", "crates/dep2"]);
    let outside = r.t.path("../outside-closure");
    std::fs::create_dir_all(&outside).unwrap();
    metadata(&r.t, &[package(&r.t, "crates/zigbee", &["../../../outside-closure"]), package(&r.t, "../outside-closure", &[])]);
    let sp = Space { crates: &["crates/zigbee"], ..sp };
    let e = all(&sp).err().unwrap();
    assert!(e.ends_with(&format!("outside {}", r.t.0.canonicalize().unwrap().display())), "{e}");
    std::fs::remove_dir_all(&outside).unwrap();
    metadata(&r.t, &[package(&r.t, "crates/zigbee", &["../nowhere"])]);
    assert!(all(&sp).err().unwrap().ends_with("crates/zigbee/../nowhere is not a package of the workspace"));
    r.t.put("metadata.json", "{");
    assert_eq!(all(&sp).err().unwrap(), "reading the output of cargo metadata");
    let bad = r.t.put("bin/bad-cargo", "#!/bin/sh\nexit 1\n");
    std::fs::set_permissions(&bad, std::fs::Permissions::from_mode(0o755)).unwrap();
    let sp = Space { cargo: bad.into_os_string(), ..sp };
    assert_eq!(all(&sp).err().unwrap(), "cargo metadata failed");
    let sp = Space { root: r.t.path("nonexistent"), cargo: fake.clone(), ..sp };
    assert_eq!(all(&sp).err().unwrap(), "running cargo metadata");
    let e = cargo_test(&fake, "demo", &r.t.path("nonexistent"), &r.t.0, None).err().unwrap().to_string();
    assert_eq!(e, "starting cargo test");
}

#[test]
fn a_failing_baseline_stops_the_run() {
    let r = rig("baseline");
    let scratch = r.t.path("target/mutants");
    let out = scratch.join("w0/stdout.txt");
    r.t.put("crates/demo/src/lib.rs", "pub fn one() -> u8 { 2 }\n");
    let e = run_list(&r.sp, "demo", &refs(CAUGHT), &scratch, 1, false, None).err().unwrap().to_string();
    assert_eq!(e, format!("the tests of demo fail without a mutation (2 test(s), e.g. one_is_one); output in {}", out.display()));
    r.t.put("crates/demo/src/lib.rs", "pub fn one() -> u8 { BROKEN }\n");
    let e = run_list(&r.sp, "demo", &refs(CAUGHT), &scratch, 1, false, None).err().unwrap().to_string();
    assert_eq!(e, format!("the tests of demo fail without a mutation (does not compile); output in {}", out.display()));
    let ran = std::fs::read_to_string(r.t.path("ran.log")).unwrap();
    assert_eq!(ran, "demo\ndemo\n");
}

#[test]
fn the_scratch_manifest() {
    let m = scratch_manifest("a\n  members = [\"x\"]\nmembers = []\n", &["p".into(), "q".into()]).unwrap();
    assert_eq!(m, "# Scratch copy for `cargo xtask mutants`; rewritten on every run.\na\nmembers = [\"p\", \"q\"]\nmembers = []\n");
    assert_eq!(scratch_manifest("[workspace]\n", &[]).err().unwrap().to_string(), "no workspace members in the root Cargo.toml");
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
    let t0 = Instant::now();
    run_in(&r.sp, &args(&["ble", "--timeout", "1"]), None).unwrap();
    assert!(t0.elapsed() < Duration::from_secs(4));
    let e = run_in(&r.sp, &args(&["-j", "0", "--timeout", "0"]), Some("harmless".into())).err().unwrap().to_string();
    assert_eq!(e, "mutations missed, unviable or stale");
    let e = run_in(&r.sp, &args(&["--filter", "stale"]), None).err().unwrap().to_string();
    assert_eq!(e, "mutations missed, unviable or stale");
    let e = run_in(&r.sp, &args(&["--filter", "broken"]), None).err().unwrap().to_string();
    assert_eq!(e, "mutations missed, unviable or stale");
    let ran = |r: &Rig| std::fs::read_to_string(r.t.path("ran.log")).unwrap_or_default();
    let before = ran(&r).len();
    let e = run_in(&r.sp, &args(&["--fail-fast"]), Some(String::new().into())).err().unwrap().to_string();
    assert_eq!(e, "mutations missed, unviable or stale");
    assert_eq!(&ran(&r)[before..], "demo\ndemo\ndemo\n");
    let before = ran(&r).len();
    let e = run_in(&r.sp, &args(&[]), Some("harmless|one is two".into())).err().unwrap().to_string();
    assert_eq!(e, "mutations missed, unviable or stale");
    assert_eq!(&ran(&r)[before..], "demo\ndemo\ndemo\nzigbee\nzigbee\n");
    let mut sp = Space { cargo: "/nonexistent".into(), ..r.sp };
    assert_eq!(run_in(&sp, &args(&["zigbee"]), None).err().unwrap().to_string(), "running cargo metadata");
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
