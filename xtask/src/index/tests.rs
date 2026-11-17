use std::path::Path;

use outglow_image::telink;
use serde_json::json;

use super::*;
use crate::sh::fake::Tmp;

fn image(text: &[u8]) -> Vec<u8> {
    let mut body = vec![0u8; 0x400];
    body[8..12].copy_from_slice(b"KNLT");
    body[0x100..0x100 + text.len()].copy_from_slice(text);
    telink::finalize(&body)
}

fn built(t: &Tmp, dir: &str, board: &str, kind: &str, image_type: u16, file_version: u32, ble: bool) -> PathBuf {
    let img = image(format!("outglow-board:{board}\0{kind}\0").as_bytes());
    let f = ota::File::with_image(ota::Header::new(0x1141, image_type, file_version, &format!("outglow {board}")), img.clone()).pack();
    if ble {
        t.put(&format!("{dir}/bulb-tc32-ota.bin"), &img);
    }
    t.put(&format!("{dir}/bulb-tc32.ota"), f)
}

fn args(ota: &[&PathBuf], extra: &[&str]) -> Args {
    use clap::Parser;
    #[derive(Parser)]
    struct Cli {
        #[command(flatten)]
        a: Args,
    }
    let mut v: Vec<String> = vec!["index".into()];
    v.extend(ota.iter().map(|p| p.display().to_string()));
    v.extend(extra.iter().map(|s| s.to_string()));
    Cli::try_parse_from(v).unwrap().a
}

fn read(p: &Path) -> Value {
    serde_json::from_str(&std::fs::read_to_string(p).unwrap()).unwrap()
}

#[test]
fn dates_from_days() {
    assert_eq!(date_of(0), "1970-01-01");
    assert_eq!(date_of(59), "1970-03-01");
    assert_eq!(date_of(11_016), "2000-02-29");
    assert_eq!(date_of(11_017), "2000-03-01");
    assert_eq!(date_of(20_774), "2026-11-17");
    assert_eq!(date_of(-1), "1969-12-31");
    assert_eq!(date_of(-719_468), "0000-03-01");
    assert_eq!(date_of(2_932_896), "9999-12-31");
    let t = today();
    assert_eq!((t.len(), &t[4..5], &t[7..8]), (10, "-", "-"));
    assert!(t.as_str() >= "2026-11-17");
}

#[test]
fn hashes_in_hex() {
    assert_eq!(hex::<Sha256>(b""), "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855");
}

#[test]
fn an_index_and_a_release_list_from_two_boards() {
    let t = Tmp::new("index");
    let dimm = built(&t, "a", "ESMLFzm_w6_Dimm-4.63", "outglow-dimm", 0x0390, 0x0100_0001, true);
    let tw = built(&t, "b", "ESMLFzm_w6_TW-4.63", "outglow-tw", 0x0391, 0x0100_0001, false);
    let index = t.put("index.json", serde_json::to_string(&json!([
        {"imageType": 0x0391, "manufacturerCode": 0x1141, "fileVersion": 0x0100_0001, "fileName": "old"},
        {"imageType": 0x0391, "manufacturerCode": 0x1141, "fileVersion": 0x0100_0000, "fileName": "older"},
        {"imageType": 0x0390, "manufacturerCode": 0x1234, "fileVersion": 0x0100_0001, "fileName": "other maker"},
    ])).unwrap());
    let site = t.put("firmware.json", serde_json::to_string(&json!({"schema": 1, "images": [
        {"board": "ESMLFzm_w6_Dimm-4.63", "version": "1.0.1", "fileVersion": "0x01000001", "url": "old"},
        {"board": "ESMLFzm_w6_Dimm-4.63", "version": "1.0.0", "fileVersion": "0x01000000"},
        {"board": "A", "version": "0.1", "fileVersion": "bad"},
    ]})).unwrap());
    let assets = t.path("assets/out");
    let a = args(&[&dimm], &["--version", "1.0.1", "--index", index.to_str().unwrap(), "--site", site.to_str().unwrap(), "--assets", assets.to_str().unwrap(), "--notes", "fixes"]);
    run_in(&t.0, &a, "2026-11-17").unwrap();
    let a = args(&[&tw], &["--version", "1.0.1", "--tag", "t1", "--index", index.to_str().unwrap(), "--assets", assets.to_str().unwrap(), "--base-url", "https://x"]);
    run_in(&t.0, &a, "2026-11-17").unwrap();
    let ix = read(&index);
    let names: Vec<&str> = ix.as_array().unwrap().iter().map(|e| e["fileName"].as_str().unwrap()).collect();
    assert_eq!(names, ["other maker", "outglow-ESMLFzm_w6_Dimm-4.63-1.0.1.ota", "older", "outglow-ESMLFzm_w6_TW-4.63-1.0.1.ota"]);
    let d = &ix[1];
    let data = std::fs::read(&dimm).unwrap();
    assert_eq!(d["url"], "https://github.com/outglow/outglow/releases/download/v1.0.1/outglow-ESMLFzm_w6_Dimm-4.63-1.0.1.ota");
    assert_eq!((d["fileVersion"].as_u64(), d["fileSize"].as_u64(), d["imageType"].as_u64(), d["manufacturerCode"].as_u64()), (Some(0x0100_0001), Some(data.len() as u64), Some(0x0390), Some(0x1141)));
    assert_eq!(d["sha512"], hex::<Sha512>(&data));
    assert_eq!(d["otaHeaderString"], "outglow ESMLFzm_w6_Dimm-4.63");
    assert_eq!(d["modelId"], "outglow-dimm");
    assert_eq!(d["releaseNotes"], "fixes");
    let w = &ix[3];
    assert_eq!(w["url"], "https://x/t1/outglow-ESMLFzm_w6_TW-4.63-1.0.1.ota");
    assert!(w.get("releaseNotes").is_none());
    let s = read(&site);
    let imgs = s["images"].as_array().unwrap();
    assert_eq!(imgs.len(), 3);
    assert_eq!(imgs[0]["board"], "A");
    assert_eq!(imgs[1]["version"], "1.0.0");
    let r = &imgs[2];
    let ble = std::fs::read(t.path("a/bulb-tc32-ota.bin")).unwrap();
    assert_eq!(r["fileVersion"], "0x01000001");
    assert_eq!(r["url"], "firmware/v1.0.1/outglow-ESMLFzm_w6_Dimm-4.63-1.0.1-ota.bin");
    assert_eq!((r["size"].as_u64(), r["sha256"].as_str()), (Some(ble.len() as u64), Some(hex::<Sha256>(&ble).as_str())));
    assert_eq!((r["date"].as_str(), r["notes"].as_str()), (Some("2026-11-17"), Some("fixes")));
    let mut copied: Vec<String> = std::fs::read_dir(&assets).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().into_owned()).collect();
    copied.sort();
    assert_eq!(copied, ["outglow-ESMLFzm_w6_Dimm-4.63-1.0.1-ota.bin", "outglow-ESMLFzm_w6_Dimm-4.63-1.0.1.ota", "outglow-ESMLFzm_w6_TW-4.63-1.0.1.ota"]);
}

