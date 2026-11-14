//! A scenario session: one emulated bulb joined to the coordinator, and what a
//! test uses to drive and observe it. A port of `Session` in
//! emulator/spec/harness.py.
//!
//! The bulb is observed only from the outside: the frames it puts on the air
//! (decoded by a sniffer that learns keys and addresses as they go by) and the
//! state of its light outputs (the PWM registers of the chip). Nothing reads
//! the firmware's own variables, so a session works on any image.

use std::any::Any;
use std::collections::HashMap;
use std::rc::Rc;

use zbnode::Node;

use crate::air::{AirFrame, Log, NodeId, World};
use crate::coord::{Coord, NWK_KEY, TCLK_DEFAULT};
use crate::station::{Sent, Tx, EP};
use crate::frames::{
    aps_parse, mac_parse, nwk_parse, read_attr_records, zcl_parse, Aps, Mac, Nwk, Zcl, ZclValue, APS_ACK, APS_CMD,
    APS_DATA, NWK_CMD, NWK_DATA,
};
use crate::image::Image;

/// How a test ends when it does not pass.
#[derive(Clone, Debug)]
pub enum Stop {
    Fail(String),
    Inconclusive(String),
    NotApplicable(String),
    /// The bulb is no longer usable for further tests in this session.
    SessionLost(String),
    /// The emulation itself failed.
    Error(String),
}

pub type R<T> = Result<T, Stop>;

pub fn expect(cond: bool, observed: impl Into<String>) -> R<()> {
    if cond {
        Ok(())
    } else {
        Err(Stop::Fail(observed.into()))
    }
}

/// A frame as the sniffer saw it.
#[derive(Clone, Debug)]
pub struct CapFrame {
    pub i: usize,
    pub t0: f64,
    pub t1: f64,
    pub ch: i64,
    pub who: NodeId,
    pub who_name: String,
    pub psdu: Rc<Vec<u8>>,
    pub mac: Option<Mac>,
    pub nwk: Option<Nwk>,
    pub aps: Option<Aps>,
}

/// A ZDO frame: sequence number, cluster, payload after the sequence number.
#[derive(Clone, Debug)]
pub struct Zdo {
    pub seq: u8,
    pub cluster: u16,
    pub payload: Vec<u8>,
}

