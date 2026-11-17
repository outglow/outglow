use std::collections::VecDeque;

use clap::Parser;
use rumqttc::{Outgoing, Publish, QoS, SubAck};

use super::*;

enum Step {
    Msg(Msg),
    Quiet,
    Fail(&'static str),
}

#[derive(Default)]
struct Fake {
    script: VecDeque<Step>,
    clock: Duration,
    waits: Vec<Duration>,
    sets: Vec<Value>,
    set_at: Vec<Duration>,
    slept: Vec<Duration>,
}

impl Fake {
    fn new(script: Vec<Step>) -> Fake {
        Fake { script: script.into(), ..Fake::default() }
    }
}

impl Bus for Fake {
    fn topic(&self) -> &str {
        "zigbee2mqtt/0x1"
    }

    fn next(&mut self, wait: Duration) -> Result<Option<Msg>> {
        assert!(self.waits.len() < 100_000, "the bus was read without end");
        self.waits.push(wait);
        match self.script.pop_front() {
            Some(Step::Msg(m)) => {
                self.clock += Duration::from_millis(10);
                Ok(Some(m))
            }
            Some(Step::Fail(e)) => bail!(e),
            Some(Step::Quiet) | None => {
                self.clock += wait;
                Ok(None)
            }
        }
    }

    fn set(&mut self, v: Value) -> Result<()> {
        self.sets.push(v);
        self.set_at.push(self.clock);
        Ok(())
    }

    fn clock(&self) -> Duration {
        self.clock
    }

