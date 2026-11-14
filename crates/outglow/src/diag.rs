//! Diagnostics and black box of a bulb on Outglow, read through Zigbee2MQTT
//! with the external converter z2m/outglow.mjs (docs/diagnostics.md).
//!
//! `diag` presses `diagnostics_refresh` (every counter and table read anew),
//! waits for the state and prints: the start (uptime, resets, why), the
//! network parameters, the way to the coordinator, the radio (noise floor,
//! RSSI histogram, MAC retries), Bluetooth, the timing worst cases, the
//! neighbour table and the event log. With an earlier run on record
//! (~/.cache/bulb-diag/<ieee>.json) the counters show how much they grew.
//!
//! `box` reads the black box (crates/zigbee/src/stack/blackbox.rs): its
//! pages, one Read Attributes each, about a minute; the bulb records nothing
//! new meanwhile. Saved with the raw blob to
//! ~/.cache/bulb-diag/<ieee>-box-<time>.json.

mod blackbox;
mod py;
mod report;

use std::collections::BTreeMap;
use std::fs;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use anyhow::{bail, Context, Result};
use rumqttc::{Client, Event, MqttOptions, Packet, QoS};
use serde_json::{json, Value};
use zigbee::stack::blackbox::BOX_PAGE;

#[derive(clap::Args)]
pub struct Conn {
    /// MQTT broker of Zigbee2MQTT (default: $MQTT_BROKER, else localhost)
    #[arg(long)]
    broker: Option<String>,
    #[arg(long, default_value_t = 1883)]
    port: u16,
    /// Zigbee2MQTT base topic
    #[arg(long, default_value = "zigbee2mqtt")]
    base_topic: String,
    /// Seconds to wait for the answers
    #[arg(long, default_value_t = 30.0)]
    timeout: f64,
}

#[derive(clap::Args)]
#[command(after_help = "Examples:\n  outglow diag 0xa4c1380000123456 --broker HOST [--json]\n  \
                        outglow diag 0xa4c1380000123456 --broker HOST --watch 60 --csv diag.csv   # a row a minute\n\n\
                        Reads only: the refresh is 13 + 10 Read Attributes frames.")]
pub struct DiagArgs {
    /// IEEE address of the bulb (its Zigbee2MQTT name), e.g. 0xa4c1380000123456
    ieee: String,
    #[command(flatten)]
    conn: Conn,
    /// Print the raw state instead of the report
    #[arg(long)]
    json: bool,
    /// Every this many seconds, a row of counters (to --csv, else to the terminal)
    #[arg(long, value_name = "SECONDS")]
    watch: Option<f64>,
    /// With --watch: the file the rows are appended to
    #[arg(long, value_name = "PATH", requires = "watch")]
    csv: Option<PathBuf>,
}

#[derive(clap::Args)]
#[command(after_help = "Examples:\n  outglow box 0xa4c1380000123456 --broker HOST [--csv rows.csv]\n  \
                        outglow box --from ~/.cache/bulb-diag/0xa4c1380000123456-box-20260101-120000.json\n\n\
                        Reads the pages (one Read Attributes each, about a minute); reading page 0 freezes the \
                        records until the last page or 60 s.")]
pub struct BoxArgs {
    /// IEEE address of the bulb (its Zigbee2MQTT name), e.g. 0xa4c1380000123456
    #[arg(required_unless_present = "from")]
    ieee: Option<String>,
    #[command(flatten)]
    conn: Conn,
    /// Also write the rows (one every two minutes) to this CSV file
    #[arg(long, value_name = "PATH")]
    csv: Option<PathBuf>,
    /// Where to save the decoded box and the raw blob (default: ~/.cache/bulb-diag/<ieee>-box-<time>.json)
    #[arg(long, value_name = "PATH")]
    json: Option<PathBuf>,
    /// Decode a box saved earlier (by this tool or the earlier bulb-diag.py --dump) instead of reading the bulb
    #[arg(long, value_name = "PATH", conflicts_with = "ieee")]
    from: Option<PathBuf>,
}

