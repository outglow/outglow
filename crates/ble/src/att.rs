use crate::hal::{Hal, LL_PAYLOAD_MAX};
use crate::ll::TX_RESERVE;
use crate::pdu;
use crate::Ble;

pub(crate) const CID_ATT: u16 = 4;
pub(crate) const CID_SIGNALING: u16 = 5;
const CID_SMP: u16 = 6;

pub(crate) const MTU_MAX: usize = 64;
const MTU_DEFAULT: usize = 23;
const FRAME_MAX: usize = MTU_MAX + 4;
const SIG_MTU: u16 = MTU_MAX as u16;

const ERROR_RSP: u8 = 0x01;
const MTU_REQ: u8 = 0x02;
const MTU_RSP: u8 = 0x03;
const FIND_INFO_REQ: u8 = 0x04;
const FIND_INFO_RSP: u8 = 0x05;
const FIND_BY_TYPE_VALUE_REQ: u8 = 0x06;
const FIND_BY_TYPE_VALUE_RSP: u8 = 0x07;
const READ_BY_TYPE_REQ: u8 = 0x08;
const READ_REQ: u8 = 0x0a;
const READ_BLOB_REQ: u8 = 0x0c;
const READ_BY_GROUP_TYPE_REQ: u8 = 0x10;
const WRITE_REQ: u8 = 0x12;
const WRITE_RSP: u8 = 0x13;
const NOTIFICATION: u8 = 0x1b;
const CONFIRMATION: u8 = 0x1e;
const WRITE_CMD: u8 = 0x52;

const ERR_INVALID_HANDLE: u8 = 0x01;
const ERR_READ_NOT_PERMITTED: u8 = 0x02;
const ERR_WRITE_NOT_PERMITTED: u8 = 0x03;
const ERR_INVALID_PDU: u8 = 0x04;
const ERR_REQUEST_NOT_SUPPORTED: u8 = 0x06;
const ERR_INVALID_OFFSET: u8 = 0x07;
const ERR_ATTRIBUTE_NOT_FOUND: u8 = 0x0a;
const ERR_INVALID_LENGTH: u8 = 0x0d;
const ERR_UNSUPPORTED_GROUP_TYPE: u8 = 0x10;

const PRIMARY_SERVICE: u16 = 0x2800;
const CHARACTERISTIC: u16 = 0x2803;
const USER_DESCRIPTION: u16 = 0x2901;
const CLIENT_CONFIGURATION: u16 = 0x2902;

const fn telink(short: u8) -> [u8; 16] {
    [short, 0x19, 0x0d, 0x0c, 0x0b, 0x0a, 0x09, 0x08, 0x07, 0x06, 0x05, 0x04, 0x03, 0x02, 0x01, 0x00]
}

const fn diag_uuid(short: u8) -> [u8; 16] {
    [0x6f, 0x5e, 0x4d, 0x3c, 0x2b, 0x1a, 0x2f, 0x8e, 0x3d, 0x4c, 0x4b, 0x5a, short, 0x00, 0x10, 0xe6]
}

const fn declaration(properties: u8, value_handle: u8, uuid: [u8; 16]) -> [u8; 19] {
    let mut d = [0u8; 19];
    d[0] = properties;
    d[1] = value_handle;
    let mut i = 0;
    while i < 16 {
        d[3 + i] = uuid[i];
        i += 1;
    }
    d
}

const UUID_MESH: [u8; 16] = telink(0x10);
const UUID_STATUS: [u8; 16] = telink(0x11);
const UUID_COMMAND: [u8; 16] = telink(0x12);
const UUID_OTA_CLASSIC: [u8; 16] = telink(0x13);
const UUID_PAIR: [u8; 16] = telink(0x14);
const UUID_OTA: [u8; 16] = telink(0x15);

const UUID_DIAG: [u8; 16] = diag_uuid(0x00);
const UUID_DIAG_INFO: [u8; 16] = diag_uuid(0x01);
const UUID_DIAG_STATE: [u8; 16] = diag_uuid(0x02);
const UUID_DIAG_LOG: [u8; 16] = diag_uuid(0x03);
const UUID_DIAG_CMD: [u8; 16] = diag_uuid(0x04);
const UUID_DIAG_FRAMES: [u8; 16] = diag_uuid(0x05);
const UUID_DIAG_CONFIG: [u8; 16] = diag_uuid(0x06);
const UUID_DIAG_COUNTERS: [u8; 16] = diag_uuid(0x07);

