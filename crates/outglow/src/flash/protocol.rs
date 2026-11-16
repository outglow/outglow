use aes::cipher::{BlockEncrypt, KeyInit};
use anyhow::{anyhow, bail, Result};
use sha2::{Digest, Sha256};

pub const STATUS: u8 = 0x11;
pub const COMMAND: u8 = 0x12;
pub const OTA_OLD: u8 = 0x13;
pub const PAIR: u8 = 0x14;
pub const OTA: u8 = 0x15;

pub const AWOX_SECRET: [u8; 8] = [0x56, 0xE8, 0x50, 0xED, 0xCF, 0xDD, 0x8B, 0x99];
pub const DEFAULT_MTU: usize = 23;
pub const CHUNK_MIN: usize = 16;

pub fn telink_uuid(short: u8) -> uuid::Uuid {
    uuid::Uuid::from_u128(0x0001_0203_0405_0607_0809_0a0b_0c0d_1900 | short as u128)
}

pub fn stock_error(e: u8) -> String {
    match e {
        0 => "ok".into(),
        1 => "CRC error".into(),
        2 => "sequence error".into(),
        3 => "index beyond the buffer".into(),
        4 => "packet in the wrong state".into(),
        5 => "flash read-back differs".into(),
        _ => format!("error {e}"),
    }
}

pub fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

pub fn unhex(s: &str) -> Result<Vec<u8>> {
    let s = s.trim();
    if s.len() % 2 != 0 {
        bail!("odd number of hex digits");
    }
    (0..s.len()).step_by(2).map(|i| u8::from_str_radix(&s[i..i + 2], 16).map_err(|e| anyhow!("{s}: {e}"))).collect()
}

pub fn crc16_modbus(data: &[u8]) -> u16 {
    let mut crc = 0xFFFFu16;
    for &b in data {
        crc ^= b as u16;
        for _ in 0..8 {
            crc = if crc & 1 != 0 { (crc >> 1) ^ 0xA001 } else { crc >> 1 };
        }
    }
    crc
}

pub fn erase_packet(img_len: usize, d: usize) -> Vec<u8> {
    let mut b = vec![0u8; d + 4];
    b[2] = (img_len / 4096 + 2) as u8;
    b
}

pub fn packet_count(img_len: usize, d: usize) -> usize {
    img_len.div_ceil(d)
}

pub fn data_packet(img: &[u8], i: usize, d: usize) -> Vec<u8> {
    let mut p = Vec::with_capacity(d + 4);
    p.extend_from_slice(&(i as u16).to_le_bytes());
    let from = (i * d).min(img.len());
    let to = ((i + 1) * d).min(img.len());
    p.extend_from_slice(&img[from..to]);
    p.resize(d + 2, 0xFF);
    let crc = crc16_modbus(&p);
    p.extend_from_slice(&crc.to_le_bytes());
    p
}

pub fn end_packet(img_len: usize, d: usize) -> Vec<u8> {
    let mut p = (packet_count(img_len, d) as u16).to_le_bytes().to_vec();
    let crc = crc16_modbus(&p);
    p.extend_from_slice(&crc.to_le_bytes());
    p
}

pub fn telink_aes(key: &[u8; 16], data: &[u8; 16]) -> [u8; 16] {
    let mut k = *key;
    k.reverse();
    let mut b = *data;
    b.reverse();
    let cipher = aes::Aes128::new(&k.into());
    let mut block = b.into();
    cipher.encrypt_block(&mut block);
    let mut out: [u8; 16] = block.into();
    out.reverse();
    out
}

fn pad16(b: &[u8]) -> [u8; 16] {
    let mut out = [0u8; 16];
    let n = b.len().min(16);
    out[..n].copy_from_slice(&b[..n]);
    out
}

pub fn name_pass(name: &[u8], password: &[u8]) -> [u8; 16] {
    let (n, p) = (pad16(name), pad16(password));
    std::array::from_fn(|i| n[i] ^ p[i])
}