fn cache_dir() -> Result<PathBuf> {
    let home = std::env::var_os("HOME").context("no $HOME")?;
    let d = PathBuf::from(home).join(".cache/bulb-diag");
    fs::create_dir_all(&d).with_context(|| format!("creating {}", d.display()))?;
    Ok(d)
}

/// Local time with a `strftime` format.
fn local_time(fmt: &str) -> String {
    let t = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0) as libc::time_t;
    let fmt = std::ffi::CString::new(fmt).unwrap_or_default();
    let mut buf = [0u8; 64];
    // SAFETY: `tm` is written by localtime_r before strftime reads it; the
    // buffer and its length are ours, and strftime NUL-terminates within it.
    let n = unsafe {
        let mut tm: libc::tm = std::mem::zeroed();
        if libc::localtime_r(&t, &mut tm).is_null() {
            return String::new();
        }
        libc::strftime(buf.as_mut_ptr().cast(), buf.len(), fmt.as_ptr(), &tm)
    };
    String::from_utf8_lossy(&buf[..n]).into_owned()
}

enum Msg {
    Subscribed,
    State(report::State),
    Failed(String),
}

/// A connection to the broker, subscribed to the bulb's state.
struct Mqtt {
    client: Client,
    rx: mpsc::Receiver<Msg>,
    topic: String,
}

impl Mqtt {
    fn connect(c: &Conn, ieee: &str) -> Result<Mqtt> {
        let broker = c.broker.clone().or_else(|| std::env::var("MQTT_BROKER").ok()).unwrap_or_else(|| "localhost".into());
        let mut o = MqttOptions::new(format!("outglow-diag-{}", std::process::id()), broker.clone(), c.port);
        o.set_keep_alive(Duration::from_secs(30));
        // a state carries the tables and the event log: well over the default 10 KB
        o.set_max_packet_size(1 << 20, 1 << 20);
        let (client, mut conn) = Client::new(o, 64);
        let topic = format!("{}/{ieee}", c.base_topic);
        let (tx, rx) = mpsc::channel();
        let t = topic.clone();
        std::thread::spawn(move || {
            for ev in conn.iter() {
                let m = match ev {
                    Ok(Event::Incoming(Packet::SubAck(_))) => Msg::Subscribed,
                    Ok(Event::Incoming(Packet::Publish(p))) if p.topic == t => {
                        match serde_json::from_slice::<report::State>(&p.payload) {
                            Ok(s) => Msg::State(s),
                            _ => continue,
                        }
                    }
                    Ok(_) => continue,
                    Err(e) => {
                        let _ = tx.send(Msg::Failed(format!("MQTT broker {broker}: {e}")));
                        return;
                    }
                };
                if tx.send(m).is_err() {
                    return;
                }
            }
        });
        client.subscribe(&topic, QoS::AtMostOnce)?;
        let m = Mqtt { client, rx, topic };
        let until = Instant::now() + Duration::from_secs_f64(c.timeout.max(1.0));
        loop {
            match m.next(until.saturating_duration_since(Instant::now()))? {
                Some(Msg::Subscribed) => return Ok(m),
                Some(_) => {}
                None => bail!("no answer from the MQTT broker"),
            }
        }
    }

    /// The next message within `wait`; None when it passed.
    fn next(&self, wait: Duration) -> Result<Option<Msg>> {
        match self.rx.recv_timeout(wait) {
            Ok(Msg::Failed(e)) => bail!(e),
            Ok(m) => Ok(Some(m)),
            Err(mpsc::RecvTimeoutError::Timeout) => Ok(None),
            Err(mpsc::RecvTimeoutError::Disconnected) => bail!("MQTT connection closed"),
        }
    }

    /// Drops what came in so far.
    fn drain(&self) -> Result<()> {
        while self.next(Duration::ZERO)?.is_some() {}
        Ok(())
    }

    fn set(&self, v: Value) -> Result<()> {
        self.client.publish(format!("{}/set", self.topic), QoS::AtMostOnce, false, v.to_string())?;
        Ok(())
    }

