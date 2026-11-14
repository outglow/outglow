//! L2CAP and the attribute server.
//!
//! The attribute table is the stock bulb's, handle for handle (GATT audit of
//! the stock firmware in the emulator): GAP, Device Information and the
//! Telink mesh service with its status, command, update and pairing
//! characteristics. Apps address these by handle.

use crate::hal::{Hal, LL_PAYLOAD_MAX};
use crate::ll::{LinkState, TX_RESERVE};
use crate::pdu;
use crate::Ble;

pub(crate) const CID_ATT: u16 = 4;
pub(crate) const CID_SIGNALING: u16 = 5;
const CID_SMP: u16 = 6;

/// The largest attribute protocol packet accepted and sent.
pub(crate) const MTU_MAX: usize = 64;
const MTU_DEFAULT: usize = 23;
const FRAME_MAX: usize = MTU_MAX + 4;
/// The largest signalling packet taken (MTUsig): what fits the frame buffer.
const SIG_MTU: u16 = MTU_MAX as u16;

// attribute protocol opcodes; a response's opcode is its request's plus one
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

/// 00010203-0405-0607-0809-0a0b0c0d19xx, least significant byte first.
const fn telink(short: u8) -> [u8; 16] {
    [short, 0x19, 0x0d, 0x0c, 0x0b, 0x0a, 0x09, 0x08, 0x07, 0x06, 0x05, 0x04, 0x03, 0x02, 0x01, 0x00]
}

/// Characteristic declaration: properties, value handle, 128-bit UUID.
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

const READ: u8 = 0x02;
const WRITE_NO_RSP: u8 = 0x04;
const WRITE: u8 = 0x08;
const NOTIFY: u8 = 0x10;

const DECL_STATUS: [u8; 19] = declaration(READ | WRITE | NOTIFY, HANDLE_STATUS as u8, 0x11);
const DECL_COMMAND: [u8; 19] = declaration(READ | WRITE | WRITE_NO_RSP, HANDLE_COMMAND as u8, 0x12);
const DECL_OTA_CLASSIC: [u8; 19] = declaration(READ | WRITE, HANDLE_OTA_CLASSIC as u8, 0x13);
const DECL_OTA: [u8; 19] = declaration(READ | WRITE_NO_RSP, HANDLE_OTA as u8, 0x15);
const DECL_PAIR: [u8; 19] = declaration(READ | WRITE, HANDLE_PAIR as u8, 0x14);

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
            // a 16-bit UUID written out in full on the Bluetooth base UUID
            (Uuid::Short(a), Wanted::Long(b)) => {
                b[..12] == [0xfb, 0x34, 0x9b, 0x5f, 0x80, 0x00, 0x00, 0x80, 0x00, 0x10, 0x00, 0x00]
                    && b[12..] == [*a as u8, (*a >> 8) as u8, 0, 0]
            }
            (Uuid::Long(_), Wanted::Short(_)) => false,
        }
    }
}

/// The attribute type a request asks for.
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
}

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

/// Attribute `n` of the table has handle `n + 1`.
static TABLE: [Attribute; 31] = [
    // Generic Access, 0x01..=0x06
    fixed(PRIMARY_SERVICE, &[0x00, 0x18]),
    fixed(CHARACTERISTIC, &[READ, 0x03, 0x00, 0x00, 0x2a]),
    read_only(0x2a00, Value::Name),
    fixed(USER_DESCRIPTION, b"name"),
    fixed(CHARACTERISTIC, &[READ, 0x06, 0x00, 0x01, 0x2a]),
    fixed(0x2a01, &[0x00, 0x00]),
    // Device Information, 0x07..=0x0f
    fixed(PRIMARY_SERVICE, &[0x0a, 0x18]),
    fixed(CHARACTERISTIC, &[READ, 0x09, 0x00, 0x26, 0x2a]),
    read_only(0x2a26, Value::FirmwareRevision),
    fixed(CHARACTERISTIC, &[READ, 0x0b, 0x00, 0x29, 0x2a]),
    fixed(0x2a29, b"AwoX"),
    fixed(CHARACTERISTIC, &[READ, 0x0d, 0x00, 0x24, 0x2a]),
    read_only(0x2a24, Value::Model),
    fixed(CHARACTERISTIC, &[READ, 0x0f, 0x00, 0x27, 0x2a]),
    read_only(0x2a27, Value::HardwareRevision),
    // Telink mesh, 0x10..=0x1f
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
];

const LAST_HANDLE: u16 = TABLE.len() as u16;

fn attribute(handle: u16) -> Option<&'static Attribute> {
    TABLE.get((handle as usize).checked_sub(1)?)
}

fn is_service(handle: u16) -> bool {
    attribute(handle).is_some_and(|a| a.uuid == Uuid::Short(PRIMARY_SERVICE))
}

/// The last handle of the service that starts at `handle`.
fn group_end(handle: u16) -> u16 {
    (handle + 1..=LAST_HANDLE).find(|h| is_service(*h)).map_or(LAST_HANDLE, |h| h - 1)
}

