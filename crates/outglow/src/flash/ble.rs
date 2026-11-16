use std::collections::HashMap;
use std::time::{Duration, Instant};

use anyhow::{anyhow, bail, Context, Result};
use btleplug::api::{Central, CharPropFlags, Characteristic, Manager as _, Peripheral as _, ScanFilter, WriteType};
use btleplug::platform::{Adapter, Manager, Peripheral};
use uuid::Uuid;

use super::protocol::{self as p, hex, Status, Target, Transport};
use super::{check_image, fmt_mac, parse_adv, product_model, Ble, AWOX_VENDOR};

const DEVINFO: [(u16, &str); 4] = [(0x2a24, "model"), (0x2a26, "firmware"), (0x2a27, "hardware"), (0x2a29, "manufacturer")];

fn sig_uuid(short: u16) -> Uuid {
    Uuid::from_u128(0x0000_0000_0000_1000_8000_0080_5f9b_34fb | (short as u128) << 96)
}

async fn adapter() -> Result<Adapter> {
    let m = Manager::new().await.context("Bluetooth is not available")?;
    m.adapters().await?.into_iter().next().ok_or_else(|| anyhow!("no Bluetooth adapter"))
}

struct Seen {
    p: Peripheral,
    id: String,
    name: Option<String>,
    rssi: Option<i16>,
    awox: Option<(u16, [u8; 6])>,
}

async fn seen(p: Peripheral) -> Option<Seen> {
    let props = p.properties().await.ok().flatten()?;
    let awox = props.manufacturer_data.get(&AWOX_VENDOR).and_then(|md| parse_adv(md));
    Some(Seen { id: p.id().to_string(), name: props.local_name, rssi: props.rssi, awox, p })
}

pub async fn scan(seconds: f64) -> Result<()> {
    let a = adapter().await?;
    a.start_scan(ScanFilter::default()).await?;
    tokio::time::sleep(Duration::from_secs_f64(seconds)).await;
    a.stop_scan().await.ok();
    let all = a.peripherals().await?;
    let total = all.len();
    let mut rows = vec![];
    for p in all {
        if let Some(s) = seen(p).await.filter(|s| s.awox.is_some()) {
            rows.push(s);
        }
    }
    rows.sort_by_key(|s| std::cmp::Reverse(s.rssi.unwrap_or(i16::MIN)));
    println!("AwoX devices (vendor {AWOX_VENDOR:#06x}): {} of {total} visible", rows.len());
    for s in rows {
        let (product, mac) = s.awox.unwrap();
        println!(
            "  {}  rssi {:>4}  mesh '{}'  product {product:#06x} ({})  MAC {}",
            s.id,
            s.rssi.map_or("?".into(), |r| r.to_string()),
            s.name.as_deref().unwrap_or(""),
            product_model(product).unwrap_or("other product"),
            fmt_mac(&mac)
        );
    }
    Ok(())
}

async fn find(a: &Adapter, target: &str, seconds: f64) -> Result<Seen> {
    let t = target.to_lowercase();
    a.start_scan(ScanFilter::default()).await?;
    let end = Instant::now() + Duration::from_secs_f64(seconds);
    let found = 'scan: loop {
        for p in a.peripherals().await? {
            let addr = p.address().to_string().to_lowercase();
            if let Some(s) = seen(p).await {
                let mac = s.awox.map(|(_, m)| fmt_mac(&m));
                if s.id.to_lowercase() == t || (addr == t && addr != "00:00:00:00:00:00") || mac.as_deref() == Some(&t) {
                    break 'scan Some(s);
                }
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
    let s = find(&a, target, seconds).await?;
    match s.awox {
        Some((product, mac)) => println!(
            "device: {} mesh '{}' product {product:#06x} MAC {}",
            s.id,
            s.name.as_deref().unwrap_or(""),
            fmt_mac(&mac)
        ),
        None => println!("device: {} (no AwoX data in the advertisement)", s.id),
    }
    tokio::time::timeout(Duration::from_secs(20), s.p.connect()).await.context("connect: timeout")??;
    s.p.discover_services().await?;
    println!("connected");
    let all: Vec<Characteristic> = s.p.characteristics().into_iter().collect();
    let mut chars = HashMap::new();
    for c in &all {
        for code in [p::STATUS, p::COMMAND, p::OTA_OLD, p::PAIR, p::OTA] {
            if c.uuid == p::telink_uuid(code) {
                println!("  ...19{code:02x} props {:?}", c.properties);
                chars.insert(code, c.clone());
            }
        }
    }
    Ok(Connected { p: s.p, chars, all, awox: s.awox })
}

impl Connected {
    async fn read_uuid(&self, u: Uuid) -> Result<Option<Vec<u8>>> {
        match self.all.iter().find(|c| c.uuid == u && c.properties.contains(CharPropFlags::READ)) {
            Some(c) => Ok(Some(self.p.read(c).await?)),
            None => Ok(None),
        }
    }

    async fn revision(&self) -> Result<Option<String>> {
        Ok(self
            .read_uuid(sig_uuid(0x2a27))
            .await?
            .map(|v| String::from_utf8_lossy(&v).trim_matches(|c| c == '\0' || c == ' ').to_string()))
    }

    async fn disconnect(&self) {
        self.p.disconnect().await.ok();
    }
}

struct BtTransport<'a> {
    c: &'a Connected,
    delay: Duration,
    writes: u64,
    reads: u64,
}

impl<'a> BtTransport<'a> {
    fn ch(&self, code: u8) -> Result<&'a Characteristic> {
        let c: &'a Connected = self.c;
        c.chars.get(&code).ok_or_else(|| anyhow!("no characteristic ...19{code:02x}"))
    }
}

