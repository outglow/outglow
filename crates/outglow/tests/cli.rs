use memcap as _;

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use outglow_image::telink;

struct Tmp(PathBuf);

impl Tmp {
    fn new(name: &str) -> Tmp {
        let d = std::env::temp_dir().join(format!("outglow-cli-test-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        Tmp(d)
    }

    fn path(&self, f: &str) -> PathBuf {
        self.0.join(f)
    }
}

impl Drop for Tmp {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn outglow(home: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_outglow")).args(args).env("HOME", home).env_remove("MQTT_BROKER").output().unwrap()
}

fn text(b: &[u8]) -> String {
    String::from_utf8_lossy(b).into_owned()
}

fn image(text: &[u8]) -> Vec<u8> {
    let mut body = vec![0u8; 0x400];
    body[8..12].copy_from_slice(b"KNLT");
    body[0x100..0x100 + text.len()].copy_from_slice(text);
    telink::finalize(&body)
}

fn closed_port() -> String {
    let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    l.local_addr().unwrap().port().to_string()
}

#[test]
fn image_describes_a_file() {
    let t = Tmp::new("image");
    let p = t.path("x-ota.bin");
    std::fs::write(&p, image(b"ESMLFzm_w6_Dimm\0 v3.0.2 4.63\0")).unwrap();
    let o = outglow(&t.0, &["image", p.to_str().unwrap()]);
    assert!(o.status.success());
    assert_eq!(text(&o.stdout), "original firmware of ESMLFzm_w6_Dimm, hardware 4.63\nheader and CRC: ok (1028 B; the bootloader takes it)\n");
    let o = outglow(&t.0, &["image", t.path("gone").to_str().unwrap()]);
    assert!(!o.status.success());
    assert!(text(&o.stderr).starts_with("Error: No such file or directory"), "{}", text(&o.stderr));
}

#[test]
fn ble_vectors_for_the_web_flasher() {
    let t = Tmp::new("vectors");
    let o = outglow(&t.0, &["ble", "vectors"]);
    assert!(o.status.success());
    assert!(text(&o.stdout).starts_with("{\"image\": \""));
    let p = t.path("x.bin");
    std::fs::write(&p, image(b"outglow-board:ESMLFzm_w6_TW-4.63\0")).unwrap();
    let o = outglow(&t.0, &["ble", "vectors", p.to_str().unwrap()]);
    assert!(o.status.success());
    assert!(text(&o.stdout).contains("\"verify\": true, \"count\": 65,"));
    let o = outglow(&t.0, &["ble", "vectors", t.path("gone").to_str().unwrap()]);
    assert!(!o.status.success());
}

#[test]
fn box_from_a_saved_file() {
    let t = Tmp::new("box");
    let blob = include_str!("../src/diag/testdata/box.hex").trim();
    let saved = t.path("box.json");
    std::fs::write(&saved, format!("{{\"raw\": \"{blob}\", \"missing\": []}}")).unwrap();
    let csv = t.path("rows.csv");
    let o = outglow(&t.0, &["box", "--from", saved.to_str().unwrap(), "--csv", csv.to_str().unwrap()]);
    assert!(o.status.success(), "{}", text(&o.stderr));
    assert_eq!(text(&o.stdout), include_str!("../src/diag/testdata/box.report.txt"));
    assert!(std::fs::read_to_string(&csv).unwrap().starts_with("minute,tx,"));
    let again = t.path("again.json");
    let o = outglow(&t.0, &["box", "--from", saved.to_str().unwrap(), "--json", again.to_str().unwrap()]);
    assert!(text(&o.stdout).ends_with(&format!("(saved {})\n", again.display())));
    let o = outglow(&t.0, &["box", "--from", t.path("gone").to_str().unwrap()]);
    assert!(!o.status.success());
}

#[test]
fn diag_and_box_without_a_broker() {
    let t = Tmp::new("mqtt");
    let port = closed_port();
    for cmd in ["diag", "box"] {
        let o = outglow(&t.0, &[cmd, "0xa4c1380000123456", "--broker", "127.0.0.1", "--port", &port, "--timeout", "5"]);
        assert!(!o.status.success());
        let e = text(&o.stderr);
        assert!(e.starts_with("Error: MQTT broker 127.0.0.1: I/O: "), "{cmd}: {e}");
    }
    assert!(t.path(".cache/bulb-diag").is_dir());
}

#[test]
fn help_names_every_command() {
    let t = Tmp::new("help");
    let o = outglow(&t.0, &["--help"]);
    assert!(o.status.success());
    let h = text(&o.stdout);
    for c in ["ble", "diag", "box", "image"] {
        assert!(h.contains(&format!("  {c} ")), "{c}: {h}");
    }
    let o = outglow(&t.0, &["ble", "push", "--help"]);
    assert!(text(&o.stdout).contains("--allow-outglow"));
}
