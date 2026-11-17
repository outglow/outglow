use std::collections::HashMap;
use std::time::{Duration, Instant};

use anyhow::{bail, Context, Result};
use uuid::Uuid;

use super::protocol::{self as p, hex, Status, Target, Transport};
use super::{check_image, fmt_mac, parse_adv, product_model, ImageInfo, PushArgs, AWOX_VENDOR};

pub const DEVINFO: [(u16, &str); 4] = [(0x2a24, "model"), (0x2a26, "firmware"), (0x2a27, "hardware"), (0x2a29, "manufacturer")];

pub const CODES: [u8; 5] = [p::STATUS, p::COMMAND, p::OTA_OLD, p::PAIR, p::OTA];

pub const IO_TIMEOUT: Duration = Duration::from_secs(30);

pub fn sig_uuid(short: u16) -> Uuid {
    Uuid::from_u128(0x0000_0000_0000_1000_8000_0080_5f9b_34fb | (short as u128) << 96)
}

pub fn telink_code(u: Uuid) -> Option<u8> {
    CODES.into_iter().find(|&c| p::telink_uuid(c) == u)
}

pub fn without_response(code: u8, write_without_response: bool) -> bool {
    code == p::OTA && write_without_response
}

pub fn revision_text(v: &[u8]) -> String {
    String::from_utf8_lossy(v).trim_matches(|c| c == '\0' || c == ' ').to_string()
}

#[derive(Clone, Debug, PartialEq)]
pub struct Seen {
    pub id: String,
    pub address: String,
    pub name: Option<String>,
    pub rssi: Option<i16>,
    pub awox: Option<(u16, [u8; 6])>,
}

impl Seen {
    pub fn new(id: String, address: String, name: Option<String>, rssi: Option<i16>, md: &HashMap<u16, Vec<u8>>) -> Seen {
        Seen { id, address, name, rssi, awox: md.get(&AWOX_VENDOR).and_then(|md| parse_adv(md)) }
    }

    pub fn is(&self, target: &str) -> bool {
        let t = target.to_lowercase();
        let addr = self.address.to_lowercase();
        let mac = self.awox.map(|(_, m)| fmt_mac(&m));
        self.id.to_lowercase() == t || (addr == t && addr != "00:00:00:00:00:00") || mac.as_deref() == Some(&t)
    }

    pub fn device_line(&self) -> String {
        match self.awox {
            Some((product, mac)) => {
                format!("device: {} mesh '{}' product {product:#06x} MAC {}", self.id, self.name.as_deref().unwrap_or(""), fmt_mac(&mac))
            }
            None => format!("device: {} (no AwoX data in the advertisement)", self.id),
        }
    }
}

pub fn scan_report(total: usize, all: Vec<Seen>) -> String {
    let mut rows: Vec<((u16, [u8; 6]), Seen)> = all.into_iter().filter_map(|s| Some((s.awox?, s))).collect();
    rows.sort_by_key(|(_, s)| std::cmp::Reverse(s.rssi.unwrap_or(i16::MIN)));
    let mut out = format!("AwoX devices (vendor {AWOX_VENDOR:#06x}): {} of {total} visible\n", rows.len());
    for ((product, mac), s) in rows {
        out += &format!(
            "  {}  rssi {:>4}  mesh '{}'  product {product:#06x} ({})  MAC {}\n",
            s.id,
            s.rssi.map_or("?".into(), |r| r.to_string()),
            s.name.as_deref().unwrap_or(""),
            product_model(product).unwrap_or("other product"),
            fmt_mac(&mac)
        );
    }
    out
}

#[allow(async_fn_in_trait)]
pub trait Device {
    fn awox(&self) -> Option<(u16, [u8; 6])>;
    fn has(&self, code: u8) -> bool;
    async fn read_info(&self, short: u16) -> Result<Option<Vec<u8>>>;
    async fn read(&self, code: u8) -> Result<Vec<u8>>;
    async fn write(&self, code: u8, data: &[u8]) -> Result<()>;
    async fn sleep(&self, d: Duration);
}

pub struct Counted<'a, D> {
    pub d: &'a D,
    pub delay: Duration,
    pub writes: u64,
    pub reads: u64,
}

impl<D: Device> Transport for Counted<'_, D> {
    async fn write(&mut self, code: u8, data: &[u8]) -> Result<()> {
        tokio::time::timeout(IO_TIMEOUT, self.d.write(code, data)).await.context("write: timeout")??;
        self.writes += 1;
        if !self.delay.is_zero() {
            self.d.sleep(self.delay).await;
        }
        Ok(())
    }

    async fn read(&mut self, code: u8) -> Result<Vec<u8>> {
        self.reads += 1;
        tokio::time::timeout(IO_TIMEOUT, self.d.read(code)).await.context("read: timeout")?
    }

    async fn sleep(&mut self, ms: u64) {
        self.d.sleep(Duration::from_millis(ms)).await;
    }
}