impl CapFrame {
    pub fn zcl(&self) -> Option<Zcl> {
        let a = self.aps.as_ref()?;
        if a.ty == APS_DATA && a.profile.is_some_and(|p| p != 0) && a.frag.unwrap_or(0) == 0 {
            return zcl_parse(a.payload.as_ref()?);
        }
        None
    }
    pub fn zdo(&self) -> Option<Zdo> {
        let a = self.aps.as_ref()?;
        let p = a.payload.as_ref()?;
        if a.ty == APS_DATA && a.profile == Some(0) && !p.is_empty() {
            return Some(Zdo { seq: p[0], cluster: a.cluster?, payload: p[1..].to_vec() });
        }
        None
    }
    pub fn nwk_cmd(&self) -> Option<u8> {
        self.nwk.as_ref().filter(|n| n.ty == NWK_CMD).and_then(|n| n.cmd)
    }
    pub fn aps_cmd(&self) -> Option<u8> {
        self.aps.as_ref().filter(|a| a.ty == APS_CMD).and_then(|a| a.cmd)
    }
    pub fn mac_cmd(&self) -> Option<u8> {
        self.mac.as_ref().filter(|m| m.ty == 3).and_then(|m| m.payload.first().copied())
    }
    /// One line about the frame, for failure messages (`Frame.brief`, the same text).
    pub fn brief(&self) -> String {
        let who = &self.who_name;
        let hex = |b: &[u8]| b.iter().map(|x| format!("{x:02x}")).collect::<String>();
        let opt = |v: Option<String>| v.unwrap_or_else(|| "None".into());
        let Some(m) = &self.mac else { return format!("{who}: undecodable {}", hex(&self.psdu)) };
        match m.ty {
            2 => return format!("{who}: MAC ACK seq={}", m.seq),
            0 => return format!("{who}: beacon"),
            3 => {
                return match m.payload.first() {
                    Some(c) => format!("{who}: MAC cmd {c:#04x}"),
                    None => format!("{who}: MAC cmd"),
                }
            }
            _ => {}
        }
        let Some(n) = &self.nwk else { return format!("{who}: MAC data {}", hex(&m.payload)) };
        let head = format!("{who}: {:#06x}->{:#06x} r={} seq={}", n.src, n.dst, n.radius, n.seq);
        let Some(pl) = &n.payload else { return head + " (NWK payload not decrypted)" };
        if n.ty == NWK_CMD {
            let c = pl.first().copied().unwrap_or(0);
            let name = nwk_cmd_name(c).map(String::from).unwrap_or_else(|| format!("{c:#x}"));
            return format!("{head} NWK {name} {}", hex(pl.get(1..).unwrap_or(&[])));
        }
        let Some(a) = &self.aps else { return format!("{head} NWK data {}", hex(pl)) };
        if a.ty == APS_ACK {
            return format!("{head} APS ack ctr={}", opt(a.counter.map(|c| c.to_string())));
        }
        if a.ty == APS_CMD {
            let p = a.payload.as_deref().map(hex).unwrap_or_else(|| "(not decrypted)".into());
            let name = a.cmd.and_then(aps_cmd_name).unwrap_or("cmd");
            return format!("{head} APS {name} {p}");
        }
        if a.profile == Some(0) {
            if let Some(p) = &a.payload {
                return format!("{head} ZDO {:#06x} {}", a.cluster.unwrap_or(0), hex(p));
            }
        }
        if let Some(z) = self.zcl() {
            return format!(
                "{head} ZCL cl={:#06x} ep{}->{} {} cmd={:#04x} seq={} {}",
                a.cluster.unwrap_or(0),
                opt(a.src_ep.map(|e| e.to_string())),
                opt(a.dst_ep.map(|e| e.to_string())),
                if z.specific { "spec" } else { "gen" },
                z.cmd,
                z.seq,
                hex(&z.payload)
            );
        }
        format!(
            "{head} APS data cl={} {}",
            opt(a.cluster.map(|c| c.to_string())),
            a.payload.as_deref().map(hex).unwrap_or_else(|| "(not decrypted)".into())
        )
    }
}

pub fn nwk_cmd_name(c: u8) -> Option<&'static str> {
    Some(match c {
        0x01 => "Route Request",
        0x02 => "Route Reply",
        0x03 => "Network Status",
        0x04 => "Leave",
        0x05 => "Route Record",
        0x06 => "Rejoin Request",
        0x07 => "Rejoin Response",
        0x08 => "Link Status",
        0x09 => "Network Report",
        0x0A => "Network Update",
        0x0B => "End Device Timeout Request",
        0x0C => "End Device Timeout Response",
        0x0D => "Link Power Delta",
        _ => return None,
    })
}

pub fn aps_cmd_name(c: u8) -> Option<&'static str> {
    Some(match c {
        0x05 => "Transport Key",
        0x06 => "Update Device",
        0x07 => "Remove Device",
        0x08 => "Request Key",
        0x09 => "Switch Key",
        0x0E => "Tunnel",
        0x0F => "Verify Key",
        0x10 => "Confirm Key",
        _ => return None,
    })
}

// ---- predicates for Session::wait / frames / collect (as in harness.py)
pub fn is_nwk_cmd(cmd: u8) -> impl Fn(&CapFrame) -> bool {
    move |f| f.nwk_cmd() == Some(cmd)
}

pub fn is_aps_cmd(cmd: u8) -> impl Fn(&CapFrame) -> bool {
    move |f| f.aps_cmd() == Some(cmd)
}

pub fn is_mac_cmd(cmd: u8) -> impl Fn(&CapFrame) -> bool {
    move |f| f.mac_cmd() == Some(cmd)
}