impl Transport for BtTransport<'_> {
    async fn write(&mut self, code: u8, data: &[u8]) -> Result<()> {
        let ch = self.ch(code)?;
        let kind = if code == p::OTA && ch.properties.contains(CharPropFlags::WRITE_WITHOUT_RESPONSE) {
            WriteType::WithoutResponse
        } else {
            WriteType::WithResponse
        };
        tokio::time::timeout(Duration::from_secs(30), self.c.p.write(ch, data, kind)).await.context("write: timeout")??;
        self.writes += 1;
        if !self.delay.is_zero() {
            tokio::time::sleep(self.delay).await;
        }
        Ok(())
    }

    async fn read(&mut self, code: u8) -> Result<Vec<u8>> {
        let ch = self.ch(code)?;
        self.reads += 1;
        Ok(tokio::time::timeout(Duration::from_secs(30), self.c.p.read(ch)).await.context("read: timeout")??)
    }

    async fn sleep(&mut self, ms: u64) {
        tokio::time::sleep(Duration::from_millis(ms)).await;
    }
}

pub async fn probe(target: &str, seconds: f64) -> Result<()> {
    let c = connect(target, seconds).await?;
    let r = probe_connected(&c).await;
    c.disconnect().await;
    r
}

async fn probe_connected(c: &Connected) -> Result<()> {
    for (short, what) in DEVINFO {
        if let Some(v) = c.read_uuid(sig_uuid(short)).await? {
            println!("  {what:10} {:?}", String::from_utf8_lossy(&v));
        }
    }
    let missing: Vec<String> =
        [p::PAIR, p::OTA].iter().filter(|k| !c.chars.contains_key(k)).map(|k| format!("...19{k:02x}")).collect();
    if !missing.is_empty() {
        println!("WARNING: missing characteristics {missing:?}: this does not look like the Telink mesh profile");
    } else {
        let st = c.p.read(&c.chars[&p::OTA]).await?;
        let target = Target::detect(&st);
        println!("  OTA status (...1915): {}  (0000.. = idle; {} B = {} layout)", hex(&st), st.len(), target.name());
        println!(
            "  its OTA handler takes blocks up to {} B: push --mtu {} would use them, if the OS negotiates that ATT_MTU",
            target.chunk_cap(),
            target.chunk_cap() + 7
        );
    }
    println!("probe: reads only, nothing written");
    Ok(())
}