pub async fn probe<D: Device>(d: &D, log: &mut dyn FnMut(&str)) -> Result<()> {
    for (short, what) in DEVINFO {
        if let Some(v) = d.read_info(short).await? {
            log(&format!("  {what:10} {:?}", String::from_utf8_lossy(&v)));
        }
    }
    let missing: Vec<String> = [p::PAIR, p::OTA].iter().filter(|&&k| !d.has(k)).map(|k| format!("...19{k:02x}")).collect();
    if !missing.is_empty() {
        log(&format!("WARNING: missing characteristics {missing:?}: this does not look like the Telink mesh profile"));
    } else {
        let st = d.read(p::OTA).await?;
        let target = Target::detect(&st);
        log(&format!("  OTA status (...1915): {}  (0000.. = idle; {} B = {} layout)", hex(&st), st.len(), target.name()));
        log(&format!(
            "  its OTA handler takes blocks up to {} B: push --mtu {} would use them, if the OS negotiates that ATT_MTU",
            target.chunk_cap(),
            target.chunk_cap() + 7
        ));
    }
    log("probe: reads only, nothing written");
    Ok(())
}

pub struct Push {
    pub a: PushArgs,
    pub img: Vec<u8>,
    pub info: ImageInfo,
    pub expected: u16,
    pub password: Option<Vec<u8>>,
}

pub fn prepare(a: PushArgs, log: &mut dyn FnMut(&str)) -> Result<Push> {
    let img = std::fs::read(&a.image).with_context(|| format!("{}", a.image.display()))?;
    let info = check_image(&img, a.allow_outglow)?;
    log(&format!(
        "image: {} {} B, {} hardware revision {} (product {:#06x}), KNLT+CRC ok, {}",
        a.image.display(),
        img.len(),
        info.model,
        info.revision,
        info.product,
        if info.outglow { "OUTGLOW" } else { "ORIGINAL FIRMWARE (canary)" }
    ));
    let expected = a.product.unwrap_or(info.product);
    let password = a.mesh_password.as_deref().map(p::unhex).transpose()?;
    if password.as_ref().is_some_and(|pw| pw.len() != 16) {
        bail!("--mesh-password must be 16 bytes in hex");
    }
    Ok(Push { a, img, info, expected, password })
}

pub async fn push<D: Device>(
    d: &D,
    s: &Push,
    rnd8: &mut dyn FnMut() -> Result<[u8; 8]>,
    log: &mut dyn FnMut(&str),
) -> Result<()> {
    let a = &s.a;
    let awox = d.awox();
    let Some(mac) = awox.filter(|(p, _)| *p == s.expected).map(|(_, m)| m) else {
        bail!(
            "device product {} != {:#06x} required by the image. --product only deliberately.",
            awox.map_or("unknown".into(), |(p, _)| format!("{p:#06x}")),
            s.expected
        );
    };
    let revision = d.read_info(0x2a27).await?.map(|v| revision_text(&v));
    log(&format!("bulb hardware revision {revision:?}, image for {:?}", s.info.revision));
    if revision.as_deref() != Some(s.info.revision.as_str()) && !a.any_revision {
        bail!(
            "the bulb is hardware revision {revision:?}, the image is for {:?}: its LEDs may be on other pins. \
             --any-revision only deliberately.",
            s.info.revision
        );
    }
    if !d.has(p::OTA) {
        bail!("no OTA characteristic ...1915");
    }
    if !a.confirm_write {
        bail!("refused: no --confirm-write (this writes the bulb's flash and restarts it); all checks passed");
    }
    let (mtu, chunk) = negotiate(d, a.mtu, a.chunk, log).await?;
    let mut t = Counted { d, delay: Duration::from_secs_f64(a.delay_ms / 1000.0), writes: 0, reads: 0 };
    if a.login {
        let pw = s.password.clone().unwrap_or_else(|| p::secure_password(&mac).to_vec());
        p::login(&mut t, a.mesh_name.as_bytes(), &pw, &rnd8()?, log).await?;
    }
    let t0 = Instant::now();
    let o = p::OtaOptions { chunk, mtu, max_resends: a.max_resends, ..Default::default() };
    let st = p::run_ota(&mut t, &s.img, &o, log).await?;
    log(&format!("final status {}; {} writes, {} reads, {:.0} s", hex(&st), t.writes, t.reads, t0.elapsed().as_secs_f64()));
    log("the bulb should now restart itself (the AwoX bootloader installs the image).");
    Ok(())
}

pub async fn negotiate<D: Device>(d: &D, mtu: usize, chunk: Option<usize>, log: &mut dyn FnMut(&str)) -> Result<(usize, usize)> {
    if mtu <= p::DEFAULT_MTU && chunk.is_none() {
        log(&format!("classic transfer: ATT_MTU {}, 16 B blocks", p::DEFAULT_MTU));
        return Ok((p::DEFAULT_MTU, p::CHUNK_MIN));
    }
    let idle = d.read(p::OTA).await?;
    let target = Target::detect(&idle);
    log(&format!(
        "OTA status before the transfer: {} ({} B -> {} handler, block limit {} B)",
        hex(&idle),
        idle.len(),
        target.name(),
        target.chunk_cap()
    ));
    if let Some(state) = Status::parse(&idle).state.filter(|s| ![0, 4].contains(s)) {
        log(&format!("WARNING: OTA state {state} before the transfer (0 = idle)"));
    }
    let mtu = mtu.max(p::DEFAULT_MTU);
    let (chunk, why) = p::pick_chunk(mtu, target, chunk)?;
    log(&format!("ATT_MTU {mtu} (as given: btleplug does not report the negotiated one); block {chunk} B ({why})"));
    Ok((mtu, chunk))
}

#[cfg(test)]
mod tests;