pub fn is_zdo(cluster: u16, seq: Option<u8>) -> impl Fn(&CapFrame) -> bool {
    move |f| f.zdo().is_some_and(|z| z.cluster == cluster && seq.map_or(true, |s| z.seq == s))
}

/// `is_zcl(cluster, cmd, seq, specific, to_client)`; None = any.
pub fn is_zcl(cluster: Option<u16>, cmd: Option<u8>, seq: Option<u8>, specific: Option<bool>, to_client: Option<bool>)
              -> impl Fn(&CapFrame) -> bool {
    move |f| {
        let Some(z) = f.zcl() else { return false };
        cluster.map_or(true, |c| f.aps.as_ref().and_then(|a| a.cluster) == Some(c))
            && cmd.map_or(true, |c| z.cmd == c)
            && seq.map_or(true, |s| z.seq == s)
            && specific.map_or(true, |s| z.specific == s)
            && to_client.map_or(true, |t| z.to_client == t)
    }
}

pub fn is_data_from(src: u16) -> impl Fn(&CapFrame) -> bool {
    move |f| f.nwk.as_ref().is_some_and(|n| n.ty == NWK_DATA && n.src == src)
}

/// What a cluster command got back.
#[derive(Clone, Debug)]
pub enum CmdAnswer {
    None,
    /// the status of a Default Response
    Status(u8),
    /// a cluster-specific response instead
    Frame(Box<CapFrame>),
}

/// The state of the light outputs (`zigbee_sim.light`): PWM0/PC1 is the only
/// dimm channel and the TW warm one, PWM5/PB5 the TW cold one.
#[derive(Clone, Debug)]
pub struct Light {
    pub on: bool,
    pub pwm_en: bool,
    pub cmp: u32,
    pub max: u32,
    /// both channels relative to the period, in %
    pub duty: f64,
    pub duty0: f64,
    pub cmp5: u32,
    pub max5: u32,
    pub pwm5_en: bool,
    pub duty5: f64,
}

pub struct SessionOpts {
    pub cpi: i64,
    pub channel: i64,
    /// join the bulb before the tests
    pub join: bool,
    pub log: Option<Log>,
    /// changes to the Trust Center before it starts (`define_session(tc=...)`)
    pub tc: Option<fn(&mut Coord)>,
}

impl Default for SessionOpts {
    fn default() -> Self {
        SessionOpts { cpi: 3, channel: 11, join: true, log: None, tc: None }
    }
}

/// The simple descriptor of an endpoint.
#[derive(Clone, Debug)]
pub struct SimpleDesc {
    pub ep: u8,
    pub profile: u16,
    pub device: u16,
    pub version: u8,
    pub inc: Vec<u16>,
    pub outc: Vec<u16>,
}

pub struct Session {
    pub w: World,
    pub image: String,
    pub cap: Vec<CapFrame>,
    n_raw: usize,
    pub dut_short: u16,
    pub dut_ext: u64,
    pub joined: bool,
    pub t_joined: f64,
    pub join_mark: usize,
    pub last_tx: Option<Sent>,
    /// active endpoints, once asked
    pub eps: Option<Vec<u8>>,
    sd: HashMap<u8, Option<SimpleDesc>>,
    /// scratch space shared by the tests of a session
    notes: HashMap<String, Box<dyn Any>>,
    /// the nodes each peer hears (None: everyone)
    pub hears: Vec<Option<Vec<String>>>,
}

fn emu(e: String) -> Stop {
    Stop::Error(e)
}