pub(crate) struct Host {
    /// An L2CAP frame being put together from link layer fragments.
    frame: [u8; FRAME_MAX],
    have: usize,
    want: usize,
    /// A frame has started and is not complete yet.
    open: bool,
    pub mtu: usize,
    configuration: [u8; 2],
}

impl Host {
    /// Whether the client enabled notifications in the status characteristic's configuration.
    pub fn notifications_enabled(&self) -> bool {
        self.configuration[0] & 0x01 != 0
    }

    pub fn new() -> Host {
        Host { frame: [0; FRAME_MAX], have: 0, want: 0, open: false, mtu: MTU_DEFAULT, configuration: [0; 2] }
    }
}

/// A response being written, cut at the connection's MTU.
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

    /// Sends an L2CAP frame, in link layer fragments. False (and nothing
    /// sent) when the transmit queue has no room for all of it.
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

    /// A notification of the status characteristic. False when it could not be queued.
    pub(crate) fn notify_status(&mut self, value: &[u8; 20]) -> bool {
        // a little room stays for answers to the central's requests
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
        // the first FRAME_MAX octets are kept, `have` counts all of them
        let at = h.have.min(FRAME_MAX);
        let n = data.len().min(FRAME_MAX - at);
        h.frame[at..at + n].copy_from_slice(&data[..n]);
        h.have += data.len();
        // the length is in the first two bytes; even the header may come in pieces
        if h.have < 4 {
            return;
        }
        h.want = 4 + u16_at(&h.frame, 0) as usize;
        if h.have < h.want {
            return;
        }
        h.open = false;
        // fragments that carry more than the Length field: the frame is
        // discarded (Vol 3 Part A 7.2.2)
        if h.have > h.want {
            return;
        }
        let frame = h.frame;
        let len = h.want;
        h.have = 0;
        let cid = u16_at(&frame, 2);
        if len > FRAME_MAX {
            // Too long to be kept. A signalling packet over MTUsig gets a
            // Command Reject "signalling MTU exceeded" with our MTUsig (Part A 4).
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
            // No pairing (the mesh has its own login, as on the stock): any
            // security manager command but a Pairing Failed gets Pairing
            // Failed, "pairing not supported". Codes past the last command
            // (0x0e) and 0x00 are reserved and ignored (Vol 3 Part H 3.3).
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
            // responses (the one to our parameter update request among them)
            // and the credit indication, which has no answer
            0x01 | 0x13 | 0x15 | 0x16 | 0x18 | 0x1a => {}
            // a request this device has no answer to: Command Reject, "not understood"
            _ => {
                self.send_l2cap(CID_SIGNALING, &[0x01, p[1], 0x02, 0x00, 0x00, 0x00]);
            }
        }
    }

    fn att_value(&self, handle: u16, out: &mut [u8; 20]) -> usize {
        let Some(a) = attribute(handle) else { return 0 };
        let bytes: &[u8] = match a.value {
            Value::Fixed(v) => v,
            Value::Name => {
                // an empty name still reads as one byte, as in the C firmware
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
                let status = self.ota_status();
                out.copy_from_slice(&status);
                return 20;
            }
        };
        let n = bytes.len().min(20);
        out[..n].copy_from_slice(&bytes[..n]);
        n
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
                // notifications enabled the standard way count like the app's own switch
                let was = self.mesh_status_enabled();
                self.host.configuration.copy_from_slice(value);
                self.mesh_status_switched(was);
            }
            Value::Command => self.mesh_command_write(value),
            Value::Ota => self.ota_write(value),
            Value::Pair => self.mesh_pair_write(value),
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
            // commands (bit 6) are never answered
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
        // Vol 3 Part F 3.3: a request of the wrong length is answered with
        // Invalid PDU and handle 0x0000; `need` for the requests with a
        // variable field at the end, `exact` for those without one
        let need = |n: usize| if p.len() >= n { Ok(()) } else { Err((0, ERR_INVALID_PDU)) };
        let exact = |n: usize| if p.len() == n { Ok(()) } else { Err((0, ERR_INVALID_PDU)) };
        // the handle range of the discovery requests
        let range = |p: &[u8]| -> Result<(u16, u16), (u16, u8)> {
            let (start, end) = (u16_at(p, 1), u16_at(p, 3));
            if start == 0 || start > end {
                return Err((start, ERR_INVALID_HANDLE));
            }
            Ok((start, end.min(LAST_HANDLE)))
        };
        let mut value = [0u8; 20];
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
                    let n = self.att_value(handle, &mut value);
                    if !a.uuid.matches(&kind) || value[..n] != *wanted {
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
                // a type neither 2 nor 16 octets long makes the PDU the wrong length (3.3)
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
                    let n = self.att_value(handle, &mut value);
                    if each == 0 {
                        // all entries of a response have the first one's length
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
                attribute(handle).ok_or((handle, ERR_INVALID_HANDLE))?;
                let n = self.att_value(handle, &mut value);
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