    fn sleep(&mut self, d: Duration) {
        self.slept.push(d);
        self.clock += d;
    }
}

fn state(v: Value) -> Msg {
    Msg::State(serde_json::from_value(v).unwrap())
}

struct Tmp(PathBuf);

impl Tmp {
    fn new(name: &str) -> Tmp {
        static N: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let n = N.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let d = std::env::temp_dir().join(format!("outglow-diag-test-{}-{n}-{name}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        Tmp(d)
    }

    fn path(&self, f: &str) -> PathBuf {
        self.0.join(f)
    }
}

impl Drop for Tmp {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[derive(Parser)]
enum Cli {
    Diag(DiagArgs),
    Box(BoxArgs),
}

fn diag_args(extra: &[&str]) -> DiagArgs {
    let mut v = vec!["outglow", "diag", "0x1"];
    v.extend_from_slice(extra);
    match Cli::try_parse_from(v).unwrap() {
        Cli::Diag(a) => a,
        Cli::Box(_) => unreachable!(),
    }
}

fn clock(_: &str) -> String {
    "2026-02-03 04:05:06".into()
}

#[test]
fn the_arguments() {
    let a = diag_args(&[]);
    assert_eq!((a.conn.broker, a.conn.port, a.conn.base_topic.as_str(), a.conn.timeout), (None, 1883, "zigbee2mqtt", 30.0));
    assert!(!a.json && a.watch.is_none() && a.csv.is_none());
    assert!(Cli::try_parse_from(["outglow", "diag", "0x1", "--csv", "x"]).is_err());
    assert!(Cli::try_parse_from(["outglow", "box"]).is_err());
    assert!(Cli::try_parse_from(["outglow", "box", "0x1", "--from", "x"]).is_err());
    match Cli::try_parse_from(["outglow", "box", "--from", "x", "--broker", "b"]).unwrap() {
        Cli::Box(b) => assert_eq!((b.ieee, b.from, b.conn.broker), (None, Some(PathBuf::from("x")), Some("b".into()))),
        Cli::Diag(_) => unreachable!(),
    }
}

#[test]
fn the_cache_is_under_home() {
    let t = Tmp::new("cache");
    assert_eq!(cache_dir(Some(t.0.clone().into())).unwrap(), t.path(".cache/bulb-diag"));
    assert!(t.path(".cache/bulb-diag").is_dir());
    assert_eq!(cache_dir(None).err().unwrap().to_string(), "no $HOME");
    fs::write(t.path("file"), "").unwrap();
    let e = cache_dir(Some(t.path("file").into())).err().unwrap().to_string();
    assert_eq!(e, format!("creating {}", t.path("file/.cache/bulb-diag").display()));
}

#[test]
fn local_time_through_strftime() {
    assert_eq!(format_time(332_380_800, "%Y-%m"), "1980-07");
    assert_eq!(format_time(332_380_800, "a\0b"), "");
    assert_eq!(format_time(libc::time_t::MAX, "%Y"), "");
    assert_eq!(local_time("%Y").len(), 4);
    assert!(local_time("%Y").as_str() >= "2026");
}

#[test]
fn mqtt_events_become_messages() {
    let ok = |p| Ok(Event::Incoming(p));
    assert!(matches!(msg_of(ok(Packet::SubAck(SubAck::new(1, vec![]))), "t", "b"), Some(Msg::Subscribed)));
    let publish = |topic: &str, payload: &str| ok(Packet::Publish(Publish::new(topic, QoS::AtMostOnce, payload.as_bytes().to_vec())));
    match msg_of(publish("t", r#"{"a": 1}"#), "t", "b") {
        Some(Msg::State(s)) => assert_eq!(s.keys, ["a"]),
        _ => panic!("another message"),
    }
    assert!(msg_of(publish("t", "[1]"), "t", "b").is_none());
    assert!(msg_of(publish("u", r#"{"a": 1}"#), "t", "b").is_none());
    assert!(msg_of(Ok(Event::Outgoing(Outgoing::PingReq)), "t", "b").is_none());
    let e = ConnectionError::Io(std::io::Error::other("refused"));
    match msg_of(Err(e), "t", "host") {
        Some(Msg::Failed(m)) => assert_eq!(m, "MQTT broker host: I/O: refused"),
        _ => panic!("another message"),
    }
}

#[test]
fn what_the_channel_gives() {
    assert_eq!(received(Ok(Msg::Failed("gone".into()))).err().unwrap().to_string(), "gone");
    assert!(matches!(received(Ok(Msg::Subscribed)), Ok(Some(Msg::Subscribed))));
    assert!(received(Err(mpsc::RecvTimeoutError::Timeout)).unwrap().is_none());
    assert_eq!(received(Err(mpsc::RecvTimeoutError::Disconnected)).err().unwrap().to_string(), "MQTT connection closed");
}

#[test]
fn the_subscription_is_waited_for() {
    let mut b = Fake::new(vec![Step::Msg(state(json!({}))), Step::Msg(Msg::Subscribed)]);
    subscribed(&mut b, 5.0).unwrap();
    assert_eq!(b.waits, [Duration::from_secs(5), Duration::from_millis(4990)]);
    let mut b = Fake::new(vec![]);
    assert_eq!(subscribed(&mut b, 0.2).err().unwrap().to_string(), "no answer from the MQTT broker");
    assert_eq!(b.waits, [Duration::from_secs(1)]);
    let mut b = Fake::new(vec![Step::Fail("broken")]);
    assert_eq!(subscribed(&mut b, 1.0).err().unwrap().to_string(), "broken");
}

#[test]
fn a_refresh_takes_the_last_state_once_the_bulb_is_quiet() {
    let mut b = Fake::new(vec![
        Step::Msg(state(json!({"stale": 1}))),
        Step::Quiet,
        Step::Msg(state(json!({"uptime": 1, "radio": {"cca_busy": 3}}))),
        Step::Msg(state(json!({"uptime": 2}))),
    ]);
    let s = fetch(&mut b, 30.0).unwrap();
    assert_eq!(s.keys, ["uptime"]);
    assert_eq!(s.map["uptime"], 2);
    assert_eq!(b.sets, [json!({"diagnostics_refresh": "refresh"})]);
    assert_eq!(b.waits[..2], [Duration::ZERO, Duration::ZERO]);
    assert_eq!(b.waits.len(), 2 + 2 + 7);
    assert!(b.waits[2..].iter().all(|w| *w == Duration::from_millis(500)));
}

#[test]
fn a_refresh_flattens_the_groups() {
    let mut b = Fake::new(vec![Step::Quiet, Step::Msg(state(json!({"radio": {"cca_busy": 3}})))]);
    let s = fetch(&mut b, 30.0).unwrap();
    assert_eq!(s.keys, ["radio", "cca_busy"]);
}

#[test]
fn a_refresh_without_an_answer_fails_at_its_timeout() {
    let mut b = Fake::new(vec![]);
    let e = fetch(&mut b, 1.2).err().unwrap().to_string();
    assert_eq!(e, "no state from zigbee2mqtt/0x1: is the bulb on, and the converter installed?");
    assert_eq!(b.waits, [Duration::ZERO, Duration::from_millis(500), Duration::from_millis(500), Duration::from_millis(200)]);
    assert_eq!(b.clock, Duration::from_millis(1200));
    let mut b = Fake::new(vec![Step::Quiet, Step::Fail("lost")]);
    assert_eq!(fetch(&mut b, 1.0).err().unwrap().to_string(), "lost");
}

#[test]
fn a_refresh_ends_at_its_timeout_while_the_bulb_still_talks() {
    let mut steps = vec![Step::Quiet];
    steps.extend((0..200).map(|i| Step::Msg(state(json!({ "n": i })))));
    let mut b = Fake::new(steps);
    let s = fetch(&mut b, 1.0).unwrap();
    assert_eq!(s.map["n"], 99);
}

const BLOB: &str = include_str!("testdata/box.hex");

fn pages(blob: &[u8]) -> Vec<Step> {
    blob.chunks(BOX_PAGE)
        .enumerate()
        .map(|(n, d)| Step::Msg(state(json!({"black_box_page": {"page": n, "hex": blackbox::hex(d)}}))))
        .collect()
}

#[test]
fn the_box_is_read_page_by_page() {
    let blob = blackbox::unhex(BLOB).unwrap();
    let mut steps = vec![Step::Msg(state(json!({"old": 1}))), Step::Quiet, Step::Msg(Msg::Subscribed)];
    steps.push(Step::Msg(state(json!({"black_box_page": null}))));
    steps.push(Step::Msg(state(json!({"black_box_page": {"page": 1, "hex": "zz"}}))));
    steps.push(Step::Msg(state(json!({"black_box_page": {"hex": "00"}}))));
    steps.push(Step::Msg(state(json!({"black_box_page": {"page": 1}}))));
    steps.push(Step::Msg(state(json!({"other": 1}))));
    steps.push(Step::Msg(state(json!({"black_box_page": {"page": 1, "hex": "ff"}}))));
    steps.extend(pages(&blob));
    let mut b = Fake::new(steps);
    let (got, missing) = fetch_box(&mut b, 240.0).unwrap();
    assert!(missing.is_empty());
    assert_eq!(got.len(), blob.len());
    assert_eq!(got[..64], blob[..64]);
    assert_eq!(got[64], 0xff);
    assert_eq!(got[65..128], [0; 63]);
    assert_eq!(got[128..], blob[128..]);
    assert_eq!(b.sets, [json!({"black_box_read": "all"})]);
    assert!(b.clock < Duration::from_secs(5), "{:?}", b.clock);
}

#[test]
fn the_box_reading_stops_once_every_page_is_there() {
    let blob = blackbox::unhex(BLOB).unwrap();
    let mut steps = vec![Step::Quiet];
    steps.extend(pages(&blob));
    let n = steps.len();
    let mut b = Fake::new(steps);
    fetch_box(&mut b, 240.0).unwrap();
    assert_eq!(n, 94);
    assert_eq!(b.waits[n..], [Duration::from_millis(100)]);
    assert_eq!(b.clock, Duration::from_millis(1030));
}

#[test]
fn missing_pages_are_asked_for_again_five_at_a_time() {
    let blob = blackbox::unhex(BLOB).unwrap();
    let mut steps = vec![Step::Quiet];
    steps.extend(pages(&blob).into_iter().enumerate().filter(|(i, _)| ![3, 4, 5, 6, 7, 9].contains(i)).map(|(_, s)| s));
    let mut b = Fake::new(steps);
    let (got, missing) = fetch_box(&mut b, 45.0).unwrap();
    assert_eq!(missing, [3, 4, 5, 6, 7, 9]);
    assert_eq!(got[3 * 64..4 * 64], [0; 64]);
    assert_eq!(got.len(), blob.len());
    let again: Vec<&Value> = b.sets[1..].iter().collect();
    assert_eq!(again[..5], [&json!({"black_box_read": 3}), &json!({"black_box_read": 4}), &json!({"black_box_read": 5}), &json!({"black_box_read": 6}), &json!({"black_box_read": 7})]);
    assert_eq!(again.len(), 10);
    let ms: Vec<u128> = b.set_at.iter().map(Duration::as_millis).collect();
    assert_eq!(ms, [0, 21_070, 21_070, 21_070, 21_070, 21_070, 41_570, 41_570, 41_570, 41_570, 41_570]);
    assert_eq!(b.clock, Duration::from_millis(45_070));
}

#[test]
fn a_box_without_page_0_is_an_error() {
    let blob = blackbox::unhex(BLOB).unwrap();
    let mut steps = vec![Step::Quiet];
    steps.extend(pages(&blob).into_iter().skip(1));
    let mut b = Fake::new(steps);
    let e = fetch_box(&mut b, 30.0).err().unwrap().to_string();
    assert_eq!(e, "no page 0 of the black box: is this image 0x00020004 or later, and the converter current?");
    assert_eq!(b.sets.len(), 1);
    let mut b = Fake::new(vec![Step::Quiet, Step::Fail("lost")]);
    assert_eq!(fetch_box(&mut b, 30.0).err().unwrap().to_string(), "lost");
}

#[test]
fn page_0_that_comes_late_still_sets_the_length() {
    let blob = blackbox::unhex(BLOB).unwrap();
    let mut steps = vec![Step::Quiet];
    let mut p = pages(&blob);
    let first = p.remove(0);
    steps.extend(p);
    steps.push(first);
    let mut b = Fake::new(steps);
    let (got, missing) = fetch_box(&mut b, 0.925).unwrap();
    assert_eq!(b.clock, Duration::from_millis(930));
    assert!(missing.is_empty());
    assert_eq!(got, blob);
}

#[test]
fn a_box_of_whole_pages_stops_with_its_last_page() {
    let mut page0 = vec![0u8; 64];
    page0[20..22].copy_from_slice(&128u16.to_le_bytes());
    let page = |n: usize, d: &[u8]| Step::Msg(state(json!({"black_box_page": {"page": n, "hex": blackbox::hex(d)}})));
    let mut b = Fake::new(vec![Step::Quiet, page(0, &page0), page(1, &[7; 64])]);
    let (got, missing) = fetch_box(&mut b, 240.0).unwrap();
    assert_eq!((got.len(), got[64], missing.len()), (128, 7, 0));
    assert_eq!(b.clock, Duration::from_millis(520));
}

#[test]
fn an_empty_box_is_read_until_the_timeout() {
    let mut page0 = vec![0u8; 24];
    page0[20] = 0;
    let mut b = Fake::new(vec![Step::Quiet, Step::Msg(state(json!({"black_box_page": {"page": 0, "hex": blackbox::hex(&page0)}})))]);
    let (got, missing) = fetch_box(&mut b, 2.0).unwrap();
    assert!(got.is_empty() && missing.is_empty());
    assert!(b.clock >= Duration::from_secs(2));
}

#[test]
fn pages_are_put_together_with_the_gaps_zeroed() {
    let pages = BTreeMap::from([(0, vec![1, 2]), (2, vec![3; 70])]);
    let (blob, missing) = assemble(&pages, 150);
    assert_eq!(missing, [1]);
    assert_eq!(blob.len(), 150);
    assert_eq!(blob[..3], [1, 2, 0]);
    assert_eq!(blob[64..128], [0; 64]);
    assert_eq!(blob[128..150], [3; 22]);
    assert_eq!(assemble(&pages, 128), (blob[..128].to_vec(), vec![1]));
    assert_eq!(assemble(&pages, 129).1, vec![1]);
    assert_eq!(assemble(&BTreeMap::new(), 64).1, vec![0]);
}

const STATE: &str = include_str!("testdata/state.json");

fn bulb(n: usize) -> Vec<Step> {
    let mut v = vec![];
    for _ in 0..n {
        v.push(Step::Quiet);
        v.push(Step::Msg(state(serde_json::from_str(STATE).unwrap())));
        v.extend((0..7).map(|_| Step::Quiet));
    }
    v
}

fn header() -> String {
    let mut h: Vec<String> = ["time", "uptime", "noise_floor"].map(String::from).to_vec();
    h.extend(report::COUNTERS.map(String::from));
    py::csv_row(&h)
}

#[test]
fn watching_writes_a_row_per_refresh_until_the_bulb_is_gone() {
    let mut b = Fake::new(bulb(2));
    let mut out = vec![];
    let a = diag_args(&["--watch", "60", "--timeout", "5"]);
    let e = diag_on(&mut b, &a, Path::new("/nonexistent"), &mut out, clock).err().unwrap();
    assert!(e.to_string().starts_with("no state from"), "{e}");
    let text = String::from_utf8(out).unwrap();
    let lines: Vec<&str> = text.split("\r\n").collect();
    assert_eq!(format!("{}\r\n", lines[0]), header());
    assert_eq!(lines.len(), 4);
    assert!(lines[1].starts_with("2026-02-03 04:05:06,1176,-105,8,"), "{}", lines[1]);
    assert_eq!(lines[1], lines[2]);
    assert_eq!(b.slept, [Duration::from_secs(60), Duration::from_secs(60)]);
    let mut b = Fake::new(bulb(1));
    let a = diag_args(&["--watch=-1"]);
    let _ = diag_on(&mut b, &a, Path::new("/nonexistent"), &mut vec![], clock);
    assert_eq!(b.slept, [Duration::ZERO]);
}

#[test]
fn watching_into_a_csv_file_appends() {
    let t = Tmp::new("watch");
    let csv = t.path("diag.csv");
    let a = diag_args(&["--watch", "1", "--csv", csv.to_str().unwrap()]);
    let mut out = vec![];
    let _ = diag_on(&mut Fake::new(bulb(1)), &a, &t.0, &mut out, clock);
    let _ = diag_on(&mut Fake::new(bulb(2)), &a, &t.0, &mut out, clock);
    assert!(out.is_empty());
    let text = fs::read_to_string(&csv).unwrap();
    assert!(text.starts_with(&header()));
    assert_eq!(text.matches("2026-02-03 04:05:06,").count(), 3);
    assert_eq!(text.matches(&header()).count(), 1);
    let a = diag_args(&["--watch", "1", "--csv", t.0.to_str().unwrap()]);
    let e = diag_on(&mut Fake::new(bulb(1)), &a, &t.0, &mut out, clock).err().unwrap();
    assert_eq!(e.to_string(), format!("opening {}", t.0.display()));
}

#[test]
fn a_report_with_the_growth_since_the_last_one() {
    let t = Tmp::new("report");
    let a = diag_args(&[]);
    let mut out = vec![];
    diag_on(&mut Fake::new(bulb(1)), &a, &t.0, &mut out, clock).unwrap();
    assert_eq!(String::from_utf8(out).unwrap(), include_str!("testdata/report.first.txt"));
    let saved: Value = serde_json::from_str(&fs::read_to_string(t.path("0x1.json")).unwrap()).unwrap();
    assert_eq!(saved["_when"], "2026-02-03 04:05:06");
    assert_eq!(saved["uptime"], 1176);
    fs::write(t.path("0x1.json"), include_str!("testdata/before.json")).unwrap();
    let mut out = vec![];
    diag_on(&mut Fake::new(bulb(1)), &a, &t.0, &mut out, clock).unwrap();
    let text = String::from_utf8(out).unwrap();
    assert_eq!(text, format!("(growth since 2026-01-01 12:00:00)\n{}", include_str!("testdata/report.txt")));
    for prev in [r#"{"number_of_resets": 7}"#, "[1]", "not json"] {
        fs::write(t.path("0x1.json"), prev).unwrap();
        let mut out = vec![];
        diag_on(&mut Fake::new(bulb(1)), &a, &t.0, &mut out, clock).unwrap();
        assert_eq!(String::from_utf8(out).unwrap(), include_str!("testdata/report.first.txt"), "{prev}");
    }
}

#[test]
fn a_report_as_json_or_not_saved() {
    let a = diag_args(&["--json"]);
    let mut out = vec![];
    diag_on(&mut Fake::new(bulb(1)), &a, Path::new("/nonexistent"), &mut out, clock).unwrap();
    let v: Value = serde_json::from_slice(&out).unwrap();
    assert_eq!(v["uptime"], 1176);
    assert!(String::from_utf8(out).unwrap().ends_with("}\n"));
    let e = diag_on(&mut Fake::new(bulb(1)), &diag_args(&[]), Path::new("/nonexistent"), &mut vec![], clock).err().unwrap();
    assert_eq!(e.to_string(), "writing /nonexistent/0x1.json");
    let e = diag_on(&mut Fake::new(vec![]), &diag_args(&["--timeout", "1"]), Path::new("/nonexistent"), &mut vec![], clock);
    assert!(e.is_err());
}

#[test]
fn a_box_is_saved_reported_and_read_back() {
    let t = Tmp::new("box");
    let blob = blackbox::unhex(BLOB).unwrap();
    let (save, csv) = (t.path("box.json"), t.path("rows.csv"));
    let mut out = vec![];
    show_box(&blob, &[2], Some(&save), Some(&csv), &mut out).unwrap();
    let bx = blackbox::decode(&blob).unwrap();
    assert_eq!(String::from_utf8(out).unwrap(), format!("{}(saved {})\n", blackbox::report(&bx, &[2]), save.display()));
    let rows = fs::read_to_string(&csv).unwrap();
    assert!(rows.starts_with("minute,tx,rx,retries,fails,coord_cost,coord_measured,coord_direct,route_disc,coord_delivery,stale,noise\r\n"));
    assert_eq!(rows.matches("\r\n").count(), bx.rows.len() + 1);
    assert_eq!(load_box(&save).unwrap(), (blob.clone(), vec![2]));
    let mut out = vec![];
    show_box(&blob, &[], None, None, &mut out).unwrap();
    assert_eq!(String::from_utf8(out).unwrap(), include_str!("testdata/box.report.txt"));
}

#[test]
fn an_empty_box_has_a_csv_of_minutes() {
    let t = Tmp::new("empty");
    let csv = t.path("rows.csv");
    show_box(&[0; 24], &[], None, Some(&csv), &mut vec![]).unwrap();
    assert_eq!(fs::read_to_string(&csv).unwrap(), "minute\r\n");
    assert!(show_box(&[0; 10], &[], None, None, &mut vec![]).is_err());
    let e = show_box(&[0; 24], &[], None, Some(&t.0), &mut vec![]).err().unwrap();
    assert_eq!(e.to_string(), format!("writing {}", t.0.display()));
    let e = show_box(&[0; 24], &[], Some(&t.0), None, &mut vec![]).err().unwrap();
    assert_eq!(e.to_string(), format!("writing {}", t.0.display()));
}

#[test]
fn a_saved_box_must_hold_the_raw_bytes() {
    let t = Tmp::new("load");
    let p = t.path("b.json");
    fs::write(&p, r#"{"raw": "0102", "missing": [1, "x", 4]}"#).unwrap();
    assert_eq!(load_box(&p).unwrap(), (vec![1, 2], vec![1, 4]));
    fs::write(&p, r#"{"raw": "0102"}"#).unwrap();
    assert_eq!(load_box(&p).unwrap(), (vec![1, 2], vec![]));
    fs::write(&p, r#"{"missing": []}"#).unwrap();
    assert_eq!(load_box(&p).err().unwrap().to_string(), "no \"raw\" in the saved box");
    fs::write(&p, r#"{"raw": "0"}"#).unwrap();
    assert!(load_box(&p).is_err());
    fs::write(&p, "nope").unwrap();
    assert_eq!(load_box(&p).err().unwrap().to_string(), format!("parsing {}", p.display()));
    let gone = t.path("gone.json");
    assert_eq!(load_box(&gone).err().unwrap().to_string(), format!("reading {}", gone.display()));
}