impl Session {
    /// Boots `image` and (with `opts.join`) lets it join the coordinator.
    pub fn new(image: &Image, opts: SessionOpts) -> R<Session> {
        let img = image.bytes().map_err(Stop::Error)?;
        let shift = image.shift(&img);
        let node = Node::new(&img, opts.cpi, shift).map_err(emu)?;
        let mut keys = crate::frames::Keys::default();
        keys.set_nwk(0, NWK_KEY);
        keys.add_link(TCLK_DEFAULT);
        let mut coord = Coord::new(opts.channel, NWK_KEY);
        if let Some(f) = opts.tc {
            f(&mut coord);
        }
        let ext = coord.ext;
        let mut w = World::new(node, coord, keys, opts.log);
        w.air.keys.learn(Some(0x0000), Some(ext));
        w.coord.start(&mut w.air);
        let mut s = Session {
            w,
            image: image.label.clone(),
            cap: Vec::new(),
            n_raw: 0,
            dut_short: 0xFFFF,
            dut_ext: 0,
            joined: false,
            t_joined: 0.0,
            join_mark: 0,
            last_tx: None,
            eps: None,
            sd: HashMap::new(),
            notes: HashMap::new(),
            hears: Vec::new(),
        };
        if opts.join {
            s.join(60.0, 8000.0)?;
        }
        Ok(s)
    }

    pub fn now(&self) -> f64 {
        self.w.now()
    }

    fn run_to(&mut self, t: f64) -> R<()> {
        self.w.run_ms(t).map_err(emu)
    }

    /// Lets `ms` of simulated time pass.
    pub fn run(&mut self, ms: f64) -> R<()> {
        let t = self.now() + ms;
        self.run_to(t)?;
        self.sync();
        Ok(())
    }

    /// Decodes what went on the air since the last call.
    pub fn sync(&mut self) {
        while self.n_raw < self.w.air.frames.len() {
            let AirFrame { t0, t1, ch, who, psdu } = self.w.air.frames[self.n_raw].clone();
            self.n_raw += 1;
            let who_name = self.w.air.name_of(who);
            let mut f = CapFrame { i: self.cap.len(), t0, t1, ch, who, who_name, psdu, mac: None, nwk: None, aps: None };
            self.decode(&mut f);
            self.cap.push(f);
        }
    }

    fn decode(&mut self, f: &mut CapFrame) {
        let keys = &mut self.w.air.keys;
        let Some(m) = mac_parse(&f.psdu) else { return };
        if m.ty == 3 && m.payload.first() == Some(&0x02) && m.payload.len() >= 4 && m.dmode == 3 {
            let short = u16::from_le_bytes([m.payload[1], m.payload[2]]);
            if m.payload[3] == 0 {
                keys.learn(Some(short), m.dst);
            }
        }
        let is_data = m.ty == 1 && !m.payload.is_empty();
        let mac_src = m.src;
        let smode = m.smode;
        let payload = m.payload.clone();
        f.mac = Some(m);
        if !is_data {
            return;
        }
        let Some(n) = nwk_parse(&payload, keys) else { return };
        if n.ver != 2 || (n.ty != NWK_DATA && n.ty != NWK_CMD) {
            return;
        }
        if n.sec_src.is_some() && smode == 2 {
            keys.learn(mac_src.map(|s| s as u16), n.sec_src);
        }
        let (src, ty, pl) = (n.src, n.ty, n.payload.clone());
        f.nwk = Some(n);
        let Some(pl) = pl else { return };
        if ty != NWK_DATA {
            return;
        }
        let a = aps_parse(&pl, keys, Some(src));
        if let Some(a) = &a {
            if let Some(p) = &a.payload {
                if a.ty == APS_CMD && a.cmd == Some(0x05) && p.len() >= 18 {
                    if (p[1] == 0x01 || p[1] == 0x05) && p.len() >= 19 {
                        keys.set_nwk(p[18], p[2..18].try_into().unwrap());
                    } else if p[1] == 0x03 || p[1] == 0x04 {
                        keys.add_link(p[2..18].try_into().unwrap());
                    }
                }
                if a.ty == APS_DATA && a.profile == Some(0) && a.cluster == Some(0x0013) && p.len() >= 11 {
                    let short = u16::from_le_bytes([p[1], p[2]]);
                    let ext = u64::from_le_bytes(p[3..11].try_into().unwrap());
                    keys.learn(Some(short), Some(ext));
                }
            }
        }
        f.aps = a;
    }

    /// Index of the next frame to be captured.
    pub fn mark(&mut self) -> usize {
        self.sync();
        self.cap.len()
    }