pub fn login_packet(name: &[u8], password: &[u8], rnd8: &[u8; 8]) -> Vec<u8> {
    let enc = telink_aes(&pad16(rnd8), &name_pass(name, password));
    let mut out = vec![0x0C];
    out.extend_from_slice(rnd8);
    out.extend_from_slice(&enc[..8]);
    out
}

pub fn session_key(name: &[u8], password: &[u8], rnd8: &[u8; 8], resp8: &[u8; 8]) -> [u8; 16] {
    let mut d = [0u8; 16];
    d[..8].copy_from_slice(rnd8);
    d[8..].copy_from_slice(resp8);
    telink_aes(&name_pass(name, password), &d)
}

pub fn secure_password(mac6: &[u8; 6]) -> [u8; 16] {
    let mut h = Sha256::new();
    h.update(mac6);
    h.update(AWOX_SECRET);
    h.update([0, 0]);
    h.finalize()[..16].try_into().unwrap()
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Target {
    Stock,
    Outglow,
    Unknown,
}

impl Target {
    pub fn detect(idle_status: &[u8]) -> Target {
        match idle_status.len() {
            9 => Target::Stock,
            20 => Target::Outglow,
            _ => Target::Unknown,
        }
    }

    pub fn chunk_cap(self) -> usize {
        match self {
            Target::Stock => 240,
            Target::Outglow => 57,
            Target::Unknown => CHUNK_MIN,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Target::Stock => "original firmware",
            Target::Outglow => "Outglow",
            Target::Unknown => "unknown",
        }
    }
}

#[derive(Clone, Debug)]
pub struct Status {
    pub raw: Vec<u8>,
    pub layout: Target,
    pub state: Option<u8>,
    pub error: Option<u8>,
    pub last_ok: Option<u16>,
    pub last_rx: Option<u16>,
    pub errors: Option<u8>,
}

impl Status {
    pub fn parse(raw: &[u8]) -> Status {
        let r = raw;
        let layout = match r.len() {
            9 => Target::Stock,
            n if n >= 9 => Target::Outglow,
            _ => Target::Unknown,
        };
        let stock = layout == Target::Stock;
        Status {
            raw: r.to_vec(),
            layout,
            state: r.first().copied(),
            error: r.get(1).copied(),
            last_ok: stock.then(|| u16::from_le_bytes([r[2], r[3]])),
            last_rx: stock.then(|| u16::from_le_bytes([r[4], r[5]])),
            errors: stock.then(|| r[6]),
        }
    }

    pub fn resumable(&self) -> bool {
        self.layout == Target::Stock && self.state == Some(3) && matches!(self.error, Some(1 | 2))
    }

    pub fn next_index(&self) -> Option<usize> {
        self.last_ok.map(|l| l.wrapping_add(1) as usize)
    }

    pub fn failed(&self) -> bool {
        if self.layout == Target::Stock {
            matches!(self.error, Some(3..=5)) || self.state == Some(0)
        } else {
            self.error == Some(2)
        }
    }

    pub fn describe(&self) -> String {
        let opt = |v: Option<u8>| v.map_or("None".into(), |v| v.to_string());
        if self.layout == Target::Stock {
            let last = match self.last_ok {
                Some(0xFFFF) => "none".into(),
                v => v.map_or("None".into(), |v| v.to_string()),
            };
            return format!(
                "state {}, {}, last written block {last}, last received {}, consecutive errors {}",
                opt(self.state),
                self.error.map_or("error None".into(), stock_error),
                self.last_rx.map_or("None".into(), |v| v.to_string()),
                opt(self.errors)
            );
        }
        let rx = (self.raw.len() >= 4).then(|| u16::from_le_bytes([self.raw[2], self.raw[3]]));
        format!(
            "state {}, result {}{}, blocks received {}",
            opt(self.state),
            opt(self.error),
            if self.error == Some(2) { " (failed)" } else { "" },
            rx.map_or("None".into(), |v| v.to_string())
        )
    }
}

pub fn pick_chunk(mtu: usize, target: Target, chunk: Option<usize>) -> Result<(usize, String)> {
    let room = mtu.saturating_sub(7);
    let cap = target.chunk_cap();
    if let Some(c) = chunk {
        if c < CHUNK_MIN || c > room {
            bail!("chunk {c} B outside {CHUNK_MIN}..{room} (ATT_MTU {mtu} - 7)");
        }
        if c > cap {
            bail!("chunk {c} B is more than the {} OTA handler takes ({cap} B)", target.name());
        }
        return Ok((c, "requested".into()));
    }
    let d = CHUNK_MIN.max(room.min(cap));
    let why = if d == room { format!("ATT_MTU {mtu} - 7") } else { format!("{} handler limit {cap} B", target.name()) };
    Ok((d, why))
}

#[allow(async_fn_in_trait)]
pub trait Transport {
    async fn write(&mut self, ch: u8, data: &[u8]) -> Result<()>;
    async fn read(&mut self, ch: u8) -> Result<Vec<u8>>;
    async fn sleep(&mut self, ms: u64);
}

pub async fn login<T: Transport>(
    t: &mut T,
    name: &[u8],
    password: &[u8],
    rnd8: &[u8; 8],
    log: &mut dyn FnMut(&str),
) -> Result<[u8; 16]> {
    t.write(PAIR, &login_packet(name, password, rnd8)).await?;
    let rsp = t.read(PAIR).await?;
    if rsp.len() < 9 || rsp[0] != 0x0D {
        bail!("login refused: {}", hex(&rsp));
    }
    log(&format!("logged in (response {})", hex(&rsp[..9])));
    Ok(session_key(name, password, rnd8, rsp[1..9].try_into().unwrap()))
}

pub struct OtaOptions {
    pub chunk: usize,
    pub mtu: usize,
    pub read_every: usize,
    pub erase_timeout_s: u64,
    pub max_resends: u32,
}

impl Default for OtaOptions {
    fn default() -> Self {
        OtaOptions { chunk: CHUNK_MIN, mtu: DEFAULT_MTU, read_every: 8, erase_timeout_s: 90, max_resends: 10 }
    }
}

pub async fn run_ota<T: Transport>(t: &mut T, img: &[u8], o: &OtaOptions, log: &mut dyn FnMut(&str)) -> Result<Vec<u8>> {
    let d = o.chunk;
    if d < CHUNK_MIN {
        bail!("block {d} B is less than {CHUNK_MIN} B");
    }
    let total = packet_count(img.len(), d);
    t.write(OTA, &erase_packet(img.len(), d)).await?;
    log(&format!("erasing: {} 4K sectors", img.len() / 4096 + 2));
    log(&format!("block {d} B, packet {} B (ATT_MTU {}), {total} packets", d + 4, o.mtu));
    t.sleep(5000).await;
    let mut waited = 5u64;
    loop {
        let rsp = t.read(OTA).await?;
        if rsp.first() == Some(&2) {
            break;
        }
        if waited > o.erase_timeout_s {
            bail!("erase did not finish: {}", hex(&rsp));
        }
        t.sleep(3000).await;
        waited += 3;
    }
    log(&format!("erase finished after ~{waited} s"));

    let mut sent = vec![0u32; total];
    let (mut resends, mut resent_packets) = (0u32, 0u32);
    let (mut stuck_from, mut stuck_n) = (-1i64, 0u32);
    let mut resend_point = |st: &Status, place: &str, log: &mut dyn FnMut(&str)| -> Result<Option<usize>> {
        if !st.resumable() {
            return Ok(None);
        }
        let nxt = st.next_index().unwrap();
        if nxt as i64 <= stuck_from {
            stuck_n += 1;
        } else {
            (stuck_from, stuck_n) = (nxt as i64, 1);
        }
        if stuck_n > o.max_resends {
            bail!("{place}: {} resends from block {nxt} without progress ({})", o.max_resends, st.describe());
        }
        resends += 1;
        log(&format!(
            "bulb reports {} {place} ({}): resending from block {nxt} (resend {resends})",
            stock_error(st.error.unwrap_or(0)),
            st.describe()
        ));
        Ok(Some(nxt))
    };

    let mut i = 0usize;
    let mut since_read = 0usize;
    let raw = loop {
        while i < total {
            let attempt = sent[i];
            sent[i] += 1;
            if attempt > 0 {
                resent_packets += 1;
            }
            t.write(OTA, &data_packet(img, i, d)).await?;
            since_read += 1;
            if i % 2000 == 0 && attempt == 0 {
                log(&format!("packet {i}/{total}"));
            }
            i += 1;
            if since_read == o.read_every {
                since_read = 0;
                let st = Status::parse(&t.read(OTA).await?);
                if let Some(nxt) = resend_point(&st, &format!("after block {}", i - 1), log)? {
                    i = nxt;
                } else if st.failed() {
                    bail!("bulb aborted OTA at packet {}: {} ({})", i - 1, hex(&st.raw), st.describe());
                }
            }
        }
        t.write(OTA, &end_packet(img.len(), d)).await?;
        since_read = 0;
        let raw = t.read(OTA).await?;
        match resend_point(&Status::parse(&raw), "at the end packet", log)? {
            Some(nxt) => i = nxt,
            None => break raw,
        }
    };
    if raw.len() < 2 || raw[1] != 0 {
        bail!("OTA end: status {}", hex(&raw));
    }
    if resends > 0 {
        log(&format!("resends: {resends}, packets sent again: {resent_packets}"));
    }
    log("OTA finished successfully (status 0)");
    Ok(raw)
}

#[cfg(test)]
mod tests {
    use super::*;

    const MAC6: [u8; 6] = [0xa4, 0xc1, 0x38, 0x12, 0x34, 0x56];
    const RND8: [u8; 8] = [1, 2, 3, 4, 5, 6, 7, 8];

    fn img() -> Vec<u8> {
        (0..1500).map(|i: usize| (i * 31 + 7) as u8).collect()
    }

    #[test]
    fn crc16_check_value() {
        assert_eq!(crc16_modbus(b"123456789"), 0x4b37);
    }

    #[test]
    fn login_and_password_as_the_python() {
        let pw = secure_password(&MAC6);
        assert_eq!(hex(&pw), "3504d3ee3b5aca48c0e9afabf862af0b");
        assert_eq!(hex(&login_packet(b"unpaired", &pw, &RND8)), "0c010203040506070819e277313fa22e08");
        assert_eq!(hex(&login_packet(b"telink_mesh1", b"123", &RND8)), "0c01020304050607088aa956707635d16a");
        let resp: [u8; 8] = std::array::from_fn(|i| 0x11 + i as u8);
        assert_eq!(hex(&session_key(b"unpaired", &pw, &RND8, &resp)), "50d0bf84818716cc0527cf88c2c12c71");
    }

    #[test]
    fn packets_as_the_python() {
        let img = img();
        assert_eq!(packet_count(img.len(), 16), 94);
        assert_eq!(packet_count(img.len(), 40), 38);
        assert_eq!(hex(&erase_packet(img.len(), 16)), "0000020000000000000000000000000000000000");
        assert_eq!(hex(&erase_packet(img.len(), 40)), format!("000002{}", "00".repeat(41)));
        assert_eq!(hex(&data_packet(&img, 0, 16)), "00000726456483a2c1e0ff1e3d5c7b9ab9d8e274");
        assert_eq!(hex(&data_packet(&img, 1, 16)), "0100f71635547392b1d0ef0e2d4c6b8aa9c8c724");
        assert_eq!(hex(&data_packet(&img, 93, 16)), "5d0037567594b3d2f1102f4e6d8cffffffff410f");
        assert_eq!(
            hex(&data_packet(&img, 0, 40)),
            "00000726456483a2c1e0ff1e3d5c7b9ab9d8f71635547392b1d0ef0e2d4c6b8aa9c8e70625446382a1c08e78"
        );
        assert_eq!(
            hex(&data_packet(&img, 37, 40)),
            "25003f5e7d9cbbdaf91837567594b3d2f1102f4e6d8cffffffffffffffffffffffffffffffffffffffff618f"
        );
        assert_eq!(hex(&end_packet(img.len(), 16)), "5e0039d0");
        assert_eq!(hex(&end_packet(img.len(), 40)), "26001bd0");
        assert_eq!(hex(&erase_packet(0x5A000, 16)), "00005c0000000000000000000000000000000000");
        assert_eq!(hex(&end_packet(0x5A000, 16)), "005a818b");
    }

    #[test]
    fn telink_uuids() {
        assert_eq!(telink_uuid(OTA).to_string(), "00010203-0405-0607-0809-0a0b0c0d1915");
    }

    #[test]
    fn chunk_limits() {
        assert_eq!(pick_chunk(23, Target::Stock, None).unwrap().0, 16);
        assert_eq!(pick_chunk(247, Target::Stock, None).unwrap().0, 240);
        assert_eq!(pick_chunk(247, Target::Outglow, None).unwrap().0, 57);
        assert_eq!(pick_chunk(247, Target::Unknown, None).unwrap().0, 16);
        assert_eq!(pick_chunk(64, Target::Outglow, Some(57)).unwrap().0, 57);
        assert!(pick_chunk(247, Target::Outglow, Some(58)).is_err());
        assert!(pick_chunk(23, Target::Stock, Some(40)).is_err());
        assert!(pick_chunk(247, Target::Stock, Some(15)).is_err());
    }

    #[test]
    fn status_layouts() {
        let s = Status::parse(&[3, 1, 36, 0, 37, 0, 1, 0, 0]);
        assert_eq!(s.layout, Target::Stock);
        assert!(s.resumable() && !s.failed());
        assert_eq!(s.next_index(), Some(37));
        assert_eq!(
            s.describe(),
            "state 3, CRC error, last written block 36, last received 37, consecutive errors 1"
        );
        assert!(Status::parse(&[0, 0, 0xff, 0xff, 0xff, 0xff, 0, 0, 0]).failed());
        let mut ours = [0u8; 20];
        ours[0] = 3;
        ours[1] = 2;
        ours[2] = 5;
        let s = Status::parse(&ours);
        assert_eq!(s.layout, Target::Outglow);
        assert!(s.failed() && !s.resumable());
        assert_eq!(s.describe(), "state 3, result 2 (failed), blocks received 5");
        assert_eq!(Target::detect(&ours), Target::Outglow);
        assert_eq!(Target::detect(&[0; 9]), Target::Stock);
        assert_eq!(Target::detect(&[0; 2]), Target::Unknown);
    }

    struct FakeStock {
        img: Vec<u8>,
        d: usize,
        state: u8,
        last_ok: u16,
        last_rx: u16,
        error: u8,
        errors: u8,
        erasing: u8,
        damage: Vec<usize>,
        drop: Vec<usize>,
        trace: Vec<String>,
        buf: Vec<u8>,
        fail_reads: bool,
    }

    impl FakeStock {
        fn new(img: &[u8], d: usize, damage: &[usize], drop: &[usize]) -> Self {
            FakeStock {
                img: img.to_vec(),
                d,
                state: 0,
                last_ok: 0xFFFF,
                last_rx: 0xFFFF,
                error: 0,
                errors: 0,
                erasing: 0,
                damage: damage.to_vec(),
                drop: drop.to_vec(),
                trace: vec![],
                buf: vec![],
                fail_reads: false,
            }
        }

        fn trace_hash(&self) -> String {
            hex(&Sha256::digest(self.trace.join("\n").as_bytes()))
        }
    }

    impl Transport for FakeStock {
        async fn write(&mut self, ch: u8, p: &[u8]) -> Result<()> {
            assert_eq!(ch, OTA);
            self.trace.push(format!("w {}", hex(p)));
            if self.state == 0 {
                assert_eq!(p.len(), self.d + 4);
                assert_eq!(p[2] as usize, self.img.len() / 4096 + 2);
                self.state = 1;
                self.erasing = 2;
                return Ok(());
            }
            if p.len() == 4 {
                if self.error == 0 {
                    self.state = 4;
                }
                return Ok(());
            }
            let mut p = p.to_vec();
            let i = u16::from_le_bytes([p[0], p[1]]) as usize;
            if let Some(k) = self.drop.iter().position(|&x| x == i) {
                self.drop.remove(k);
                return Ok(());
            }
            if let Some(k) = self.damage.iter().position(|&x| x == i) {
                self.damage.remove(k);
                *p.last_mut().unwrap() ^= 0x5A;
            }
            if self.error != 0 {
                return Ok(());
            }
            assert_eq!(p.len(), self.d + 4, "data packets must match the erase packet's block");
            self.last_rx = i as u16;
            let n = p.len();
            if crc16_modbus(&p[..n - 2]) != u16::from_le_bytes([p[n - 2], p[n - 1]]) {
                (self.error, self.state) = (1, 3);
                self.errors += 1;
                return Ok(());
            }
            if i as u16 != self.last_ok.wrapping_add(1) {
                (self.error, self.state) = (2, 3);
                self.errors += 1;
                return Ok(());
            }
            self.errors = 0;
            self.last_ok = i as u16;
            self.state = 3;
            self.buf.extend_from_slice(&p[2..n - 2]);
            Ok(())
        }

        async fn read(&mut self, _ch: u8) -> Result<Vec<u8>> {
            self.trace.push("r".into());
            if self.fail_reads {
                return Ok(vec![0, 4, 0, 0, 0, 0, 0, 0, 0]);
            }
            if self.state == 1 {
                self.erasing -= 1;
                if self.erasing == 0 {
                    self.state = 2;
                }
            }
            if self.state == 4 {
                let ok = self.last_ok as usize == packet_count(self.img.len(), self.d) - 1
                    && self.buf[..self.img.len()] == self.img[..];
                return Ok(vec![4, if ok { 0 } else { 1 }, 0, 0, 0, 0, 0, 100, 0]);
            }
            let mut r = vec![self.state, self.error];
            r.extend_from_slice(&self.last_ok.to_le_bytes());
            r.extend_from_slice(&self.last_rx.to_le_bytes());
            r.extend_from_slice(&[self.errors, 0, 0]);
            self.error = 0;
            Ok(r)
        }

        async fn sleep(&mut self, ms: u64) {
            self.trace.push(format!("s {ms}"));
        }
    }

    async fn run(d: usize, damage: &[usize], drop: &[usize]) -> (FakeStock, Vec<String>, Result<Vec<u8>>) {
        let img = img();
        let mut f = FakeStock::new(&img, d, damage, drop);
        let mut logs = vec![];
        let o = OtaOptions { chunk: d, ..Default::default() };
        let r = run_ota(&mut f, &img, &o, &mut |s| logs.push(s.to_string())).await;
        (f, logs, r)
    }

    #[tokio::test]
    async fn a_clean_transfer() {
        let (f, logs, r) = run(16, &[], &[]).await;
        assert_eq!(r.unwrap(), [4, 0, 0, 0, 0, 0, 0, 100, 0]);
        assert_eq!(f.buf.len(), 94 * 16);
        assert!(!logs.iter().any(|l| l.contains("resend")));
    }

    #[tokio::test]
    async fn resend_after_a_crc_error_as_the_python() {
        let (f, logs, r) = run(16, &[37], &[]).await;
        assert_eq!(hex(&r.unwrap()), "040000000000006400");
        assert_eq!(f.trace.len(), 116);
        assert_eq!(f.trace_hash(), "a0fb1e1994c2848311c0cee5a7a62a7010b280a59ebbe47264d46401ab56eea3");
        assert!(logs.contains(&"bulb reports CRC error after block 39 (state 3, CRC error, last written block 36, last received 37, consecutive errors 1): resending from block 37 (resend 1)".to_string()));
        assert!(logs.contains(&"resends: 1, packets sent again: 3".to_string()));
    }

    #[tokio::test]
    async fn sequence_and_crc_errors_with_40_byte_blocks_as_the_python() {
        let (f, logs, r) = run(40, &[10], &[5]).await;
        assert!(r.is_ok());
        assert_eq!(f.trace.len(), 56);
        assert_eq!(f.trace_hash(), "e0d5040f92646423ec36ac57b5ba500f66c10e138e5bddea8fd74b3c8b009730");
        assert!(logs.iter().any(|l| l.starts_with("bulb reports sequence error after block 7") && l.ends_with("resending from block 5 (resend 1)")));
    }

    #[tokio::test]
    async fn resend_at_the_end_packet_as_the_python() {
        let (f, logs, r) = run(16, &[3, 50, 93], &[20]).await;
        assert!(r.is_ok());
        assert_eq!(f.trace.len(), 131);
        assert_eq!(f.trace_hash(), "ad4cd42827ab076bd2c2422ecbbb372389b1d15b484614a1dd154e953558df48");
        assert!(logs.iter().any(|l| l.starts_with("bulb reports CRC error at the end packet")));
        assert!(logs.contains(&"resends: 4, packets sent again: 15".to_string()));
    }

    #[tokio::test]
    async fn gives_up_after_max_resends_without_progress() {
        let img = img();
        let mut f = FakeStock::new(&img, 16, &[37; 20], &[]);
        let o = OtaOptions { max_resends: 3, ..Default::default() };
        let e = run_ota(&mut f, &img, &o, &mut |_| {}).await.unwrap_err().to_string();
        assert!(e.contains("3 resends from block 37 without progress"), "{e}");
    }

    #[tokio::test]
    async fn a_refusing_bulb_aborts() {
        let img = img();
        let mut f = FakeStock::new(&img, 16, &[], &[]);
        f.fail_reads = false;
        let o = OtaOptions::default();
        struct Refuse(FakeStock, u32);
        impl Transport for Refuse {
            async fn write(&mut self, ch: u8, p: &[u8]) -> Result<()> {
                self.0.write(ch, p).await
            }
            async fn read(&mut self, ch: u8) -> Result<Vec<u8>> {
                self.1 += 1;
                self.0.fail_reads = self.1 > 2;
                self.0.read(ch).await
            }
            async fn sleep(&mut self, _ms: u64) {}
        }
        let mut t = Refuse(f, 0);
        let e = run_ota(&mut t, &img, &o, &mut |_| {}).await.unwrap_err().to_string();
        assert!(e.starts_with("bulb aborted OTA at packet 7"), "{e}");
    }

    #[tokio::test]
    async fn erase_timeout() {
        struct Never;
        impl Transport for Never {
            async fn write(&mut self, _: u8, _: &[u8]) -> Result<()> {
                Ok(())
            }
            async fn read(&mut self, _: u8) -> Result<Vec<u8>> {
                Ok(vec![1, 0, 0xff, 0xff, 0xff, 0xff, 0, 0, 0])
            }
            async fn sleep(&mut self, _ms: u64) {}
        }
        let e = run_ota(&mut Never, &img(), &OtaOptions::default(), &mut |_| {}).await.unwrap_err();
        assert!(e.to_string().starts_with("erase did not finish"));
    }

    #[tokio::test]
    async fn login_over_a_transport() {
        struct Pair(Vec<Vec<u8>>);
        impl Transport for Pair {
            async fn write(&mut self, ch: u8, p: &[u8]) -> Result<()> {
                assert_eq!(ch, PAIR);
                self.0.push(p.to_vec());
                Ok(())
            }
            async fn read(&mut self, _: u8) -> Result<Vec<u8>> {
                Ok([0x0d, 0x11, 0x12, 0x13, 0x14, 0x15, 0x16, 0x17, 0x18].to_vec())
            }
            async fn sleep(&mut self, _ms: u64) {}
        }
        let pw = secure_password(&MAC6);
        let mut t = Pair(vec![]);
        let k = login(&mut t, b"unpaired", &pw, &RND8, &mut |_| {}).await.unwrap();
        assert_eq!(hex(&t.0[0]), "0c010203040506070819e277313fa22e08");
        assert_eq!(hex(&k), "50d0bf84818716cc0527cf88c2c12c71");
    }
}