    /// Presses the refresh and returns the state once the reads have
    /// settled (no new state for 3 s after the first that came in), flat.
    fn fetch(&self, timeout: f64) -> Result<report::State> {
        self.drain()?;
        self.set(json!({"diagnostics_refresh": "refresh"}))?;
        let asked = Instant::now();
        let timeout = Duration::from_secs_f64(timeout);
        let mut last: Option<(report::State, Instant)> = None;
        while asked.elapsed() < timeout {
            if last.as_ref().is_some_and(|(_, t)| t.elapsed() > Duration::from_secs(3)) {
                break;
            }
            if let Some(Msg::State(s)) = self.next(Duration::from_millis(500).min(timeout.saturating_sub(asked.elapsed())))? {
                last = Some((s, Instant::now()));
            }
        }
        match last {
            Some((s, _)) => Ok(report::flatten(s)),
            None => bail!("no state from {}: is the bulb on, and the converter installed?", self.topic),
        }
    }

    /// Reads every page of the black box; returns the blob and the pages
    /// that did not come.
    fn fetch_box(&self, timeout: f64) -> Result<(Vec<u8>, Vec<usize>)> {
        self.drain()?;
        let mut pages: BTreeMap<usize, Vec<u8>> = BTreeMap::new();
        let mut last = Instant::now();
        self.set(json!({"black_box_read": "all"}))?;
        let mut total: Option<usize> = None;
        let t0 = Instant::now();
        let timeout = Duration::from_secs_f64(timeout);
        let mut tick = Instant::now();
        while t0.elapsed() < timeout {
            if let Some(Msg::State(s)) = self.next(Duration::from_millis(100))? {
                if let Some(p) = s.map.get("black_box_page").filter(|p| py::truthy(Some(p))) {
                    let n = p.get("page").and_then(Value::as_u64).map(|n| n as usize);
                    let data = p.get("hex").and_then(Value::as_str).and_then(|h| blackbox::unhex(h).ok());
                    if let (Some(n), Some(data)) = (n, data) {
                        if !pages.contains_key(&n) {
                            pages.insert(n, data);
                            last = Instant::now();
                        }
                    }
                }
            }
            if tick.elapsed() < Duration::from_millis(500) {
                continue;
            }
            tick = Instant::now();
            if total.is_none() {
                total = pages.get(&0).and_then(|p| blackbox::total_len(p));
            }
            if let Some(t) = total {
                if t > 0 && pages.len() * BOX_PAGE >= t {
                    break;
                }
            }
            if !pages.is_empty() && last.elapsed() > Duration::from_secs(20) {
                // a page lost: ask for the missing ones again, a few at a time
                let want = (0..total.unwrap_or(0).div_ceil(BOX_PAGE)).filter(|p| !pages.contains_key(p));
                for p in want.take(5) {
                    self.set(json!({"black_box_read": p}))?;
                }
                last = Instant::now();
            }
            eprint!("\rblack box: {} pages", pages.len());
        }
        eprintln!();
        let Some(total) = total.or_else(|| pages.get(&0).and_then(|p| blackbox::total_len(p))) else {
            bail!("no page 0 of the black box: is this image 0x00020004 or later, and the converter current?");
        };
        let n = total.div_ceil(BOX_PAGE);
        let mut blob = Vec::with_capacity(n * BOX_PAGE);
        let mut missing = Vec::new();
        for p in 0..n {
            match pages.get(&p) {
                Some(d) => {
                    let mut d = d.clone();
                    d.resize(BOX_PAGE, 0);
                    blob.extend(d);
                }
                None => {
                    missing.push(p);
                    blob.extend([0u8; BOX_PAGE]);
                }
            }
        }
        blob.truncate(total);
        Ok((blob, missing))
    }
}

fn write_csv(path: &Path, header: &[String], rows: impl Iterator<Item = Vec<String>>) -> Result<()> {
    let mut out = py::csv_row(header);
    for r in rows {
        out += &py::csv_row(&r);
    }
    fs::write(path, out).with_context(|| format!("writing {}", path.display()))
}