    /// Frames since `since` from `who` (None: anyone) that satisfy `pred`.
    pub fn frames(&mut self, since: usize, who: Option<NodeId>, pred: impl Fn(&CapFrame) -> bool) -> Vec<CapFrame> {
        self.sync();
        self.cap[since.min(self.cap.len())..]
            .iter()
            .filter(|f| who.map_or(true, |w| f.who == w) && pred(f))
            .cloned()
            .collect()
    }

    /// Runs until a frame from `who` (None: anyone) satisfies `pred`; None after `max_ms`.
    pub fn wait(&mut self, pred: impl Fn(&CapFrame) -> bool, max_ms: f64, since: Option<usize>, who: Option<NodeId>,
                step: f64) -> R<Option<CapFrame>> {
        let mut i = match since {
            Some(i) => i,
            None => self.mark(),
        };
        let end = self.now() + max_ms;
        loop {
            self.sync();
            if let Some(f) = self.cap[i.min(self.cap.len())..].iter().find(|f| who.map_or(true, |w| f.who == w) && pred(f)) {
                return Ok(Some(f.clone()));
            }
            i = self.cap.len();
            if self.now() >= end {
                return Ok(None);
            }
            let t = end.min(self.now() + step);
            self.run_to(t)?;
        }
    }

    /// Runs until `cond` holds or `max_ms` passed (checked every `step` ms).
    pub fn run_until(&mut self, max_ms: f64, step: f64, cond: impl Fn(&Session) -> bool) -> R<bool> {
        let end = self.now() + max_ms;
        while self.now() < end {
            if cond(self) {
                return Ok(true);
            }
            let t = end.min(self.now() + step);
            self.run_to(t)?;
        }
        self.sync();
        Ok(cond(self))
    }

    pub fn light(&self) -> Light {
        light_of(&self.w.air.dut)
    }

    pub fn duty(&self) -> f64 {
        self.light().duty
    }

    /// Waits for the bulb to join: Device Announce, then the TC link key exchange if the image does one.
    pub fn join(&mut self, max_s: f64, tclk_wait_ms: f64) -> R<()> {
        let ok = self.run_until(max_s * 1000.0, 20.0, |s| s.w.coord.devices.iter().any(|d| d.announced))?;
        if !ok {
            return Err(Stop::SessionLost(format!("the bulb did not join the network within {max_s:.0} s")));
        }
        let d = self.w.coord.devices.iter().find(|d| d.announced).unwrap().clone();
        self.dut_short = d.short;
        self.dut_ext = d.ext;
        self.w.air.keys.learn(Some(d.short), Some(d.ext));
        self.joined = true;
        self.t_joined = self.now();
        self.run_until(tclk_wait_ms, 20.0, |s| s.w.coord.has_event("verify-key"))?;
        self.run(300.0)?;
        self.sync();
        self.join_mark = self.cap.len();
        Ok(())
    }

    /// Power loss and return of the bulb.
    pub fn reboot(&mut self) -> R<()> {
        self.w.reboot().map_err(emu)
    }