pub async fn push(cmd: Ble) -> Result<()> {
    let Ble::Push {
        target,
        image,
        confirm_write,
        allow_outglow,
        any_revision,
        product,
        mtu,
        chunk,
        max_resends,
        delay_ms,
        login,
        mesh_name,
        mesh_password,
        seconds,
    } = cmd
    else {
        unreachable!()
    };
    let img = std::fs::read(&image).with_context(|| format!("{}", image.display()))?;
    let info = check_image(&img, allow_outglow)?;
    println!(
        "image: {} {} B, {} hardware revision {} (product {:#06x}), KNLT+CRC ok, {}",
        image.display(),
        img.len(),
        info.model,
        info.revision,
        info.product,
        if info.outglow { "OUTGLOW" } else { "ORIGINAL FIRMWARE (canary)" }
    );
    let expected = product.unwrap_or(info.product);
    let password = mesh_password.as_deref().map(p::unhex).transpose()?;
    if password.as_ref().is_some_and(|pw| pw.len() != 16) {
        bail!("--mesh-password must be 16 bytes in hex");
    }
    let c = connect(&target, seconds).await?;
    let r = async {
        let bulb_product = c.awox.map(|(p, _)| p);
        if bulb_product != Some(expected) {
            bail!(
                "device product {} != {expected:#06x} required by the image. --product only deliberately.",
                bulb_product.map_or("unknown".into(), |p| format!("{p:#06x}"))
            );
        }
        let revision = c.revision().await?;
        println!("bulb hardware revision {revision:?}, image for {:?}", info.revision);
        if revision.as_deref() != Some(info.revision.as_str()) && !any_revision {
            bail!(
                "the bulb is hardware revision {revision:?}, the image is for {:?}: its LEDs may be on other pins. \
                 --any-revision only deliberately.",
                info.revision
            );
        }
        if !c.chars.contains_key(&p::OTA) {
            bail!("no OTA characteristic ...1915");
        }
        if !confirm_write {
            bail!("refused: no --confirm-write (this writes the bulb's flash and restarts it); all checks passed");
        }
        let (mtu, d) = negotiate(&c, mtu, chunk).await?;
        let mut t = BtTransport { c: &c, delay: Duration::from_secs_f64(delay_ms / 1000.0), writes: 0, reads: 0 };
        let mut log = |s: &str| println!("{s}");
        if login {
            let pw = match &password {
                Some(pw) => pw.clone(),
                None => {
                    let (_, mac) = c.awox.ok_or_else(|| anyhow!("no MAC for the secure password: give --mesh-password"))?;
                    p::secure_password(&mac).to_vec()
                }
            };
            let mut rnd8 = [0u8; 8];
            getrandom::getrandom(&mut rnd8).map_err(|e| anyhow!("random: {e}"))?;
            p::login(&mut t, mesh_name.as_bytes(), &pw, &rnd8, &mut log).await?;
        }
        let t0 = Instant::now();
        let o = p::OtaOptions { chunk: d, mtu, max_resends, ..Default::default() };
        let st = p::run_ota(&mut t, &img, &o, &mut log).await?;
        println!(
            "final status {}; {} writes, {} reads, {:.0} s",
            hex(&st),
            t.writes,
            t.reads,
            t0.elapsed().as_secs_f64()
        );
        println!("the bulb should now restart itself (the AwoX bootloader installs the image).");
        Ok(())
    }
    .await;
    c.disconnect().await;
    r
}

async fn negotiate(c: &Connected, mtu: usize, chunk: Option<usize>) -> Result<(usize, usize)> {
    if mtu <= p::DEFAULT_MTU && chunk.is_none() {
        println!("classic transfer: ATT_MTU {}, 16 B blocks", p::DEFAULT_MTU);
        return Ok((p::DEFAULT_MTU, p::CHUNK_MIN));
    }
    let idle = c.p.read(&c.chars[&p::OTA]).await?;
    let target = Target::detect(&idle);
    println!(
        "OTA status before the transfer: {} ({} B -> {} handler, block limit {} B)",
        hex(&idle),
        idle.len(),
        target.name(),
        target.chunk_cap()
    );
    let st = Status::parse(&idle);
    if !matches!(st.state, None | Some(0) | Some(4)) {
        println!("WARNING: OTA state {} before the transfer (0 = idle)", st.state.unwrap());
    }
    let mtu = mtu.max(p::DEFAULT_MTU);
    let (d, why) = p::pick_chunk(mtu, target, chunk)?;
    println!("ATT_MTU {mtu} (as given: btleplug does not report the negotiated one); block {d} B ({why})");
    Ok((mtu, d))
}

#[cfg(test)]
mod tests {
    #[test]
    fn device_information_uuids() {
        assert_eq!(super::sig_uuid(0x2a27).to_string(), "00002a27-0000-1000-8000-00805f9b34fb");
    }
}