pub fn run_diag(a: DiagArgs) -> Result<()> {
    let cache = cache_dir()?;
    let m = Mqtt::connect(&a.conn, &a.ieee)?;
    if let Some(every) = a.watch {
        let mut out: Box<dyn std::io::Write> = match &a.csv {
            Some(p) => Box::new(
                fs::OpenOptions::new().create(true).append(true).open(p).with_context(|| format!("opening {}", p.display()))?,
            ),
            None => Box::new(std::io::stdout()),
        };
        let empty = match &a.csv {
            Some(p) => fs::metadata(p)?.len() == 0,
            None => true,
        };
        if empty {
            let mut h: Vec<String> = ["time", "uptime", "noise_floor"].map(String::from).to_vec();
            h.extend(report::COUNTERS.map(String::from));
            out.write_all(py::csv_row(&h).as_bytes())?;
        }
        loop {
            let s = m.fetch(a.conn.timeout)?;
            let mut row = vec![py::csv_quote(&local_time("%Y-%m-%d %H:%M:%S"))];
            row.extend(["uptime", "noise_floor"].iter().chain(report::COUNTERS.iter()).map(|k| py::csv_cell(s.map.get(*k))));
            out.write_all(py::csv_row(&row).as_bytes())?;
            out.flush()?;
            std::thread::sleep(Duration::from_secs_f64(every.max(0.0)));
        }
    }
    let s = m.fetch(a.conn.timeout)?;
    let _ = m.client.disconnect();
    if a.json {
        println!("{}", serde_json::to_string_pretty(&s)?);
        return Ok(());
    }
    let file = cache.join(format!("{}.json", a.ieee));
    let mut before = None;
    if let Ok(text) = fs::read_to_string(&file) {
        if let Ok(Value::Object(prev)) = serde_json::from_str::<Value>(&text) {
            // only the same start of the bulb: the counters do not survive a power cut
            if py::eq(prev.get("number_of_resets"), s.map.get("number_of_resets")) {
                println!("(growth since {})", py::get(&prev, "_when"));
                before = Some(prev);
            }
        }
    }
    let mut store = s.clone();
    store.insert("_when".into(), Value::String(local_time("%Y-%m-%d %H:%M:%S")));
    print!("{}", report::report(&s.map, before.as_ref()));
    fs::write(&file, serde_json::to_string(&store)?).with_context(|| format!("writing {}", file.display()))?;
    Ok(())
}

pub fn run_box(a: BoxArgs) -> Result<()> {
    let (blob, missing, save) = match (&a.from, &a.ieee) {
        (Some(from), _) => {
            let text = fs::read_to_string(from).with_context(|| format!("reading {}", from.display()))?;
            let v: Value = serde_json::from_str(&text).with_context(|| format!("parsing {}", from.display()))?;
            let raw = v.get("raw").and_then(Value::as_str).context("no \"raw\" in the saved box")?;
            let missing = v
                .get("missing")
                .and_then(Value::as_array)
                .map(|m| m.iter().filter_map(Value::as_u64).map(|p| p as usize).collect())
                .unwrap_or_default();
            (blackbox::unhex(raw)?, missing, a.json.clone())
        }
        (None, Some(ieee)) => {
            let m = Mqtt::connect(&a.conn, ieee)?;
            let (blob, missing) = m.fetch_box(a.conn.timeout.max(240.0))?;
            let _ = m.client.disconnect();
            let save = match &a.json {
                Some(p) => p.clone(),
                None => cache_dir()?.join(format!("{ieee}-box-{}.json", local_time("%Y%m%d-%H%M%S"))),
            };
            (blob, missing, Some(save))
        }
        (None, None) => bail!("which bulb?"),
    };
    let bx = blackbox::decode(&blob)?;
    if let Some(p) = &save {
        let mut v = serde_json::to_value(&bx)?;
        v["missing"] = json!(missing);
        v["raw"] = Value::String(blackbox::hex(&blob));
        fs::write(p, serde_json::to_string(&v)?).with_context(|| format!("writing {}", p.display()))?;
    }
    print!("{}", blackbox::report(&bx, &missing));
    if let Some(p) = &a.csv {
        let header: Vec<String> = if bx.rows.is_empty() {
            vec!["minute".into()]
        } else {
            blackbox::Row::FIELDS.map(String::from).to_vec()
        };
        write_csv(p, &header, bx.rows.iter().map(blackbox::Row::cells))?;
    }
    if let Some(p) = &save {
        println!("(saved {})", p.display());
    }
    Ok(())
}
