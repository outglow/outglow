use std::collections::HashMap;
use std::time::{Duration, Instant};

use anyhow::{anyhow, Context, Result};
use btleplug::api::{Central, CharPropFlags, Characteristic, Manager as _, Peripheral as _, ScanFilter, WriteType};
use btleplug::platform::{Adapter, Manager, Peripheral};

use super::ble::{self, sig_uuid, Device, Seen};
use super::{vectors, Ble, PushArgs};

pub fn run(c: Ble) -> Result<()> {
    let block = |f: std::pin::Pin<Box<dyn std::future::Future<Output = Result<()>>>>| tokio::runtime::Runtime::new()?.block_on(f);
    match c {
        Ble::Vectors { image } => vectors::run(image),
        Ble::Scan { seconds } => block(Box::pin(scan(seconds))),
        Ble::Probe { target, seconds } => block(Box::pin(probe(target, seconds))),
        Ble::Push(a) => block(Box::pin(push(a))),
    }
}

fn say(s: &str) {
    println!("{s}");
}

async fn adapter() -> Result<Adapter> {
    let m = Manager::new().await.context("Bluetooth is not available")?;
    m.adapters().await?.into_iter().next().ok_or_else(|| anyhow!("no Bluetooth adapter"))
}

async fn seen(p: Peripheral) -> Option<(Peripheral, Seen)> {
    let props = p.properties().await.ok().flatten()?;
    let s = Seen::new(p.id().to_string(), p.address().to_string(), props.local_name, props.rssi, &props.manufacturer_data);
    Some((p, s))
}

async fn scan(seconds: f64) -> Result<()> {
    let a = adapter().await?;
    a.start_scan(ScanFilter::default()).await?;
    tokio::time::sleep(Duration::from_secs_f64(seconds)).await;
    a.stop_scan().await.ok();
    let all = a.peripherals().await?;
    let total = all.len();
    let mut rows = vec![];
    for p in all {
        rows.extend(seen(p).await.map(|(_, s)| s));
    }
    print!("{}", ble::scan_report(total, rows));
    Ok(())
}

async fn find(a: &Adapter, target: &str, seconds: f64) -> Result<(Peripheral, Seen)> {
    a.start_scan(ScanFilter::default()).await?;
    let end = Instant::now() + Duration::from_secs_f64(seconds);
    let found = 'scan: loop {
        for p in a.peripherals().await? {
            if let Some(s) = seen(p).await.filter(|(_, s)| s.is(target)) {
                break 'scan Some(s);
            }
        }
        if Instant::now() >= end {
            break None;
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    };
    a.stop_scan().await.ok();
    found.ok_or_else(|| anyhow!("device {target} not seen in BLE advertisements ({seconds} s)"))
}

struct Connected {
    p: Peripheral,
    chars: HashMap<u8, Characteristic>,
    all: Vec<Characteristic>,
    awox: Option<(u16, [u8; 6])>,
}

async fn connect(target: &str, seconds: f64) -> Result<Connected> {
    let a = adapter().await?;
    let (p, s) = find(&a, target, seconds).await?;
    say(&s.device_line());
    tokio::time::timeout(Duration::from_secs(20), p.connect()).await.context("connect: timeout")??;
    p.discover_services().await?;
    say("connected");
    let all: Vec<Characteristic> = p.characteristics().into_iter().collect();
    let mut chars = HashMap::new();
    for c in &all {
        if let Some(code) = ble::telink_code(c.uuid) {
            say(&format!("  ...19{code:02x} props {:?}", c.properties));
            chars.insert(code, c.clone());
        }
    }
    Ok(Connected { p, chars, all, awox: s.awox })
}

impl Connected {
    fn ch(&self, code: u8) -> Result<&Characteristic> {
        self.chars.get(&code).ok_or_else(|| anyhow!("no characteristic ...19{code:02x}"))
    }
}

impl Device for Connected {
    fn awox(&self) -> Option<(u16, [u8; 6])> {
        self.awox
    }

    fn has(&self, code: u8) -> bool {
        self.chars.contains_key(&code)
    }

    async fn read_info(&self, short: u16) -> Result<Option<Vec<u8>>> {
        let u = sig_uuid(short);
        match self.all.iter().find(|c| c.uuid == u && c.properties.contains(CharPropFlags::READ)) {
            Some(c) => Ok(Some(self.p.read(c).await?)),
            None => Ok(None),
        }
    }

    async fn read(&self, code: u8) -> Result<Vec<u8>> {
        Ok(self.p.read(self.ch(code)?).await?)
    }

    async fn write(&self, code: u8, data: &[u8]) -> Result<()> {
        let ch = self.ch(code)?;
        let kind = if ble::without_response(code, ch.properties.contains(CharPropFlags::WRITE_WITHOUT_RESPONSE)) {
            WriteType::WithoutResponse
        } else {
            WriteType::WithResponse
        };
        Ok(self.p.write(ch, data, kind).await?)
    }

    async fn sleep(&self, d: Duration) {
        tokio::time::sleep(d).await;
    }
}

async fn probe(target: String, seconds: f64) -> Result<()> {
    let c = connect(&target, seconds).await?;
    let r = ble::probe(&c, &mut say).await;
    c.p.disconnect().await.ok();
    r
}

async fn push(a: PushArgs) -> Result<()> {
    let s = ble::prepare(a, &mut say)?;
    let c = connect(&s.a.target, s.a.seconds).await?;
    let mut rnd8 = || {
        let mut r = [0u8; 8];
        getrandom::getrandom(&mut r).map_err(|e| anyhow!("random: {e}"))?;
        Ok(r)
    };
    let r = ble::push(&c, &s, &mut rnd8, &mut say).await;
    c.p.disconnect().await.ok();
    r
}
