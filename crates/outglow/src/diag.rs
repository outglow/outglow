mod blackbox;
pub mod mqtt;
mod py;
mod report;

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::time::Duration;

use anyhow::{bail, Context, Result};
use rumqttc::{ConnectionError, Event, Packet};
use serde_json::{json, Value};
use zigbee::stack::blackbox::BOX_PAGE;

#[derive(clap::Args)]
pub struct Conn {
    #[doc = " MQTT broker of Zigbee2MQTT (default: $MQTT_BROKER, else localhost)"]
    #[arg(long)]
    broker: Option<String>,
    #[arg(long, default_value_t = 1883)]
    port: u16,
    #[doc = " Zigbee2MQTT base topic"]
    #[arg(long, default_value = "zigbee2mqtt")]
    base_topic: String,
    #[doc = " Seconds to wait for the answers"]
    #[arg(long, default_value_t = 30.0)]
    timeout: f64,
}

#[derive(clap::Args)]
#[command(after_help = "Examples:\n  outglow diag 0xa4c1380000123456 --broker HOST [--json]\n  \
                        outglow diag 0xa4c1380000123456 --broker HOST --watch 60 --csv diag.csv   # a row a minute\n\n\
                        Reads only: the refresh is 13 + 10 Read Attributes frames.")]
pub struct DiagArgs {
    #[doc = " IEEE address of the bulb (its Zigbee2MQTT name), e.g. 0xa4c1380000123456"]
    ieee: String,
    #[command(flatten)]
    conn: Conn,
    #[doc = " Print the raw state instead of the report"]
    #[arg(long)]
    json: bool,
    #[doc = " Every this many seconds, a row of counters (to --csv, else to the terminal)"]
    #[arg(long, value_name = "SECONDS")]
    watch: Option<f64>,
    #[doc = " With --watch: the file the rows are appended to"]
    #[arg(long, value_name = "PATH", requires = "watch")]
    csv: Option<PathBuf>,
}

#[derive(clap::Args)]
#[command(after_help = "Examples:\n  outglow box 0xa4c1380000123456 --broker HOST [--csv rows.csv]\n  \
                        outglow box --from ~/.cache/bulb-diag/0xa4c1380000123456-box-20260101-120000.json\n\n\
                        Reads the pages (one Read Attributes each, about a minute); reading page 0 freezes the \
                        records until the last page or 60 s.")]
pub struct BoxArgs {
    #[doc = " IEEE address of the bulb (its Zigbee2MQTT name), e.g. 0xa4c1380000123456"]
    #[arg(required_unless_present = "from")]
    ieee: Option<String>,
    #[command(flatten)]
    conn: Conn,
    #[doc = " Also write the rows (one every two minutes) to this CSV file"]
    #[arg(long, value_name = "PATH")]
    csv: Option<PathBuf>,
    #[doc = " Where to save the decoded box and the raw blob (default: ~/.cache/bulb-diag/<ieee>-box-<time>.json)"]
    #[arg(long, value_name = "PATH")]
    json: Option<PathBuf>,
    #[doc = " Decode a box saved earlier (by this tool or the earlier bulb-diag.py --dump) instead of reading the bulb"]
    #[arg(long, value_name = "PATH", conflicts_with = "ieee")]
    from: Option<PathBuf>,
}

pub fn cache_dir(home: Option<std::ffi::OsString>) -> Result<PathBuf> {
    let d = PathBuf::from(home.context("no $HOME")?).join(".cache/bulb-diag");
    fs::create_dir_all(&d).with_context(|| format!("creating {}", d.display()))?;
    Ok(d)
}

pub fn local_time(fmt: &str) -> String {
    let t = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    format_time(t as libc::time_t, fmt)
}

fn format_time(t: libc::time_t, fmt: &str) -> String {
    let fmt = std::ffi::CString::new(fmt).unwrap_or_default();
    let mut buf = [0u8; 64];
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    let known = unsafe { !libc::localtime_r(&t, &mut tm).is_null() };
    let n = if known { unsafe { libc::strftime(buf.as_mut_ptr().cast(), buf.len(), fmt.as_ptr(), &tm) } } else { 0 };
    String::from_utf8_lossy(&buf[..n]).into_owned()
}

pub enum Msg {
    Subscribed,
    State(report::State),
    Failed(String),
}

pub fn msg_of(ev: Result<Event, ConnectionError>, topic: &str, broker: &str) -> Option<Msg> {
    match ev {
        Ok(Event::Incoming(Packet::SubAck(_))) => Some(Msg::Subscribed),
        Ok(Event::Incoming(Packet::Publish(p))) if p.topic == topic => serde_json::from_slice(&p.payload).ok().map(Msg::State),
        Ok(_) => None,
        Err(e) => Some(Msg::Failed(format!("MQTT broker {broker}: {e}"))),
    }
}

