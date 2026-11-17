use std::cell::{Cell, RefCell};

use clap::Parser;
use outglow_image::telink;

use super::*;

const MAC: [u8; 6] = [0xa4, 0xc1, 0x38, 0x12, 0x34, 0x56];
const DIMM: u16 = 0x20a7;

#[derive(Default)]
struct Bulb {
    awox: Option<(u16, [u8; 6])>,
    codes: Vec<u8>,
    info: Vec<(u16, Vec<u8>)>,
    idle: Vec<u8>,
    hang: bool,
    trace: RefCell<Vec<String>>,
    phase: Cell<u8>,
}

impl Bulb {
    fn dimm() -> Bulb {
        Bulb {
            awox: Some((DIMM, MAC)),
            codes: CODES.to_vec(),
            info: vec![(0x2a27, b"4.63\0".to_vec())],
            idle: vec![0; 9],
            ..Bulb::default()
        }
    }

    fn trace(&self) -> Vec<String> {
        self.trace.borrow().clone()
    }
}

impl Device for Bulb {
    fn awox(&self) -> Option<(u16, [u8; 6])> {
        self.awox
    }

    fn has(&self, code: u8) -> bool {
        self.codes.contains(&code)
    }

    async fn read_info(&self, short: u16) -> Result<Option<Vec<u8>>> {
        Ok(self.info.iter().find(|(s, _)| *s == short).map(|(_, v)| v.clone()))
    }

    async fn read(&self, code: u8) -> Result<Vec<u8>> {
        if self.hang {
            std::future::pending::<()>().await;
        }
        self.trace.borrow_mut().push(format!("r {code:02x}"));
        Ok(match (code, self.phase.get()) {
            (p::PAIR, _) => vec![0x0d, 0x11, 0x12, 0x13, 0x14, 0x15, 0x16, 0x17, 0x18],
            (_, 0) => self.idle.clone(),
            (_, 1) => vec![2, 0, 0xff, 0xff, 0xff, 0xff, 0, 0, 0],
            (_, 2) => vec![3, 0, 0xff, 0xff, 0xff, 0xff, 0, 0, 0],
            _ => vec![4, 0, 0, 0, 0, 0, 0, 100, 0],
        })
    }

    async fn write(&self, code: u8, data: &[u8]) -> Result<()> {
        if self.hang {
            std::future::pending::<()>().await;
        }
        if !self.has(code) {
            bail!("no characteristic ...19{code:02x}");
        }
        self.trace.borrow_mut().push(format!("w {code:02x} {}", hex(data)));
        if code == p::OTA {
            self.phase.set(match (self.phase.get(), data.len()) {
                (0, _) => 1,
                (_, 4) => 3,
                _ => 2,
            });
        }
        Ok(())
    }

    async fn sleep(&self, d: Duration) {
        self.trace.borrow_mut().push(format!("s {}", d.as_millis()));
    }
}

fn image(text: &[u8]) -> Vec<u8> {
    let mut body = vec![0u8; 0x400];
    body[8..12].copy_from_slice(b"KNLT");
    body[0x100..0x100 + text.len()].copy_from_slice(text);
    telink::finalize(&body)
}

fn quiet(_: &str) {}

fn stock_image() -> Vec<u8> {
    image(b"ESMLFzm_w6_Dimm\0 v3.0.2 4.63\0")
}

#[derive(Parser)]
struct Cli {
    #[command(flatten)]
    a: PushArgs,
}

struct Tmp(std::path::PathBuf);