    // ---- scratch space
    pub fn note<T: Clone + 'static>(&self, k: &str) -> Option<T> {
        self.notes.get(k).and_then(|v| v.downcast_ref::<T>()).cloned()
    }
    pub fn set_note<T: 'static>(&mut self, k: &str, v: T) {
        self.notes.insert(k.to_string(), Box::new(v));
    }
    pub fn has_note(&self, k: &str) -> bool {
        self.notes.contains_key(k)
    }

    /// Frames from the bulb since `since`.
    pub fn dut_frames(&mut self, since: usize, pred: impl Fn(&CapFrame) -> bool) -> Vec<CapFrame> {
        self.frames(since, Some(NodeId::Dut), pred)
    }

    /// A frame from the bulb, as `wait` with the defaults (from the bulb, 5 ms steps).
    pub fn wait_dut(&mut self, pred: impl Fn(&CapFrame) -> bool, max_ms: f64, since: Option<usize>) -> R<Option<CapFrame>> {
        self.wait(pred, max_ms, since, Some(NodeId::Dut), 5.0)
    }

    /// Runs for `ms` and returns the frames from the bulb since the mark that satisfy `pred`.
    pub fn collect(&mut self, pred: impl Fn(&CapFrame) -> bool, ms: f64, since: Option<usize>) -> R<Vec<CapFrame>> {
        let i = match since {
            Some(i) => i,
            None => self.mark(),
        };
        self.run(ms)?;
        Ok(self.dut_frames(i, pred))
    }

    /// The link key the Trust Center holds for the bulb (the unique one after an exchange).
    pub fn tclk(&self) -> crate::frames::Key {
        self.w.coord.devices.iter().find(|d| d.ext == self.dut_ext).and_then(|d| d.tclk).unwrap_or(self.w.coord.tclk)
    }

    // ---- requests from the Trust Center (or a peer) and their answers
    /// Sends a ZCL command to the bulb (`o.dst` for another address, `o.sender`
    /// for a peer) and returns the answering frame (same sequence number,
    /// server to client, same cluster) or None.
    pub fn zcl(&mut self, cluster: u16, cmd: u8, payload: &[u8], o: Tx) -> R<Option<CapFrame>> {
        let i = self.mark();
        let dst = o.dst.unwrap_or(self.dut_short);
        let sender = o.sender.unwrap_or(NodeId::Coord);
        let max_ms = o.max_ms.unwrap_or(1500.0);
        let (st, air) = self.w.st(sender);
        let r = st.tx_zcl(air, dst, cluster, cmd, payload, &o);
        let seq = r.zcl_seq;
        let to_client = !o.to_client;
        let dut = self.dut_short;
        let f = self.wait(
            move |f| {
                f.zcl().is_some_and(|z| Some(z.seq) == seq && z.to_client == to_client)
                    && f.aps.as_ref().and_then(|a| a.cluster) == Some(cluster)
                    && f.nwk.as_ref().is_some_and(|n| n.src == dut)
            },
            max_ms,
            Some(i),
            Some(NodeId::Dut),
            5.0,
        )?;
        self.last_tx = Some(r);
        Ok(f)
    }

    /// Sends a ZCL frame without waiting for anything.
    pub fn tx_zcl(&mut self, cluster: u16, cmd: u8, payload: &[u8], o: Tx) -> Sent {
        let dst = o.dst.unwrap_or(self.dut_short);
        let sender = o.sender.unwrap_or(NodeId::Coord);
        let (st, air) = self.w.st(sender);
        st.tx_zcl(air, dst, cluster, cmd, payload, &o)
    }

    /// A cluster command: the status of the Default Response, a cluster-specific
    /// response frame, or nothing.
    pub fn cmd(&mut self, cluster: u16, cmd: u8, payload: &[u8]) -> R<CmdAnswer> {
        self.cmd_with(cluster, cmd, payload, Tx::new())
    }

    pub fn cmd_with(&mut self, cluster: u16, cmd: u8, payload: &[u8], o: Tx) -> R<CmdAnswer> {
        let f = self.zcl(cluster, cmd, payload, o)?;
        let Some(f) = f else { return Ok(CmdAnswer::None) };
        let z = f.zcl().unwrap();
        if !z.specific && z.cmd == 0x0B && z.payload.len() >= 2 {
            return Ok(CmdAnswer::Status(z.payload[1]));
        }
        Ok(CmdAnswer::Frame(Box::new(f)))
    }

    /// Read Attributes -> records, or None when nothing came back.
    pub fn read(&mut self, cluster: u16, attrs: &[u16], max_ms: f64) -> R<Option<Vec<(u16, u8, Option<u8>, Option<ZclValue>)>>> {
        self.read_with(cluster, attrs, Tx::new().max_ms(max_ms))
    }

    pub fn read_with(&mut self, cluster: u16, attrs: &[u16], o: Tx) -> R<Option<Vec<(u16, u8, Option<u8>, Option<ZclValue>)>>> {
        let pl: Vec<u8> = attrs.iter().flat_map(|a| a.to_le_bytes()).collect();
        let f = self.zcl(cluster, 0x00, &pl, o.general())?;
        let Some(f) = f else { return Ok(None) };
        let z = f.zcl().unwrap();
        if z.specific || z.cmd != 0x01 {
            return Ok(None);
        }
        Ok(read_attr_records(&z.payload).ok())
    }

    /// (status, type, value) of one attribute; all None when nothing came back.
    pub fn read1(&mut self, cluster: u16, attr: u16) -> R<(Option<u8>, Option<u8>, Option<ZclValue>)> {
        self.read1_with(cluster, attr, Tx::new())
    }

    pub fn read1_with(&mut self, cluster: u16, attr: u16, o: Tx) -> R<(Option<u8>, Option<u8>, Option<ZclValue>)> {
        let r = self.read_with(cluster, &[attr], o)?;
        Ok(match r.and_then(|v| v.into_iter().find(|x| x.0 == attr)) {
            Some((_, st, t, v)) => (Some(st), t, v),
            None => (None, None, None),
        })
    }

    /// The value of one attribute (None when nothing came back or no value).
    pub fn value(&mut self, cluster: u16, attr: u16) -> R<Option<ZclValue>> {
        Ok(self.read1(cluster, attr)?.2)
    }

    /// Write Attributes; records = [(id, type, value bytes)]. Returns the response frame.
    pub fn write(&mut self, cluster: u16, records: &[(u16, u8, Vec<u8>)]) -> R<Option<CapFrame>> {
        self.write_with(cluster, records, false, false, Tx::new())
    }

    pub fn write_with(&mut self, cluster: u16, records: &[(u16, u8, Vec<u8>)], undivided: bool, no_response: bool,
                      o: Tx) -> R<Option<CapFrame>> {
        let mut pl = Vec::new();
        for (a, t, v) in records {
            pl.extend_from_slice(&a.to_le_bytes());
            pl.push(*t);
            pl.extend_from_slice(v);
        }
        let cmd = if no_response {
            0x05
        } else if undivided {
            0x03
        } else {
            0x02
        };
        self.zcl(cluster, cmd, &pl, o.general())
    }

    /// A ZDO request; the response frame (cluster | 0x8000, same sequence number) or None.
    pub fn zdo(&mut self, cluster: u16, payload: &[u8], max_ms: f64) -> R<Option<CapFrame>> {
        self.zdo_with(cluster, payload, Tx::new().max_ms(max_ms))
    }

    pub fn zdo_with(&mut self, cluster: u16, payload: &[u8], o: Tx) -> R<Option<CapFrame>> {
        let i = self.mark();
        let dst = o.dst.unwrap_or(self.dut_short);
        let sender = o.sender.unwrap_or(NodeId::Coord);
        let max_ms = o.max_ms.unwrap_or(2000.0);
        let (st, air) = self.w.st(sender);
        let r = st.tx_zdo(air, dst, cluster, payload, &o);
        let seq = r.zdo_seq;
        let dut = self.dut_short;
        self.wait(
            move |f| {
                f.zdo().is_some_and(|z| z.cluster == (cluster | 0x8000) && Some(z.seq) == seq)
                    && f.nwk.as_ref().is_some_and(|n| n.src == dut)
            },
            max_ms,
            Some(i),
            Some(NodeId::Dut),
            5.0,
        )
    }

    /// Does the bulb still answer a Read Attributes (Basic ZCLVersion)?
    pub fn alive(&mut self) -> R<bool> {
        for _ in 0..2 {
            if self.read(0x0000, &[0x0000], 2500.0)?.is_some() {
                return Ok(true);
            }
        }
        Ok(false)
    }

    /// Light on at full level, no transition; used between tests of a shared session.
    pub fn baseline(&mut self) -> R<()> {
        self.tx_zcl(0x0006, 0x01, &[], Tx::new().ddr());
        self.run(150.0)?;
        self.tx_zcl(0x0008, 0x04, &[254, 0, 0], Tx::new().ddr());
        self.run(400.0)
    }

    /// Active endpoints of the bulb.
    pub fn endpoints(&mut self) -> R<Vec<u8>> {
        if let Some(e) = &self.eps {
            return Ok(e.clone());
        }
        let dst = self.dut_short.to_le_bytes();
        let f = self.zdo(0x0005, &dst, 2000.0)?;
        let Some(f) = f else { return Err(Stop::Inconclusive("no Active_EP_rsp".into())) };
        let p = f.zdo().unwrap().payload;
        if p.first() != Some(&0) || p.len() < 4 {
            return Err(Stop::Inconclusive("no Active_EP_rsp".into()));
        }
        let n = p[3] as usize;
        let eps = p.get(4..4 + n).unwrap_or(&[]).to_vec();
        self.eps = Some(eps.clone());
        Ok(eps)
    }

    /// The simple descriptor of an endpoint, or None when the bulb answers with an error status.
    pub fn simple_desc(&mut self, ep: u8) -> R<Option<SimpleDesc>> {
        if let Some(d) = self.sd.get(&ep) {
            return Ok(d.clone());
        }
        let mut q = self.dut_short.to_le_bytes().to_vec();
        q.push(ep);
        let Some(f) = self.zdo(0x0004, &q, 2000.0)? else { return Err(Stop::Inconclusive("no Simple_Desc_rsp".into())) };
        let p = f.zdo().unwrap().payload;
        let d = if p.first() != Some(&0) {
            None
        } else {
            let d = &p[4..];
            let u16_at = |i: usize| u16::from_le_bytes([d[i], d[i + 1]]);
            let nin = d[6] as usize;
            let inc: Vec<u16> = (0..nin).map(|k| u16_at(7 + 2 * k)).collect();
            let nout = d[7 + 2 * nin] as usize;
            let outc: Vec<u16> = (0..nout).map(|k| u16_at(8 + 2 * nin + 2 * k)).collect();
            Some(SimpleDesc { ep: d[0], profile: u16_at(1), device: u16_at(3), version: d[5] & 0xF, inc, outc })
        };
        self.sd.insert(ep, d.clone());
        Ok(d)
    }

    pub fn has_cluster(&mut self, cluster: u16) -> R<bool> {
        Ok(self.simple_desc(EP)?.is_some_and(|d| d.inc.contains(&cluster)))
    }

    pub fn need_cluster(&mut self, cluster: u16) -> R<()> {
        if !self.has_cluster(cluster)? {
            return Err(Stop::NotApplicable(format!(
                "endpoint {EP} has no server cluster {cluster:#06x} in its simple descriptor"
            )));
        }
        Ok(())
    }
}

/// The light outputs of any emulated bulb (`zigbee_sim.light`).
pub fn light_of(node: &Node) -> Light {
    let g = &node.m.regs;
    let u16le = |a: usize| u16::from_le_bytes([g[a], g[a + 1]]) as u32;
    let en = g[0x781] & 1 != 0;
    let c = u16le(0x794);
    let x = u16le(0x796);
    let pin_pwm = g[0x596] & 0x02 == 0;
    let c5_raw = u16le(0x7a8);
    let x5 = u16le(0x7aa);
    let pin5_pwm = g[0x58e] & 0x20 == 0 && g[0x5ab] & 0x0c == 0x04;
    let en5 = g[0x780] & 0x20 != 0 && pin5_pwm;
    let c5 = if en5 { c5_raw } else { 0 };
    let pct = |a: u32, b: u32| if b != 0 { 100.0 * a as f64 / b as f64 } else { 0.0 };
    Light {
        on: en && pin_pwm && c + c5 > 0,
        pwm_en: en,
        cmp: c,
        max: x,
        duty: pct(c + c5, x),
        duty0: pct(c, x),
        cmp5: c5,
        max5: x5,
        pwm5_en: en5,
        duty5: pct(c5, x5),
    }
}