pub fn received(r: Result<Msg, mpsc::RecvTimeoutError>) -> Result<Option<Msg>> {
    match r {
        Ok(Msg::Failed(e)) => bail!(e),
        Ok(m) => Ok(Some(m)),
        Err(mpsc::RecvTimeoutError::Timeout) => Ok(None),
        Err(mpsc::RecvTimeoutError::Disconnected) => bail!("MQTT connection closed"),
    }
}

pub trait Bus {
    fn topic(&self) -> &str;
    fn next(&mut self, wait: Duration) -> Result<Option<Msg>>;
    fn set(&mut self, v: Value) -> Result<()>;
    fn clock(&self) -> Duration;
    fn sleep(&mut self, d: Duration);
}

fn since<B: Bus>(b: &B, t: Duration) -> Duration {
    b.clock().saturating_sub(t)
}

pub fn subscribed<B: Bus>(b: &mut B, timeout: f64) -> Result<()> {
    let until = b.clock() + Duration::from_secs_f64(timeout.max(1.0));
    loop {
        match b.next(until.saturating_sub(b.clock()))? {
            Some(Msg::Subscribed) => return Ok(()),
            Some(_) => {}
            None => bail!("no answer from the MQTT broker"),
        }
    }
}

fn drain<B: Bus>(b: &mut B) -> Result<()> {
    while b.next(Duration::ZERO)?.is_some() {}
    Ok(())
}

pub fn fetch<B: Bus>(b: &mut B, timeout: f64) -> Result<report::State> {
    drain(b)?;
    b.set(json!({"diagnostics_refresh": "refresh"}))?;
    let asked = b.clock();
    let timeout = Duration::from_secs_f64(timeout);
    let mut last: Option<(report::State, Duration)> = None;
    while since(b, asked) < timeout {
        if last.as_ref().is_some_and(|(_, t)| since(b, *t) > Duration::from_secs(3)) {
            break;
        }
        if let Some(Msg::State(s)) = b.next(Duration::from_millis(500).min(timeout.saturating_sub(since(b, asked))))? {
            last = Some((s, b.clock()));
        }
    }
    match last {
        Some((s, _)) => Ok(report::flatten(s)),
        None => bail!("no state from {}: is the bulb on, and the converter installed?", b.topic()),
    }
}

fn page_of(s: &report::State) -> Option<(usize, Vec<u8>)> {
    let p = s.map.get("black_box_page").filter(|p| py::truthy(Some(p)))?;
    let n = p.get("page").and_then(Value::as_u64)? as usize;
    let data = blackbox::unhex(p.get("hex").and_then(Value::as_str)?).ok()?;
    Some((n, data))
}

pub fn fetch_box<B: Bus>(b: &mut B, timeout: f64) -> Result<(Vec<u8>, Vec<usize>)> {
    drain(b)?;
    let mut pages: BTreeMap<usize, Vec<u8>> = BTreeMap::new();
    let mut last = b.clock();
    b.set(json!({"black_box_read": "all"}))?;
    let mut total: Option<usize> = None;
    let t0 = b.clock();
    let timeout = Duration::from_secs_f64(timeout);
    let mut tick = b.clock();
    while since(b, t0) < timeout {
        if let Some((n, data)) = b.next(Duration::from_millis(100))?.and_then(|m| match m {
            Msg::State(s) => page_of(&s),
            Msg::Subscribed | Msg::Failed(_) => None,
        }) {
            if !pages.contains_key(&n) {
                pages.insert(n, data);
                last = b.clock();
            }
        }
        if since(b, tick) < Duration::from_millis(500) {
            continue;
        }
        tick = b.clock();
        if total.is_none() {
            total = pages.get(&0).and_then(|p| blackbox::total_len(p));
        }
        if total.is_some_and(|t| t > 0 && pages.len() * BOX_PAGE >= t) {
            break;
        }
        if !pages.is_empty() && since(b, last) > Duration::from_secs(20) {
            let want = (0..total.unwrap_or(0).div_ceil(BOX_PAGE)).filter(|p| !pages.contains_key(p));
            for p in want.take(5) {
                b.set(json!({"black_box_read": p}))?;
            }
            last = b.clock();
        }
        eprint!("\rblack box: {} pages", pages.len());
    }
    eprintln!();
    let Some(total) = total.or_else(|| pages.get(&0).and_then(|p| blackbox::total_len(p))) else {
        bail!("no page 0 of the black box: is this image 0x00020004 or later, and the converter current?");
    };
    Ok(assemble(&pages, total))
}

pub fn assemble(pages: &BTreeMap<usize, Vec<u8>>, total: usize) -> (Vec<u8>, Vec<usize>) {
    let n = total.div_ceil(BOX_PAGE);
    let mut blob = Vec::with_capacity(n * BOX_PAGE);
    let mut missing = Vec::new();
    for p in 0..n {
        let mut d = pages.get(&p).cloned().unwrap_or_else(|| {
            missing.push(p);
            Vec::new()
        });
        d.resize(BOX_PAGE, 0);
        blob.extend(d);
    }
    blob.truncate(total);
    (blob, missing)
}