#[test]
fn new_files_and_their_defaults() {
    let t = Tmp::new("index-new");
    let dimm = built(&t, "a", "ESMLFzm_w6_Dimm-4.63", "x", 0x0390, 2, true);
    std::fs::create_dir_all(t.path("ota")).unwrap();
    let site = t.path("site.json");
    run_in(&t.0, &args(&[&dimm], &["--version", "2.0.0", "--site", site.to_str().unwrap()]), "d").unwrap();
    let ix = read(&t.path("ota/index.json"));
    assert_eq!(ix.as_array().unwrap().len(), 1);
    assert!(ix[0].get("modelId").is_none());
    let s = read(&site);
    assert_eq!(s["schema"], 1);
    assert!(s["images"][0].get("notes").is_none());
}

#[test]
fn what_an_index_refuses() {
    let t = Tmp::new("index-bad");
    let dimm = built(&t, "a", "ESMLFzm_w6_Dimm-4.63", "outglow-dimm", 0x0390, 1, true);
    let index = t.path("index.json");
    let ix = ["--version", "1", "--index", index.to_str().unwrap()];
    let go = |ota: &PathBuf, extra: &[&str]| {
        let mut v = ix.to_vec();
        v.extend_from_slice(extra);
        run_in(&t.0, &args(&[ota], &v), "d").map_err(|e| e.to_string())
    };
    std::fs::write(&index, "{}").unwrap();
    assert_eq!(go(&dimm, &[]), Err(format!("{}: not a list", index.display())));
    std::fs::write(&index, "nope").unwrap();
    assert_eq!(go(&dimm, &[]), Err(index.display().to_string()));
    std::fs::remove_file(&index).unwrap();
    std::fs::create_dir_all(&index).unwrap();
    assert!(go(&dimm, &[]).unwrap_err().starts_with("Is a directory"));
    std::fs::remove_dir_all(&index).unwrap();
    let gone = t.path("gone.ota");
    assert_eq!(go(&gone, &[]), Err(gone.display().to_string()));
    let junk = t.put("junk.ota", "junk");
    assert_eq!(go(&junk, &[]), Err(format!("{}: shorter than an OTA header", junk.display())));
    let empty = t.put("empty.ota", ota::File { header: ota::Header::new(1, 2, 3, "e"), sub_elements: vec![] }.pack());
    assert_eq!(go(&empty, &[]), Err(format!("{}: no upgrade image sub-element", empty.display())));
    let plain = t.put("plain.ota", ota::File::with_image(ota::Header::new(1, 2, 3, "p"), image(b"ESMLFzm_w6_Dimm\0 4.63\0")).pack());
    assert_eq!(go(&plain, &[]), Err(format!("{}: no outglow-board: tag (not an Outglow image?)", plain.display())));
    let site = t.path("site.json");
    let s = ["--site", site.to_str().unwrap()];
    let tw = built(&t, "b", "ESMLFzm_w6_TW-4.63", "outglow-tw", 0x0391, 1, false);
    let ble = t.path("b/bulb-tc32-ota.bin");
    assert_eq!(go(&tw, &s), Err(format!("{}: no Bluetooth image next to {}", ble.display(), tw.display())));
    std::fs::write(&ble, image(b"outglow-board:ESMLFzm_w6_Dimm-4.63\0")).unwrap();
    assert_eq!(go(&tw, &s), Err(format!("{} is for Some(\"ESMLFzm_w6_Dimm-4.63\"), {} for ESMLFzm_w6_TW-4.63", ble.display(), tw.display())));
    std::fs::write(&site, "{\"images\": 1}").unwrap();
    assert_eq!(go(&dimm, &s), Err("site list without images".into()));
}
