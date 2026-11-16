use crate::hal::{Hal, LL_PAYLOAD_MAX};
use crate::ll::{LinkState, TX_RESERVE};
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

const fn declaration(properties: u8, value_handle: u8, short: u8) -> [u8; 19] {
    let u = telink(short);
    let mut d = [0u8; 19];
    d[0] = properties;
    d[1] = value_handle;
    let mut i = 0;
    while i < 16 {
        d[3 + i] = u[i];
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

const fn diag_uuid(short: u8) -> [u8; 16] {
    [0x6f, 0x5e, 0x4d, 0x3c, 0x2b, 0x1a, 0x2f, 0x8e, 0x3d, 0x4c, 0x4b, 0x5a, short, 0x00, 0x10, 0xe6]
}

const fn diag_declaration(properties: u8, value_handle: u8, short: u8) -> [u8; 19] {
    let u = diag_uuid(short);
    let mut d = [0u8; 19];
    d[0] = properties;
    d[1] = value_handle;
    let mut i = 0;
    while i < 16 {
        d[3 + i] = u[i];
        i += 1;
    }
    d
}

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
const INDICATE: u8 = 0x20;

const fn cccd_bits(properties: u8) -> u8 {
    (if properties & NOTIFY != 0 { 0x01 } else { 0 }) | (if properties & INDICATE != 0 { 0x02 } else { 0 })
}

const DECL_STATUS: [u8; 19] = declaration(READ | WRITE | NOTIFY, HANDLE_STATUS as u8, 0x11);
const DECL_COMMAND: [u8; 19] = declaration(READ | WRITE | WRITE_NO_RSP, HANDLE_COMMAND as u8, 0x12);
const DECL_OTA_CLASSIC: [u8; 19] = declaration(READ | WRITE, HANDLE_OTA_CLASSIC as u8, 0x13);
const DECL_OTA: [u8; 19] = declaration(READ | WRITE_NO_RSP, HANDLE_OTA as u8, 0x15);
const DECL_PAIR: [u8; 19] = declaration(READ | WRITE, HANDLE_PAIR as u8, 0x14);
const DECL_DIAG_INFO: [u8; 19] = diag_declaration(READ, 0x22, 0x01);
const DECL_DIAG_STATE: [u8; 19] = diag_declaration(READ, 0x24, 0x02);
const DECL_DIAG_LOG: [u8; 19] = diag_declaration(READ, 0x26, 0x03);
const DECL_DIAG_CMD: [u8; 19] = diag_declaration(WRITE, 0x28, 0x04);
const DECL_DIAG_FRAMES: [u8; 19] = diag_declaration(READ, 0x2a, 0x05);
const DECL_DIAG_CONFIG: [u8; 19] = diag_declaration(READ, 0x2c, 0x06);
const DECL_DIAG_COUNTERS: [u8; 19] = diag_declaration(READ, 0x2e, 0x07);

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

struct Attribute {
    uuid: Uuid,
    value: Value,
    writable: bool,
}

const fn fixed(uuid: u16, value: &'static [u8]) -> Attribute {
    Attribute { uuid: Uuid::Short(uuid), value: Value::Fixed(value), writable: false }
}

const fn read_only(uuid: u16, value: Value) -> Attribute {
    Attribute { uuid: Uuid::Short(uuid), value, writable: false }
}

const fn mesh_value(uuid: &'static [u8; 16], value: Value) -> Attribute {
    Attribute { uuid: Uuid::Long(uuid), value, writable: true }
}

const fn diag_value(uuid: &'static [u8; 16], value: Value) -> Attribute {
    Attribute { uuid: Uuid::Long(uuid), value, writable: matches!(value, Value::DiagCommand) }
}

const fn fixed_long(value: &'static [u8; 16]) -> Attribute {
    Attribute { uuid: Uuid::Short(PRIMARY_SERVICE), value: Value::Fixed(value), writable: false }
}

static TABLE: [Attribute; 46] = [
    fixed(PRIMARY_SERVICE, &[0x00, 0x18]),
    fixed(CHARACTERISTIC, &[READ, 0x03, 0x00, 0x00, 0x2a]),
    read_only(0x2a00, Value::Name),
    fixed(USER_DESCRIPTION, b"name"),
    fixed(CHARACTERISTIC, &[READ, 0x06, 0x00, 0x01, 0x2a]),
    fixed(0x2a01, &[0x00, 0x00]),
    fixed(PRIMARY_SERVICE, &[0x0a, 0x18]),
    fixed(CHARACTERISTIC, &[READ, 0x09, 0x00, 0x26, 0x2a]),
    read_only(0x2a26, Value::FirmwareRevision),
    fixed(CHARACTERISTIC, &[READ, 0x0b, 0x00, 0x29, 0x2a]),
    fixed(0x2a29, b"AwoX"),
    fixed(CHARACTERISTIC, &[READ, 0x0d, 0x00, 0x24, 0x2a]),
    read_only(0x2a24, Value::Model),
    fixed(CHARACTERISTIC, &[READ, 0x0f, 0x00, 0x27, 0x2a]),
    read_only(0x2a27, Value::HardwareRevision),
    fixed(PRIMARY_SERVICE, &UUID_MESH),
    fixed(CHARACTERISTIC, &DECL_STATUS),
    mesh_value(&UUID_STATUS, Value::Status),
    Attribute { uuid: Uuid::Short(CLIENT_CONFIGURATION), value: Value::StatusConfiguration, writable: true },
    fixed(CHARACTERISTIC, &DECL_COMMAND),
    mesh_value(&UUID_COMMAND, Value::Command),
    fixed(USER_DESCRIPTION, b"cmd"),
    fixed(CHARACTERISTIC, &DECL_OTA_CLASSIC),
    mesh_value(&UUID_OTA_CLASSIC, Value::Ota),
    fixed(USER_DESCRIPTION, b"OTA"),
    fixed(CHARACTERISTIC, &DECL_OTA),
    mesh_value(&UUID_OTA, Value::Ota),
    fixed(USER_DESCRIPTION, b"OTA2"),
    fixed(CHARACTERISTIC, &DECL_PAIR),
    mesh_value(&UUID_PAIR, Value::Pair),
    fixed(USER_DESCRIPTION, b"pair"),
    fixed_long(&UUID_DIAG),
    fixed(CHARACTERISTIC, &DECL_DIAG_INFO),
    diag_value(&UUID_DIAG_INFO, Value::DiagInfo),
    fixed(CHARACTERISTIC, &DECL_DIAG_STATE),
    diag_value(&UUID_DIAG_STATE, Value::DiagState),
    fixed(CHARACTERISTIC, &DECL_DIAG_LOG),
    diag_value(&UUID_DIAG_LOG, Value::DiagLog),
    fixed(CHARACTERISTIC, &DECL_DIAG_CMD),
    diag_value(&UUID_DIAG_CMD, Value::DiagCommand),
    fixed(CHARACTERISTIC, &DECL_DIAG_FRAMES),
    diag_value(&UUID_DIAG_FRAMES, Value::DiagFrames),
    fixed(CHARACTERISTIC, &DECL_DIAG_CONFIG),
    diag_value(&UUID_DIAG_CONFIG, Value::DiagConfig),
    fixed(CHARACTERISTIC, &DECL_DIAG_COUNTERS),
    diag_value(&UUID_DIAG_COUNTERS, Value::DiagCounters),
];

const LAST_HANDLE: u16 = TABLE.len() as u16;

fn attribute(handle: u16) -> Option<&'static Attribute> {
    TABLE.get((handle as usize).checked_sub(1)?)
}

impl Attribute {
    fn readable(&self) -> bool {
        !matches!(self.value, Value::DiagCommand)
    }
}

fn is_service(handle: u16) -> bool {
    attribute(handle).is_some_and(|a| a.uuid == Uuid::Short(PRIMARY_SERVICE))
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
        self.configuration[0] & 0x01 != 0
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

    pub(crate) fn send_l2cap(&mut self, cid: u16, data: &[u8]) -> bool {
        let mut frame = [0u8; FRAME_MAX];
        let n = data.len().min(MTU_MAX);
        frame[..2].copy_from_slice(&(n as u16).to_le_bytes());
        frame[2..4].copy_from_slice(&cid.to_le_bytes());
        frame[4..4 + n].copy_from_slice(&data[..n]);
        let frame = &frame[..4 + n];
        if self.link.state != LinkState::Connected || self.hal.tx_free() < frame.len().div_ceil(LL_PAYLOAD_MAX) {
            return false;
        }
        for (i, fragment) in frame.chunks(LL_PAYLOAD_MAX).enumerate() {
            self.hal.tx_push(if i == 0 { pdu::LLID_START } else { pdu::LLID_CONTINUE }, fragment);
        }
        true
    }

    pub(crate) fn notify_status(&mut self, value: &[u8; 20]) -> bool {
        if self.hal.tx_free() < TX_RESERVE {
            return false;
        }
        let mut pdu = [0u8; 23];
        pdu[0] = NOTIFICATION;
        pdu[1..3].copy_from_slice(&HANDLE_STATUS.to_le_bytes());
        pdu[3..].copy_from_slice(value);
        self.send_l2cap(CID_ATT, &pdu)
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

    fn att_value<'a>(&'a self, handle: u16, scratch: &'a mut [u8; SCRATCH]) -> &'a [u8] {
        let Some(a) = attribute(handle) else { return &[] };
        match a.value {
            Value::Fixed(v) => v,
            Value::Name => {
                let name = self.mesh.name();
                if name.is_empty() {
                    &[0]
                } else {
                    name
                }
            }
            Value::Model => self.identity.model.as_bytes(),
            Value::FirmwareRevision => self.identity.firmware_revision.as_bytes(),
            Value::HardwareRevision => self.identity.hardware_revision.as_bytes(),
            Value::Status => &self.mesh.att_status,
            Value::StatusConfiguration => &self.host.configuration,
            Value::Command => &self.mesh.att_command,
            Value::Pair => &self.mesh.att_pair,
            Value::Ota => {
                scratch[..20].copy_from_slice(&self.ota_status());
                &scratch[..20]
            }
            Value::DiagInfo => self.diag_info(),
            Value::DiagState => {
                let n = self.diag_state(scratch);
                &scratch[..n]
            }
            Value::DiagLog => self.diag_log(),
            Value::DiagFrames => self.diag_frames(),
            Value::DiagConfig => self.diag_config(),
            Value::DiagCounters => self.diag_counters(),
            Value::DiagCommand => &[],
        }
    }

    fn att_write(&mut self, handle: u16, value: &[u8]) -> Result<(), u8> {
        let a = attribute(handle).ok_or(ERR_INVALID_HANDLE)?;
        if !a.writable {
            return Err(ERR_WRITE_NOT_PERMITTED);
        }
        match a.value {
            Value::Status => self.mesh_status_write(value),
            Value::StatusConfiguration => {
                if value.len() != 2 {
                    return Err(ERR_INVALID_LENGTH);
                }
                let was = self.mesh_status_enabled();
                self.host.configuration = [value[0] & cccd_bits(DECL_STATUS[0]), 0];
                self.mesh_status_switched(was);
            }
            Value::Command => self.mesh_command_write(value),
            Value::Ota => self.ota_write(value),
            Value::Pair => self.mesh_pair_write(value),
            Value::DiagCommand => self.diag_write(value),
            _ => {}
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
                for handle in start..=end {
                    let Some(a) = attribute(handle) else { break };
                    let (this, len) = match a.uuid {
                        Uuid::Short(_) => (1, 2),
                        Uuid::Long(_) => (2, 16),
                    };
                    if format == 0 {
                        format = this;
                        out.put(&[FIND_INFO_RSP, format]);
                    }
                    if this != format || out.room() < 2 + len {
                        break;
                    }
                    out.put(&handle.to_le_bytes());
                    match a.uuid {
                        Uuid::Short(u) => out.put(&u.to_le_bytes()),
                        Uuid::Long(u) => out.put(u),
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
                for handle in start..=end {
                    let Some(a) = attribute(handle) else { break };
                    if !a.uuid.matches(&kind) || self.att_value(handle, &mut scratch) != wanted {
                        continue;
                    }
                    if out.room() < 4 {
                        break;
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
                if grouped && !Uuid::Short(PRIMARY_SERVICE).matches(&kind) {
                    return Err((start, ERR_UNSUPPORTED_GROUP_TYPE));
                }
                let head = if grouped { 4 } else { 2 };
                let mut each = 0;
                for handle in start..=end {
                    let Some(a) = attribute(handle) else { break };
                    if !a.uuid.matches(&kind) {
                        continue;
                    }
                    if !a.readable() {
                        if each == 0 {
                            return Err((handle, ERR_READ_NOT_PERMITTED));
                        }
                        break;
                    }
                    let value = self.att_value(handle, &mut scratch);
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
                if !a.readable() {
                    return Err((handle, ERR_READ_NOT_PERMITTED));
                }
                let value = self.att_value(handle, &mut scratch);
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