fn write_csv(path: &Path, header: &[String], rows: impl Iterator<Item = Vec<String>>) -> Result<()> {
    let mut out = py::csv_row(header);
    for r in rows {
        out += &py::csv_row(&r);
    }
    fs::write(path, out).with_context(|| format!("writing {}", path.display()))
}

pub type Clock = fn(&str) -> String;

pub fn watch<B: Bus>(m: &mut B, a: &DiagArgs, every: f64, out: &mut dyn std::io::Write, time: Clock) -> Result<()> {
    let mut file;
    let (out, empty): (&mut dyn std::io::Write, bool) = match &a.csv {
        Some(p) => {
            file = fs::OpenOptions::new().create(true).append(true).open(p).with_context(|| format!("opening {}", p.display()))?;
            let empty = file.metadata()?.len() == 0;
            (&mut file, empty)
        }
        None => (out, true),
    };
    if empty {
        let mut h: Vec<String> = ["time", "uptime", "noise_floor"].map(String::from).to_vec();
        h.extend(report::COUNTERS.map(String::from));
        out.write_all(py::csv_row(&h).as_bytes())?;
    }
    loop {
        let s = fetch(m, a.conn.timeout)?;
        let mut row = vec![py::csv_quote(&time("%Y-%m-%d %H:%M:%S"))];
        row.extend(["uptime", "noise_floor"].iter().chain(report::COUNTERS.iter()).map(|k| py::csv_cell(s.map.get(*k))));
        out.write_all(py::csv_row(&row).as_bytes())?;
        out.flush()?;
        m.sleep(Duration::from_secs_f64(every.max(0.0)));
    }
}

pub fn diag_on<B: Bus>(m: &mut B, a: &DiagArgs, cache: &Path, out: &mut dyn std::io::Write, time: Clock) -> Result<()> {
    if let Some(every) = a.watch {
        return watch(m, a, every, out, time);
    }
    let s = fetch(m, a.conn.timeout)?;
    if a.json {
        writeln!(out, "{}", serde_json::to_string_pretty(&s)?)?;
        return Ok(());
    }
    let file = cache.join(format!("{}.json", a.ieee));
    let before = fs::read_to_string(&file)
        .ok()
        .and_then(|text| match serde_json::from_str::<Value>(&text) {
            Ok(Value::Object(prev)) => Some(prev),
            _ => None,
        })
        .filter(|prev| py::eq(prev.get("number_of_resets"), s.map.get("number_of_resets")));
    if let Some(prev) = &before {
        writeln!(out, "(growth since {})", py::get(prev, "_when"))?;
    }
    let mut store = s.clone();
    store.insert("_when".into(), Value::String(time("%Y-%m-%d %H:%M:%S")));
    write!(out, "{}", report::report(&s.map, before.as_ref()))?;
    fs::write(&file, serde_json::to_string(&store)?).with_context(|| format!("writing {}", file.display()))?;
    Ok(())
}

pub fn load_box(from: &Path) -> Result<(Vec<u8>, Vec<usize>)> {
    let text = fs::read_to_string(from).with_context(|| format!("reading {}", from.display()))?;
    let v: Value = serde_json::from_str(&text).with_context(|| format!("parsing {}", from.display()))?;
    let raw = v.get("raw").and_then(Value::as_str).context("no \"raw\" in the saved box")?;
    let missing = v
        .get("missing")
        .and_then(Value::as_array)
        .map(|m| m.iter().filter_map(Value::as_u64).map(|p| p as usize).collect())
        .unwrap_or_default();
    Ok((blackbox::unhex(raw)?, missing))
}

pub fn run_box(a: BoxArgs) -> Result<()> {
    let (blob, missing, save) = match &a.from {
        Some(from) => {
            let (blob, missing) = load_box(from)?;
            (blob, missing, a.json.clone())
        }
        None => mqtt::read_box(&a, a.ieee.as_deref().unwrap_or_default())?,
    };
    show_box(&blob, &missing, save.as_deref(), a.csv.as_deref(), &mut std::io::stdout())
}

pub fn show_box(blob: &[u8], missing: &[usize], save: Option<&Path>, csv: Option<&Path>, out: &mut dyn std::io::Write) -> Result<()> {
    let bx = blackbox::decode(blob)?;
    if let Some(p) = save {
        let mut v = serde_json::to_value(&bx)?;
        v["missing"] = json!(missing);
        v["raw"] = Value::String(blackbox::hex(blob));
        fs::write(p, serde_json::to_string(&v)?).with_context(|| format!("writing {}", p.display()))?;
    }
    write!(out, "{}", blackbox::report(&bx, missing))?;
    if let Some(p) = csv {
        let header: Vec<String> =
            if bx.rows.is_empty() { vec!["minute".into()] } else { blackbox::Row::FIELDS.map(String::from).to_vec() };
        write_csv(p, &header, bx.rows.iter().map(blackbox::Row::cells))?;
    }
    if let Some(p) = save {
        writeln!(out, "(saved {})", p.display())?;
    }
    Ok(())
}

#[cfg(test)]
mod tests;