impl Tmp {
    fn with(name: &str, data: &[u8]) -> Tmp {
        static N: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let n = N.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let dir = std::env::temp_dir().join(format!("outglow-ble-test-{}-{n}-{name}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("image.bin");
        std::fs::write(&p, data).unwrap();
        Tmp(p)
    }

    fn arg(&self) -> &str {
        self.0.to_str().unwrap()
    }
}

impl Drop for Tmp {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(self.0.parent().unwrap());
    }
}

fn args(image: &str, extra: &[&str]) -> PushArgs {
    let mut v = vec!["push", "a4:c1:38:12:34:56", image];
    v.extend_from_slice(extra);
    Cli::try_parse_from(v).unwrap().a
}

fn prepared(name: &str, img: &[u8], extra: &[&str]) -> (Result<Push>, Vec<String>) {
    let t = Tmp::with(name, img);
    let mut log = vec![];
    let r = prepare(args(t.arg(), extra), &mut |s: &str| log.push(s.to_string()));
    let log = log.into_iter().map(|l| l.replace(t.arg(), "IMG")).collect();
    (r, log)
}

async fn pushed(b: &Bulb, extra: &[&str]) -> (Result<()>, Vec<String>) {
    let (s, _) = prepared("push", &stock_image(), extra);
    let s = s.unwrap();
    let mut log = vec![];
    let mut rnd = || Ok([1, 2, 3, 4, 5, 6, 7, 8]);
    let r = push(b, &s, &mut rnd, &mut |l: &str| log.push(l.to_string())).await;
    (r, log)
}

#[test]
fn uuids_and_codes() {
    assert_eq!(sig_uuid(0x2a27).to_string(), "00002a27-0000-1000-8000-00805f9b34fb");
    for c in CODES {
        assert_eq!(telink_code(p::telink_uuid(c)), Some(c));
    }
    assert_eq!(telink_code(p::telink_uuid(0x16)), None);
    assert_eq!(telink_code(sig_uuid(0x2a27)), None);
    assert!(without_response(p::OTA, true));
    assert!(!without_response(p::OTA, false));
    assert!(!without_response(p::PAIR, true));
    assert_eq!(revision_text(b"\0 4.63 \0\0"), "4.63");
    assert_eq!(revision_text(b"4. 63"), "4. 63");
}

fn md(adv: &[u8]) -> HashMap<u16, Vec<u8>> {
    HashMap::from([(AWOX_VENDOR, adv.to_vec()), (0x004c, vec![1, 2, 3, 4, 5, 6])])
}

fn seen(id: &str, rssi: Option<i16>, product: Option<u16>) -> Seen {
    let m = match product {
        Some(p) => md(&[p as u8, (p >> 8) as u8, 0x56, 0x34, 0x12, 0x38]),
        None => HashMap::from([(0x004c, vec![0xa7, 0x20, 0x56, 0x34, 0x12, 0x38])]),
    };
    Seen::new(id.into(), "AA:BB:CC:DD:EE:FF".into(), Some("mesh".into()), rssi, &m)
}

#[test]
fn what_an_advertisement_says() {
    let s = seen("ID-1", Some(-50), Some(DIMM));
    assert_eq!(s.awox, Some((DIMM, MAC)));
    assert_eq!(s.device_line(), "device: ID-1 mesh 'mesh' product 0x20a7 MAC a4:c1:38:12:34:56");
    let o = seen("ID-2", None, None);
    assert_eq!(o.awox, None);
    assert_eq!(o.device_line(), "device: ID-2 (no AwoX data in the advertisement)");
    let short = Seen::new("x".into(), "y".into(), None, None, &md(&[0xa7, 0x20]));
    assert_eq!(short.awox, None);
    let unnamed = Seen { name: None, ..s.clone() };
    assert_eq!(unnamed.device_line(), "device: ID-1 mesh '' product 0x20a7 MAC a4:c1:38:12:34:56");
}

#[test]
fn a_target_is_an_id_an_address_or_a_mac() {
    let s = seen("Id-1", None, Some(DIMM));
    assert!(s.is("ID-1"));
    assert!(s.is("aa:bb:cc:dd:ee:ff"));
    assert!(s.is("A4:C1:38:12:34:56"));
    assert!(!s.is("a4:c1:38:12:34:57"));
    assert!(!s.is("id-2"));
    let zero = Seen { address: "00:00:00:00:00:00".into(), ..s.clone() };
    assert!(!zero.is("00:00:00:00:00:00"));
    let plain = seen("Id-1", None, None);
    assert!(!plain.is("a4:c1:38:12:34:56"));
}

#[test]
fn the_scan_lists_awox_bulbs_strongest_first() {
    let rows = vec![
        seen("weak", Some(-90), Some(DIMM)),
        seen("other", Some(-10), None),
        seen("unknown", None, Some(0x1234)),
        seen("strong", Some(-40), Some(0x20a6)),
    ];
    assert_eq!(
        scan_report(7, rows),
        "AwoX devices (vendor 0x0160): 3 of 7 visible\n\
         \x20 strong  rssi  -40  mesh 'mesh'  product 0x20a6 (ESMLFzm_w6_TW)  MAC a4:c1:38:12:34:56\n\
         \x20 weak  rssi  -90  mesh 'mesh'  product 0x20a7 (ESMLFzm_w6_Dimm)  MAC a4:c1:38:12:34:56\n\
         \x20 unknown  rssi    ?  mesh 'mesh'  product 0x1234 (other product)  MAC a4:c1:38:12:34:56\n"
    );
    let unnamed = Seen { name: None, ..seen("u", Some(i16::MIN + 1), Some(DIMM)) };
    let r = scan_report(1, vec![seen("none", None, Some(DIMM)), unnamed]);
    assert!(r.ends_with("  u  rssi -32767  mesh ''  product 0x20a7 (ESMLFzm_w6_Dimm)  MAC a4:c1:38:12:34:56\n  none  rssi    ?  mesh 'mesh'  product 0x20a7 (ESMLFzm_w6_Dimm)  MAC a4:c1:38:12:34:56\n"), "{r}");
}

#[tokio::test]
async fn writes_and_reads_are_counted_and_paced() {
    let b = Bulb::dimm();
    let mut t = Counted { d: &b, delay: Duration::from_millis(7), writes: 0, reads: 0 };
    t.write(p::OTA, &[1, 2]).await.unwrap();
    assert_eq!(t.read(p::OTA).await.unwrap(), vec![2, 0, 0xff, 0xff, 0xff, 0xff, 0, 0, 0]);
    t.sleep(42).await;
    assert_eq!((t.writes, t.reads), (1, 1));
    assert_eq!(b.trace(), ["w 15 0102", "s 7", "r 15", "s 42"]);
    let mut t = Counted { d: &b, delay: Duration::ZERO, writes: 0, reads: 0 };
    t.write(p::PAIR, &[3]).await.unwrap();
    assert_eq!(b.trace().last().unwrap(), "w 14 03");
    let none = Bulb { codes: vec![], ..Bulb::dimm() };
    let mut t = Counted { d: &none, delay: Duration::from_millis(7), writes: 0, reads: 0 };
    assert_eq!(t.write(p::OTA, &[1]).await.unwrap_err().to_string(), "no characteristic ...1915");
    assert_eq!((t.writes, none.trace().len()), (0, 0));
}

#[tokio::test(start_paused = true)]
async fn a_bulb_that_does_not_answer_times_out() {
    let b = Bulb { hang: true, ..Bulb::dimm() };
    let mut t = Counted { d: &b, delay: Duration::ZERO, writes: 0, reads: 0 };
    let t0 = tokio::time::Instant::now();
    assert_eq!(t.write(p::OTA, &[1]).await.unwrap_err().to_string(), "write: timeout");
    assert_eq!(t.read(p::OTA).await.unwrap_err().to_string(), "read: timeout");
    assert_eq!(t0.elapsed(), 2 * IO_TIMEOUT);
    assert_eq!(IO_TIMEOUT, Duration::from_secs(30));
}

async fn probed(b: &Bulb) -> Vec<String> {
    let mut log = vec![];
    probe(b, &mut |l: &str| log.push(l.to_string())).await.unwrap();
    log
}

#[tokio::test]
async fn a_probe_reads_device_information_and_the_ota_status() {
    let mut b = Bulb::dimm();
    b.info = vec![(0x2a29, b"AwoX".to_vec()), (0x2a24, b"ESMLFzm_w6_Dimm".to_vec())];
    assert_eq!(
        probed(&b).await,
        [
            "  model      \"ESMLFzm_w6_Dimm\"",
            "  manufacturer \"AwoX\"",
            "  OTA status (...1915): 000000000000000000  (0000.. = idle; 9 B = original firmware layout)",
            "  its OTA handler takes blocks up to 240 B: push --mtu 247 would use them, if the OS negotiates that ATT_MTU",
            "probe: reads only, nothing written",
        ]
    );
    b.idle = vec![0; 20];
    assert!(probed(&b).await.contains(&"  its OTA handler takes blocks up to 57 B: push --mtu 64 would use them, if the OS negotiates that ATT_MTU".to_string()));
}

#[tokio::test]
async fn a_probe_of_something_else_warns() {
    let b = Bulb { codes: vec![p::OTA], ..Bulb::dimm() };
    assert_eq!(
        probed(&b).await,
        [
            "  hardware   \"4.63\\0\"",
            "WARNING: missing characteristics [\"...1914\"]: this does not look like the Telink mesh profile",
            "probe: reads only, nothing written",
        ]
    );
    let b = Bulb { codes: vec![p::PAIR], ..Bulb::dimm() };
    assert!(probed(&b).await[1].starts_with("WARNING: missing characteristics [\"...1915\"]"));
    assert!(b.trace().is_empty());
}

#[test]
fn an_image_is_checked_before_connecting() {
    let (r, log) = prepared("ok", &stock_image(), &[]);
    let s = r.unwrap();
    assert_eq!(log, ["image: IMG 1028 B, ESMLFzm_w6_Dimm hardware revision 4.63 (product 0x20a7), KNLT+CRC ok, ORIGINAL FIRMWARE (canary)"]);
    assert_eq!((s.expected, s.password.clone(), s.img.len()), (DIMM, None, 1028));
    let (r, log) = prepared("og", &image(b"outglow-board:ESMLFzm_w6_TW-4.63\0"), &["--allow-outglow", "--product", "0x1234"]);
    assert_eq!(r.unwrap().expected, 0x1234);
    assert!(log[0].ends_with("(product 0x20a6), KNLT+CRC ok, OUTGLOW"), "{log:?}");
    let pw = "00112233445566778899aabbccddeeff";
    let (r, _) = prepared("pw", &stock_image(), &["--mesh-password", pw]);
    assert_eq!(hex(&r.unwrap().password.unwrap()), pw);
}

#[test]
fn a_bad_image_or_password_is_refused() {
    let (r, log) = prepared("og2", &image(b"outglow-board:ESMLFzm_w6_TW-4.63\0"), &[]);
    assert!(r.err().unwrap().to_string().contains("--allow-outglow"));
    assert!(log.is_empty());
    let (r, _) = prepared("short", &stock_image(), &["--mesh-password", "0011"]);
    assert_eq!(r.err().unwrap().to_string(), "--mesh-password must be 16 bytes in hex");
    let (r, _) = prepared("odd", &stock_image(), &["--mesh-password", "001"]);
    assert_eq!(r.err().unwrap().to_string(), "odd number of hex digits");
    let e = prepare(args("/nonexistent/outglow.bin", &[]), &mut quiet).err().unwrap();
    assert_eq!(e.to_string(), "/nonexistent/outglow.bin");
}

#[tokio::test]
async fn the_bulb_must_be_the_images_product() {
    let b = Bulb { awox: Some((0x20a6, MAC)), ..Bulb::dimm() };
    let (r, log) = pushed(&b, &["--confirm-write"]).await;
    assert_eq!(r.unwrap_err().to_string(), "device product 0x20a6 != 0x20a7 required by the image. --product only deliberately.");
    assert!(log.is_empty());
    let b = Bulb { awox: None, ..Bulb::dimm() };
    let (r, _) = pushed(&b, &["--confirm-write"]).await;
    assert_eq!(r.unwrap_err().to_string(), "device product unknown != 0x20a7 required by the image. --product only deliberately.");
}

#[tokio::test]
async fn the_bulb_must_be_the_images_hardware_revision() {
    let b = Bulb { info: vec![(0x2a27, b"4.60".to_vec())], ..Bulb::dimm() };
    let (r, log) = pushed(&b, &["--confirm-write"]).await;
    assert_eq!(
        r.unwrap_err().to_string(),
        "the bulb is hardware revision Some(\"4.60\"), the image is for \"4.63\": its LEDs may be on other pins. --any-revision only deliberately."
    );
    assert_eq!(log, ["bulb hardware revision Some(\"4.60\"), image for \"4.63\""]);
    let b = Bulb { info: vec![], ..Bulb::dimm() };
    let (r, _) = pushed(&b, &["--confirm-write"]).await;
    assert!(r.unwrap_err().to_string().starts_with("the bulb is hardware revision None"));
    let (r, _) = pushed(&b, &["--confirm-write", "--any-revision"]).await;
    r.unwrap();
}

#[tokio::test]
async fn nothing_is_written_without_the_ota_channel_or_confirmation() {
    let b = Bulb { codes: vec![p::PAIR], ..Bulb::dimm() };
    let (r, _) = pushed(&b, &["--confirm-write"]).await;
    assert_eq!(r.unwrap_err().to_string(), "no OTA characteristic ...1915");
    let b = Bulb::dimm();
    let (r, log) = pushed(&b, &[]).await;
    assert_eq!(r.unwrap_err().to_string(), "refused: no --confirm-write (this writes the bulb's flash and restarts it); all checks passed");
    assert_eq!(log.len(), 1);
    assert!(b.trace().is_empty());
}

#[tokio::test]
async fn a_classic_push() {
    let b = Bulb::dimm();
    let (r, log) = pushed(&b, &["--confirm-write"]).await;
    r.unwrap();
    assert_eq!(log[0], "bulb hardware revision Some(\"4.63\"), image for \"4.63\"");
    assert_eq!(log[1], "classic transfer: ATT_MTU 23, 16 B blocks");
    assert_eq!(log[2], "erasing: 2 4K sectors");
    assert_eq!(log[log.len() - 3], "OTA finished successfully (status 0)");
    assert_eq!(log[log.len() - 2], "final status 040000000000006400; 67 writes, 10 reads, 0 s");
    assert_eq!(log[log.len() - 1], "the bulb should now restart itself (the AwoX bootloader installs the image).");
    let t = b.trace();
    assert_eq!(t[0], format!("w 15 000002{}", "00".repeat(17)));
    assert_eq!(t[1], "s 5000");
    assert_eq!(t.iter().filter(|l| l.starts_with("w 15 ") && l.len() == 5 + 40).count(), 66);
    assert!(!t.iter().any(|l| l.starts_with("w 14")));
}

#[tokio::test]
async fn a_push_with_a_login_and_a_pause_after_each_write() {
    let b = Bulb::dimm();
    let (r, log) = pushed(&b, &["--confirm-write", "--login", "--delay-ms", "2.5", "--max-resends", "1"]).await;
    r.unwrap();
    let t = b.trace();
    let pw = p::secure_password(&MAC);
    let login = hex(&p::login_packet(b"unpaired", &pw, &[1, 2, 3, 4, 5, 6, 7, 8]));
    assert_eq!(t[0], format!("w 14 {login}"));
    assert_eq!(t[1], "s 2");
    assert_eq!(t[2], "r 14");
    assert!(log.contains(&"logged in (response 0d1112131415161718)".to_string()));
    let b = Bulb::dimm();
    let (r, _) = pushed(&b, &["--confirm-write", "--login", "--mesh-name", "n", "--mesh-password", "000102030405060708090a0b0c0d0e0f"]).await;
    r.unwrap();
    let pw: Vec<u8> = (0..16).collect();
    assert_eq!(b.trace()[0], format!("w 14 {}", hex(&p::login_packet(b"n", &pw, &[1, 2, 3, 4, 5, 6, 7, 8]))));
}

#[tokio::test]
async fn a_push_stops_when_the_random_numbers_fail() {
    let b = Bulb::dimm();
    let (s, _) = prepared("rnd", &stock_image(), &["--confirm-write", "--login"]);
    let mut rnd = || -> Result<[u8; 8]> { bail!("random: none") };
    let e = push(&b, &s.unwrap(), &mut rnd, &mut quiet).await.unwrap_err();
    assert_eq!(e.to_string(), "random: none");
    assert!(b.trace().is_empty());
}

async fn negotiated(b: &Bulb, mtu: usize, chunk: Option<usize>) -> (Result<(usize, usize)>, Vec<String>) {
    let mut log = vec![];
    let r = negotiate(b, mtu, chunk, &mut |l: &str| log.push(l.to_string())).await;
    (r, log)
}

#[tokio::test]
async fn the_block_follows_the_mtu_and_the_handler() {
    let b = Bulb::dimm();
    let (r, log) = negotiated(&b, 23, None).await;
    assert_eq!(r.unwrap(), (23, 16));
    assert_eq!(log, ["classic transfer: ATT_MTU 23, 16 B blocks"]);
    assert_eq!(negotiate(&b, 10, None, &mut quiet).await.unwrap(), (23, 16));
    assert!(b.trace().is_empty());
    let (r, log) = negotiated(&b, 247, None).await;
    assert_eq!(r.unwrap(), (247, 240));
    assert_eq!(
        log,
        [
            "OTA status before the transfer: 000000000000000000 (9 B -> original firmware handler, block limit 240 B)",
            "ATT_MTU 247 (as given: btleplug does not report the negotiated one); block 240 B (ATT_MTU 247 - 7)",
        ]
    );
    let o = Bulb { idle: vec![0; 20], ..Bulb::dimm() };
    let (r, log) = negotiated(&o, 247, None).await;
    assert_eq!(r.unwrap(), (247, 57));
    assert_eq!(log[1], "ATT_MTU 247 (as given: btleplug does not report the negotiated one); block 57 B (Outglow handler limit 57 B)");
    let (r, log) = negotiated(&b, 10, Some(16)).await;
    assert_eq!(r.unwrap(), (23, 16));
    assert_eq!(log[1], "ATT_MTU 23 (as given: btleplug does not report the negotiated one); block 16 B (requested)");
    let (r, _) = negotiated(&b, 23, Some(17)).await;
    assert_eq!(r.unwrap_err().to_string(), "chunk 17 B outside 16..16 (ATT_MTU 23 - 7)");
}

#[tokio::test]
async fn a_busy_ota_state_is_warned_of() {
    for (state, warned) in [(0u8, false), (1, true), (3, true), (4, false), (5, true)] {
        let b = Bulb { idle: vec![state, 0, 0, 0, 0, 0, 0, 0, 0], ..Bulb::dimm() };
        let (_, log) = negotiated(&b, 30, None).await;
        let w = format!("WARNING: OTA state {state} before the transfer (0 = idle)");
        assert_eq!(log.contains(&w), warned, "{state}: {log:?}");
        assert_eq!(log.len(), 2 + warned as usize);
    }
    let b = Bulb { idle: vec![], ..Bulb::dimm() };
    let (r, log) = negotiated(&b, 30, None).await;
    assert_eq!(r.unwrap(), (30, 16));
    assert_eq!(log.len(), 2);
}