const READ: u8 = 0x02;
const WRITE_NO_RSP: u8 = 0x04;
const WRITE: u8 = 0x08;
const NOTIFY: u8 = 0x10;
const CCCD_NOTIFY: u8 = 0x01;

const DECL_STATUS: [u8; 19] = declaration(READ | WRITE | NOTIFY, HANDLE_STATUS as u8, UUID_STATUS);
const DECL_COMMAND: [u8; 19] = declaration(READ | WRITE | WRITE_NO_RSP, HANDLE_COMMAND as u8, UUID_COMMAND);
const DECL_OTA_CLASSIC: [u8; 19] = declaration(READ | WRITE, HANDLE_OTA_CLASSIC as u8, UUID_OTA_CLASSIC);
const DECL_OTA: [u8; 19] = declaration(READ | WRITE_NO_RSP, HANDLE_OTA as u8, UUID_OTA);
const DECL_PAIR: [u8; 19] = declaration(READ | WRITE, HANDLE_PAIR as u8, UUID_PAIR);
const DECL_DIAG_INFO: [u8; 19] = declaration(READ, 0x22, UUID_DIAG_INFO);
const DECL_DIAG_STATE: [u8; 19] = declaration(READ, 0x24, UUID_DIAG_STATE);
const DECL_DIAG_LOG: [u8; 19] = declaration(READ, 0x26, UUID_DIAG_LOG);
const DECL_DIAG_CMD: [u8; 19] = declaration(WRITE, 0x28, UUID_DIAG_CMD);
const DECL_DIAG_FRAMES: [u8; 19] = declaration(READ, 0x2a, UUID_DIAG_FRAMES);
const DECL_DIAG_CONFIG: [u8; 19] = declaration(READ, 0x2c, UUID_DIAG_CONFIG);
const DECL_DIAG_COUNTERS: [u8; 19] = declaration(READ, 0x2e, UUID_DIAG_COUNTERS);

pub(crate) const HANDLE_STATUS: u16 = 0x12;
pub(crate) const HANDLE_COMMAND: u16 = 0x15;
pub(crate) const HANDLE_OTA_CLASSIC: u16 = 0x18;
pub(crate) const HANDLE_OTA: u16 = 0x1b;
pub(crate) const HANDLE_PAIR: u16 = 0x1e;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Uuid {
    Short(u16),
    Long(&'static [u8; 16]),
}

impl Uuid {
    fn matches(&self, wanted: &Wanted) -> bool {
        match (self, wanted) {
            (Uuid::Short(a), Wanted::Short(b)) => a == b,
            (Uuid::Long(a), Wanted::Long(b)) => a[..] == b[..],
            (Uuid::Short(a), Wanted::Long(b)) => {
                b[..12] == [0xfb, 0x34, 0x9b, 0x5f, 0x80, 0x00, 0x00, 0x80, 0x00, 0x10, 0x00, 0x00]
                    && b[12..] == [*a as u8, (*a >> 8) as u8, 0, 0]
            }
            (Uuid::Long(_), Wanted::Short(_)) => false,
        }
    }
}

enum Wanted {
    Short(u16),
    Long([u8; 16]),
}

impl Wanted {
    fn parse(b: &[u8]) -> Option<Wanted> {
        match b.len() {
            2 => Some(Wanted::Short(u16::from_le_bytes([b[0], b[1]]))),
            16 => {
                let mut u = [0u8; 16];
                u.copy_from_slice(b);
                Some(Wanted::Long(u))
            }
            _ => None,
        }
    }
}

#[derive(Clone, Copy)]
enum Value {
    Fixed(&'static [u8]),
    Name,
    Model,
    FirmwareRevision,
    HardwareRevision,
    Status,
    StatusConfiguration,
    Command,
    Ota,
    Pair,
    DiagInfo,
    DiagState,
    DiagLog,
    DiagCommand,
    DiagFrames,
    DiagConfig,
    DiagCounters,
}

const SCRATCH: usize = crate::diag::STATE_LEN;

struct Attribute(Uuid, Value);

use Uuid::{Long, Short};
use Value::*;

static TABLE: [Attribute; 46] = [
    Attribute(Short(PRIMARY_SERVICE), Fixed(&[0x00, 0x18])),
    Attribute(Short(CHARACTERISTIC), Fixed(&[READ, 0x03, 0x00, 0x00, 0x2a])),
    Attribute(Short(0x2a00), Name),
    Attribute(Short(USER_DESCRIPTION), Fixed(b"name")),
    Attribute(Short(CHARACTERISTIC), Fixed(&[READ, 0x06, 0x00, 0x01, 0x2a])),
    Attribute(Short(0x2a01), Fixed(&[0x00, 0x00])),
    Attribute(Short(PRIMARY_SERVICE), Fixed(&[0x0a, 0x18])),
    Attribute(Short(CHARACTERISTIC), Fixed(&[READ, 0x09, 0x00, 0x26, 0x2a])),
    Attribute(Short(0x2a26), FirmwareRevision),
    Attribute(Short(CHARACTERISTIC), Fixed(&[READ, 0x0b, 0x00, 0x29, 0x2a])),
    Attribute(Short(0x2a29), Fixed(b"AwoX")),
    Attribute(Short(CHARACTERISTIC), Fixed(&[READ, 0x0d, 0x00, 0x24, 0x2a])),
    Attribute(Short(0x2a24), Model),
    Attribute(Short(CHARACTERISTIC), Fixed(&[READ, 0x0f, 0x00, 0x27, 0x2a])),
    Attribute(Short(0x2a27), HardwareRevision),
    Attribute(Short(PRIMARY_SERVICE), Fixed(&UUID_MESH)),
    Attribute(Short(CHARACTERISTIC), Fixed(&DECL_STATUS)),
    Attribute(Long(&UUID_STATUS), Status),
    Attribute(Short(CLIENT_CONFIGURATION), StatusConfiguration),
    Attribute(Short(CHARACTERISTIC), Fixed(&DECL_COMMAND)),
    Attribute(Long(&UUID_COMMAND), Command),
    Attribute(Short(USER_DESCRIPTION), Fixed(b"cmd")),
    Attribute(Short(CHARACTERISTIC), Fixed(&DECL_OTA_CLASSIC)),
    Attribute(Long(&UUID_OTA_CLASSIC), Ota),
    Attribute(Short(USER_DESCRIPTION), Fixed(b"OTA")),
    Attribute(Short(CHARACTERISTIC), Fixed(&DECL_OTA)),
    Attribute(Long(&UUID_OTA), Ota),
    Attribute(Short(USER_DESCRIPTION), Fixed(b"OTA2")),
    Attribute(Short(CHARACTERISTIC), Fixed(&DECL_PAIR)),
    Attribute(Long(&UUID_PAIR), Pair),
    Attribute(Short(USER_DESCRIPTION), Fixed(b"pair")),
    Attribute(Short(PRIMARY_SERVICE), Fixed(&UUID_DIAG)),
    Attribute(Short(CHARACTERISTIC), Fixed(&DECL_DIAG_INFO)),
    Attribute(Long(&UUID_DIAG_INFO), DiagInfo),
    Attribute(Short(CHARACTERISTIC), Fixed(&DECL_DIAG_STATE)),
    Attribute(Long(&UUID_DIAG_STATE), DiagState),
    Attribute(Short(CHARACTERISTIC), Fixed(&DECL_DIAG_LOG)),
    Attribute(Long(&UUID_DIAG_LOG), DiagLog),
    Attribute(Short(CHARACTERISTIC), Fixed(&DECL_DIAG_CMD)),
    Attribute(Long(&UUID_DIAG_CMD), DiagCommand),
    Attribute(Short(CHARACTERISTIC), Fixed(&DECL_DIAG_FRAMES)),
    Attribute(Long(&UUID_DIAG_FRAMES), DiagFrames),
    Attribute(Short(CHARACTERISTIC), Fixed(&DECL_DIAG_CONFIG)),
    Attribute(Long(&UUID_DIAG_CONFIG), DiagConfig),
    Attribute(Short(CHARACTERISTIC), Fixed(&DECL_DIAG_COUNTERS)),
    Attribute(Long(&UUID_DIAG_COUNTERS), DiagCounters),
];

const LAST_HANDLE: u16 = TABLE.len() as u16;

fn attribute(handle: u16) -> Option<&'static Attribute> {
    TABLE.get((handle as usize).checked_sub(1)?)
}

fn attributes(start: u16, end: u16) -> impl Iterator<Item = (u16, &'static Attribute)> {
    (1..).zip(&TABLE).skip(start as usize - 1).take_while(move |(handle, _)| *handle <= end)
}

fn is_service(handle: u16) -> bool {
    attribute(handle).is_some_and(|a| a.0 == Short(PRIMARY_SERVICE))
}

fn group_end(handle: u16) -> u16 {
    (handle + 1..=LAST_HANDLE).find(|h| is_service(*h)).map_or(LAST_HANDLE, |h| h - 1)
}

pub(crate) struct Host {
    frame: [u8; FRAME_MAX],
    have: usize,
    want: usize,
    open: bool,
    pub mtu: usize,
    configuration: [u8; 2],
}

impl Host {
    pub fn notifications_enabled(&self) -> bool {
        self.configuration[0] & CCCD_NOTIFY != 0
    }

    pub fn new() -> Host {
        Host { frame: [0; FRAME_MAX], have: 0, want: 0, open: false, mtu: MTU_DEFAULT, configuration: [0; 2] }
    }
}

struct Out {
    buf: [u8; MTU_MAX],
    len: usize,
    limit: usize,
}

impl Out {
    fn new(limit: usize) -> Out {
        Out { buf: [0; MTU_MAX], len: 0, limit: limit.min(MTU_MAX) }
    }

    fn room(&self) -> usize {
        self.limit - self.len
    }

    fn put(&mut self, bytes: &[u8]) {
        let n = bytes.len().min(self.room());
        self.buf[self.len..self.len + n].copy_from_slice(&bytes[..n]);
        self.len += n;
    }

    fn bytes(&self) -> &[u8] {
        &self.buf[..self.len]
    }
}

fn u16_at(p: &[u8], i: usize) -> u16 {
    u16::from_le_bytes([p[i], p[i + 1]])
}

impl<H: Hal> Ble<H> {
    pub(crate) fn host_reset(&mut self) {
        self.host = Host::new();
    }

    pub(crate) fn host_poll(&mut self) {
        self.link_idle();
    }

    pub(crate) fn send_l2cap(&mut self, cid: u16, data: &[u8]) {
        let mut frame = [0u8; FRAME_MAX];
        let n = data.len().min(MTU_MAX);
        frame[..2].copy_from_slice(&(n as u16).to_le_bytes());
        frame[2..4].copy_from_slice(&cid.to_le_bytes());
        frame[4..4 + n].copy_from_slice(&data[..n]);
        for (i, fragment) in frame[..4 + n].chunks(LL_PAYLOAD_MAX).enumerate() {
            self.hal.tx_push(if i == 0 { pdu::LLID_START } else { pdu::LLID_CONTINUE }, fragment);
        }
    }

    pub(crate) fn notify_status(&mut self, value: &[u8; 20]) -> bool {
        if self.hal.tx_free() < TX_RESERVE {
            return false;
        }
        let mut pdu = [0u8; 23];
        pdu[0] = NOTIFICATION;
        pdu[1..3].copy_from_slice(&HANDLE_STATUS.to_le_bytes());
        pdu[3..].copy_from_slice(value);
        self.send_l2cap(CID_ATT, &pdu);
        true
    }

    pub(crate) fn on_l2cap_fragment(&mut self, start: bool, data: &[u8]) {
        let h = &mut self.host;
        if start {
            h.have = 0;
            h.open = true;
        } else if !h.open {
            return;
        }
        let at = h.have.min(FRAME_MAX);
        let n = data.len().min(FRAME_MAX - at);
        h.frame[at..at + n].copy_from_slice(&data[..n]);
        h.have += data.len();
        if h.have < 4 {
            return;
        }
        h.want = 4 + u16_at(&h.frame, 0) as usize;
        if h.have < h.want {
            return;
        }
        h.open = false;
        if h.have > h.want {
            return;
        }
        let frame = h.frame;
        let len = h.want;
        h.have = 0;
        let cid = u16_at(&frame, 2);
        if len > FRAME_MAX {
            if cid == CID_SIGNALING {
                let mut reject = [0x01, frame[5], 0x04, 0x00, 0x01, 0x00, 0, 0];
                reject[6..].copy_from_slice(&SIG_MTU.to_le_bytes());
                self.send_l2cap(CID_SIGNALING, &reject);
            }
            return;
        }
        let payload = &frame[4..len];
        match cid {
            CID_ATT => self.on_att(payload),
            CID_SIGNALING => self.on_signaling(payload),
            CID_SMP if payload.first().is_some_and(|code| (0x01..=0x0e).contains(code) && *code != 0x05) => {
                self.send_l2cap(CID_SMP, &[0x05, 0x05]);
            }
            _ => {}
        }
    }

    fn on_signaling(&mut self, p: &[u8]) {
        if p.len() < 4 {
            return;
        }
        match p[0] {
            0x01 | 0x13 | 0x15 | 0x16 | 0x18 | 0x1a => {}
            _ => {
                self.send_l2cap(CID_SIGNALING, &[0x01, p[1], 0x02, 0x00, 0x00, 0x00]);
            }
        }
    }

    fn att_value<'a>(&'a self, a: &Attribute, scratch: &'a mut [u8; SCRATCH]) -> Option<&'a [u8]> {
        Some(match a.1 {
            Fixed(v) => v,
            Name => {
                let name = self.mesh.name();
                if name.is_empty() {
                    &[0]
                } else {
                    name
                }
            }
            Model => self.identity.model.as_bytes(),
            FirmwareRevision => self.identity.firmware_revision.as_bytes(),
            HardwareRevision => self.identity.hardware_revision.as_bytes(),
            Status => &self.mesh.att_status,
            StatusConfiguration => &self.host.configuration,
            Command => &self.mesh.att_command,
            Pair => &self.mesh.att_pair,
            Ota => {
                scratch[..20].copy_from_slice(&self.ota_status());
                &scratch[..20]
            }
            DiagInfo => self.diag_info(),
            DiagState => {
                let n = self.diag_state(scratch);
                &scratch[..n]
            }
            DiagLog => self.diag_log(),
            DiagFrames => self.diag_frames(),
            DiagConfig => self.diag_config(),
            DiagCounters => self.diag_counters(),
            DiagCommand => return None,
        })
    }

    fn att_write(&mut self, handle: u16, value: &[u8]) -> Result<(), u8> {
        match attribute(handle).ok_or(ERR_INVALID_HANDLE)?.1 {
            Status => self.mesh_status_write(value),
            StatusConfiguration => {
                if value.len() != 2 {
                    return Err(ERR_INVALID_LENGTH);
                }
                let was = self.mesh_status_enabled();
                self.host.configuration = [value[0] & CCCD_NOTIFY, 0];
                self.mesh_status_switched(was);
            }
            Command => self.mesh_command_write(value),
            Ota => self.ota_write(value),
            Pair => self.mesh_pair_write(value),
            DiagCommand => self.diag_write(value),
            _ => return Err(ERR_WRITE_NOT_PERMITTED),
        }
        Ok(())
    }

    fn on_att(&mut self, p: &[u8]) {
        let Some(&opcode) = p.first() else { return };
        let mut out = Out::new(self.host.mtu);
        let result = self.att_request(opcode, p, &mut out);
        match result {
            Ok(()) => {}
            Err(_) if opcode & 0x40 != 0 => return,
            Err((handle, code)) => {
                out.len = 0;
                out.put(&[ERROR_RSP, opcode]);
                out.put(&handle.to_le_bytes());
                out.put(&[code]);
            }
        }
        if out.len > 0 {
            self.send_l2cap(CID_ATT, out.bytes());
        }
    }

    fn att_request(&mut self, opcode: u8, p: &[u8], out: &mut Out) -> Result<(), (u16, u8)> {
        let need = |n: usize| if p.len() >= n { Ok(()) } else { Err((0, ERR_INVALID_PDU)) };
        let exact = |n: usize| if p.len() == n { Ok(()) } else { Err((0, ERR_INVALID_PDU)) };
        let range = |p: &[u8]| -> Result<(u16, u16), (u16, u8)> {
            let (start, end) = (u16_at(p, 1), u16_at(p, 3));
            if start == 0 || start > end {
                return Err((start, ERR_INVALID_HANDLE));
            }
            Ok((start, end.min(LAST_HANDLE)))
        };
        let mut scratch = [0u8; SCRATCH];
        match opcode {
            MTU_REQ => {
                exact(3)?;
                let theirs = u16_at(p, 1) as usize;
                out.put(&[MTU_RSP, MTU_MAX as u8, 0]);
                self.host.mtu = theirs.clamp(MTU_DEFAULT, MTU_MAX);
            }
            FIND_INFO_REQ => {
                exact(5)?;
                let (start, end) = range(p)?;
                let mut format = 0;
                for (handle, a) in attributes(start, end) {
                    let (this, len) = match a.0 {
                        Short(_) => (1, 2),
                        Long(_) => (2, 16),
                    };
                    if format == 0 {
                        format = this;
                        out.put(&[FIND_INFO_RSP, format]);
                    }
                    if this != format || out.room() < 2 + len {
                        break;
                    }
                    out.put(&handle.to_le_bytes());
                    match a.0 {
                        Short(u) => out.put(&u.to_le_bytes()),
                        Long(u) => out.put(u),
                    }
                }
                if format == 0 {
                    return Err((start, ERR_ATTRIBUTE_NOT_FOUND));
                }
            }
            FIND_BY_TYPE_VALUE_REQ => {
                need(7)?;
                let (start, end) = range(p)?;
                let kind = Wanted::Short(u16_at(p, 5));
                let wanted = &p[7..];
                out.put(&[FIND_BY_TYPE_VALUE_RSP]);
                for (handle, a) in attributes(start, end) {
                    if !a.0.matches(&kind) || self.att_value(a, &mut scratch) != Some(wanted) {
                        continue;
                    }
                    let last = if is_service(handle) { group_end(handle) } else { handle };
                    out.put(&handle.to_le_bytes());
                    out.put(&last.to_le_bytes());
                }
                if out.len == 1 {
                    return Err((start, ERR_ATTRIBUTE_NOT_FOUND));
                }
            }
            READ_BY_TYPE_REQ | READ_BY_GROUP_TYPE_REQ => {
                need(7)?;
                let kind = Wanted::parse(&p[5..]).ok_or((0, ERR_INVALID_PDU))?;
                let (start, end) = range(p)?;
                let grouped = opcode == READ_BY_GROUP_TYPE_REQ;
                if grouped && !Short(PRIMARY_SERVICE).matches(&kind) {
                    return Err((start, ERR_UNSUPPORTED_GROUP_TYPE));
                }
                let head = if grouped { 4 } else { 2 };
                let mut each = 0;
                for (handle, a) in attributes(start, end) {
                    if !a.0.matches(&kind) {
                        continue;
                    }
                    let value = self.att_value(a, &mut scratch).ok_or((handle, ERR_READ_NOT_PERMITTED))?;
                    let n = value.len();
                    if each == 0 {
                        each = n.min(out.limit - 2 - head);
                        out.put(&[opcode + 1, (head + each) as u8]);
                    } else if n != each || out.room() < head + each {
                        break;
                    }
                    out.put(&handle.to_le_bytes());
                    if grouped {
                        out.put(&group_end(handle).to_le_bytes());
                    }
                    out.put(&value[..each]);
                }
                if each == 0 {
                    return Err((start, ERR_ATTRIBUTE_NOT_FOUND));
                }
            }
            READ_REQ | READ_BLOB_REQ => {
                let blob = opcode == READ_BLOB_REQ;
                exact(if blob { 5 } else { 3 })?;
                let handle = u16_at(p, 1);
                let a = attribute(handle).ok_or((handle, ERR_INVALID_HANDLE))?;
                let value = self.att_value(a, &mut scratch).ok_or((handle, ERR_READ_NOT_PERMITTED))?;
                let n = value.len();
                let offset = if blob { u16_at(p, 3) as usize } else { 0 };
                if offset > n {
                    return Err((handle, ERR_INVALID_OFFSET));
                }
                out.put(&[opcode + 1]);
                out.put(&value[offset..n]);
            }
            WRITE_REQ | WRITE_CMD => {
                need(3)?;
                let handle = u16_at(p, 1);
                self.att_write(handle, &p[3..]).map_err(|code| (handle, code))?;
                if opcode == WRITE_REQ {
                    out.put(&[WRITE_RSP]);
                }
            }
            CONFIRMATION => {}
            _ => return Err((0, ERR_REQUEST_NOT_SUPPORTED)),
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_declaration_holds_the_properties_the_value_handle_and_the_128_bit_uuid() {
        let status = declaration(READ | WRITE | NOTIFY, 0x12, telink(0x11));
        assert_eq!(status, [0x1a, 0x12, 0x00, 0x11, 0x19, 0x0d, 0x0c, 0x0b, 0x0a, 0x09, 0x08, 0x07, 0x06, 0x05, 0x04, 0x03, 0x02, 0x01, 0x00]);
        assert_eq!(status, DECL_STATUS);
        let command = declaration(WRITE, 0x28, diag_uuid(0x04));
        assert_eq!(command, [0x08, 0x28, 0x00, 0x6f, 0x5e, 0x4d, 0x3c, 0x2b, 0x1a, 0x2f, 0x8e, 0x3d, 0x4c, 0x4b, 0x5a, 0x04, 0x00, 0x10, 0xe6]);
        assert_eq!(command, DECL_DIAG_CMD);
    }
}
