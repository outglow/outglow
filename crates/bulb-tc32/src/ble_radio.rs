//! Bluetooth LE (1 Mb/s) on the TLSR8258 radio, next to 802.15.4, written
//! for this project.
//!
//! The radio is the 802.15.4 driver's (`radio.rs`) and is borrowed for one
//! event at a time: an advertising event, a connection event or a moment
//! on the mesh channels. `enter` switches it to BLE, `leave` gives it back
//! with the receiver on. Everything in between runs with interrupts off and
//! polls the radio's status register.
//!
//! Register values and the order of the writes follow what Telink's BLE
//! library does on this chip, as written down in
//! `reference/radio/tlsr8258-ble-spec.md` (sections in brackets below).
//! What is done differently on purpose:
//! - events are polled from the main loop, not started by the system-timer
//!   interrupt, and the receiver of a connection event is opened by the
//!   radio's own start tick a fixed time before the expected packet;
//! - new data is recognised by the hardware's NESN having moved, not by a
//!   software copy of the central's SN;
//! - packets are handed to the hardware FIFO at the start of an event only.
//!
//! The parts with deadlines inside the 150 us turnaround (the answer to a
//! scan or connect request, the check of a received data packet) are in
//! `.ram_code`.

use core::ptr::{addr_of, addr_of_mut};

use ble::hal::{ConnEvent, EventResult, ADV_PDU_MAX, CONNECT_LEN, LL_PAYLOAD_MAX};

use crate::radio;
use crate::reg::{self, clear8, read16, read32, read8, set8, tick, write16, write32, write8, TICKS_PER_US};

const RF_CMD: usize = 0xf00; // 0x80 stop, 0x82 BRX, 0x85 STX, 0x87 STX then RX
const RF_SN_RESET: usize = 0xf01;
const RF_MODE_CTRL: usize = 0xf02;
const RF_LL_CTRL: usize = 0xf03; // bit 0 FSM timeout on, bit 4 SN, bit 5 NESN
const RF_TX_SETTLE: usize = 0xf04;
const RF_RX_TIMEOUT: usize = 0xf0a;
const RF_CMD_CTRL: usize = 0xf16; // bit 2: start at RF_START_TICK
const RF_START_TICK: usize = 0xf18;
const RF_IRQ_MASK: usize = 0xf1c;
const RF_IRQ_STATUS: usize = 0xf20; // write one to clear
const RF_SN: usize = 0xf22; // bit 0: SN of the packet in flight
const RF_NESN: usize = 0xf23; // bit 4: NESN
const RF_FIRST_TIMEOUT: usize = 0xf28;
const RF_FSM_TIMEOUT: usize = 0xf2c;
const RF_ACCESS: usize = 0x408; // first byte on air in the most significant byte
const RF_ACCESS_CTRL: usize = 0x405;
const RF_CHANNEL: usize = 0x40d;
const RF_CRC_INIT: usize = 0x424;
const RF_RX_CTRL: usize = 0x428;
const RF_TIMESTAMP: usize = 0x450;
const BASEBAND_RESET: usize = 0x61;
const DMA_RX_ADDR: usize = 0xc08;
const DMA_TX_ADDR: usize = 0xc0c;
const DMA_CHANNEL_ENABLE: usize = 0xc20;
const DMA_FIFO: usize = 0xc28; // pointers of the transmit FIFO as one word
const TX_FIFO_RPTR: usize = 0xc2a; // 0x10 empty the FIFO, 0x40 | n set the read pointer
const TX_FIFO_WPTR: usize = 0xc2b;
const TX_FIFO_PUSH: usize = 0xc2c;

const STATUS_RX: u16 = 0x0001;
const STATUS_TX: u16 = 0x0002;
/// Receive timeout, CRC, command done, FSM timeout, first timeout: the event is over.
const STATUS_END: u16 = 0x0474;

/// Radio setup for BLE 1M, in the vendor's order and widths [1.2].
const TABLE: [(u16, u8, u32); 20] = [
    (0x1223, 1, 0x23),
    (0x1221, 1, 0x0a),
    (0x12d2, 1, 0x9b),
    (0x12d3, 1, 0x19),
    (0x127b, 1, 0x0e),
    (0x1276, 1, 0x50),
    (0x1220, 4, 0x2320_0a16),
    (0x124a, 1, 0x0e),
    (0x124b, 1, 0x09),
    (0x0400, 1, 0x1f),
    (0x0401, 1, 0x01),
    (0x0402, 1, 0x46),
    (0x0404, 1, 0xf5),
    (0x0405, 1, 0x04),
    (0x0420, 1, 0x1e),
    (0x0421, 1, 0xa1),
    (0x0430, 1, 0x3e),
    (0x0460, 4, 0x5f4f_4434),
    (0x0464, 2, 0x766b),
    (0x0f06, 1, 0x00),
];
const TABLE_TAIL: [(u16, u8); 3] = [(0x0f0c, 0x50), (0x0f0e, 0x00), (0x0f10, 0x00)];

const ADV_ACCESS: u32 = 0xd6be_898e;
const ADV_CRC_INIT: u32 = 0x0055_5555;
/// TX settle time of advertising and of connection events [8].
const ADV_TX_SETTLE: u16 = 74;
const CONN_TX_SETTLE: u16 = 75;

/// The receiver of a connection event is opened this long before the time
/// the link layer asks for (the vendor opens 0.5 ms before the anchor).
const RX_LEAD: u32 = 300 * TICKS_PER_US;
/// The radio is taken from 802.15.4 this long before the receiver opens.
const SETUP: u32 = 400 * TICKS_PER_US;
/// A packet that started inside the window has ended by then.
const FIRST_RX_GRACE: u32 = 400 * TICKS_PER_US;
/// Once the central was heard: the latest its next packet can end [8].
const RX_GUARD: u32 = 2100 * TICKS_PER_US;
/// Anchor point from the timestamp of the first packet [6.5].
const TIMESTAMP_TO_ANCHOR: u32 = 0x500;
/// How long an advertising event or a moment on the mesh waits for the
/// 802.15.4 traffic of the moment to end (`wait_for_quiet`, `let_zigbee_finish`).
const ADV_PATIENCE: u32 = 6000 * TICKS_PER_US;
/// An 802.15.4 neighbour heard this recently may still be about to send
/// the frames its last one brings: nwkcMaxBroadcastJitter (R22 3.6.5,
/// 64 ms), the longest a router waits before it relays a broadcast. (A
/// stock router answers a broadcast at once and relays it 10 to 30 ms
/// later.) An advertising event then uses the first advertising channel only
/// (`advertise`).
const NEIGHBOURS_QUIET: u32 = 64_000 * TICKS_PER_US;

#[repr(C, align(4))]
struct Aligned<const N: usize>([u8; N]);

/// DMA receive buffers: u32 length, header, payload, CRC, timestamp, 2
/// bytes, RSSI, status [4.2]. Two, so that the next packet has somewhere to
/// go while one is looked at.
const RX_LEN: usize = 64;
static mut RX: [Aligned<RX_LEN>; 2] = [Aligned([0; RX_LEN]), Aligned([0; RX_LEN])];
static mut RX_CURRENT: u8 = 0;

/// Transmit buffers: u32 length (PDU length), header, payload [4.1].
static mut ADV: Aligned<48> = Aligned([0; 48]);
static mut SCAN_RSP: Aligned<48> = Aligned([0; 48]);
/// What the radio sends when the FIFO is empty: an empty data packet.
static mut EMPTY: Aligned<8> = Aligned([2, 0, 0, 0, 1, 0, 0, 0]);

const TX_SLOTS: usize = 8;
const TX_SLOT: usize = 36;
static mut TX_QUEUE: Aligned<{ TX_SLOTS * TX_SLOT }> = Aligned([0; TX_SLOTS * TX_SLOT]);
/// Counters into the transmit queue (slot = counter % TX_SLOTS): oldest
/// unacknowledged packet, next packet to hand to the radio, next free slot.
static mut TX_HEAD: u8 = 0;
static mut TX_HANDED: u8 = 0;
static mut TX_TAIL: u8 = 0;
/// What each entry of the radio's FIFO is: 1 a packet of the queue, 0 the
/// empty packet pushed ahead of the first one [6.3].
static mut FIFO_KIND: [u8; 16] = [0; 16];
/// Software copies carried from event to event [6.3]: SN and NESN as bits 4
/// and 5 of `RF_LL_CTRL`, the FIFO's read pointer, and how far the queue
/// has been freed.
static mut SN_NESN: u8 = 0x10;
static mut FIFO_RPTR: u8 = 0;
static mut FIFO_FREED: u8 = 0;
static mut FIFO_WORD: u32 = 0;
static mut FIFO_STARTED: u8 = 0;
static mut RESET_PENDING: u8 = 0;

const RX_SLOTS: usize = 8;
const RX_SLOT: usize = 2 + LL_PAYLOAD_MAX;
static mut RX_QUEUE: [[u8; RX_SLOT]; RX_SLOTS] = [[0; RX_SLOT]; RX_SLOTS];
static mut RX_HEAD: u8 = 0;
static mut RX_TAIL: u8 = 0;

/// Results of the `.ram_code` parts.
static mut CONNECT_TICK: u32 = 0;
static mut ANCHOR: u32 = 0;
static mut ANCHOR_SEEN: u8 = 0;

/// CRC-24 of BLE, reflected, a nibble at a time. In `.data` (SRAM): the
/// check runs inside the turnaround.
static mut CRC_TABLE: [u32; 16] = crc_table();

/// Counters for tests and diagnostics.
#[repr(C)]
pub struct Counters {
    pub adv_events: u32,
    pub conn_events: u32,
    pub heard: u32,
    pub crc_errors: u32,
    pub rx_overflows: u32,
    pub mesh_bursts: u32,
    pub mesh_received: u32,
}
#[no_mangle]
pub static mut BLE_RADIO: Counters =
    Counters { adv_events: 0, conn_events: 0, heard: 0, crc_errors: 0, rx_overflows: 0, mesh_bursts: 0, mesh_received: 0 };

/// The radio lent to BLE (every `enter` .. `leave`): how often, for how
/// many whole milliseconds and ticks beyond them, and when the current loan began.
static mut LENDS: u32 = 0;
static mut LENT_MS: u32 = 0;
/// The longest single loan of the radio, microseconds.
static mut LENT_MAX_US: u32 = 0;
static mut LENT_REST: u32 = 0;
static mut LENT_AT: u32 = 0;

macro_rules! get {
    ($s:ident) => {
        // SAFETY: plain data, single thread of execution (BLE runs with interrupts off).
        unsafe { addr_of!($s).read_volatile() }
    };
}
macro_rules! put {
    ($s:ident, $v:expr) => {
        // SAFETY: as in `get!`.
        unsafe { addr_of_mut!($s).write_volatile($v) }
    };
}
macro_rules! count {
    ($field:ident) => {
        // SAFETY: as in `get!`.
        unsafe {
            let p = &raw mut (*(&raw mut BLE_RADIO)).$field;
            p.write_volatile(p.read_volatile().wrapping_add(1));
        }
    };
}

const fn crc_table() -> [u32; 16] {
    let mut table = [0u32; 16];
    let mut i = 0;
    while i < 16 {
        let mut c = i as u32;
        let mut k = 0;
        while k < 4 {
            c = if c & 1 != 0 { (c >> 1) ^ 0xda_6000 } else { c >> 1 };
            k += 1;
        }
        table[i] = c;
        i += 1;
    }
    table
}

/// The 24 bits of a CRC start value in reversed order, as the reflected CRC wants it.
fn reverse24(v: u32) -> u32 {
    (v << 8).reverse_bits() & 0x00ff_ffff
}

#[inline(always)]
fn rx_buffer(i: u8) -> *mut u8 {
    // SAFETY: `i & 1` is a valid index; only the address is taken.
    unsafe { addr_of_mut!(RX[(i & 1) as usize].0) as *mut u8 }
}

#[inline(always)]
fn reached(now: u32, t: u32) -> bool {
    now.wrapping_sub(t) as i32 >= 0
}

/// Receiver and transmitter off, command scheduling off [1.3].
#[inline(always)]
fn trx_off() {
    write8(RF_CMD_CTRL, 0x29);
    write8(RF_RX_CTRL, 0x80);
    write8(RF_MODE_CTRL, 0x45);
}

#[inline(always)]
fn stop() {
    write8(RF_CMD, 0x80);
}

/// Schedules a command of the radio's state machine for tick `at`; the
/// transmit buffer is named after the command, as the vendor does [1.3].
#[inline(always)]
fn command(cmd: u8, at: u32, tx: *const u8) {
    write32(RF_START_TICK, at);
    set8(RF_CMD_CTRL, 0x04);
    write8(RF_CMD, cmd);
    write16(DMA_TX_ADDR, tx as usize as u16);
}

/// Tunes to a BLE channel index (0..=36 data, 37..=39 advertising) [3.1].
#[link_section = ".ram_code"]
#[inline(never)]
extern "C" fn set_channel(chn: u8) {
    write8(RF_CHANNEL, chn);
    let idx = if chn <= 10 {
        chn + 1
    } else if chn <= 36 {
        chn + 2
    } else if chn == 37 {
        0
    } else if chn == 38 {
        12
    } else {
        39
    };
    let f = 2402 + 2 * idx as u16;
    let band = if f > 2519 {
        0x04
    } else if f > 2494 {
        0x08
    } else if f > 2464 {
        0x0c
    } else if f > 2434 {
        0x10
    } else if f > 2404 {
        0x14
    } else {
        0x18
    };
    write8(0x1244, 0x20);
    write8(0x1245, 0x26);
    write8(0x1228, (read8(0x1228) & 0xc0) | idx);
    write8(0x1229, (read8(0x1229) & 0xc3) | band);
}

/// Takes the radio from 802.15.4 and sets it up for BLE [2.1]. Returns the
/// interrupt state for `leave`.
fn enter() -> u8 {
    let s = radio::suspend();
    let lends = get!(LENDS);
    put!(LENDS, lends.wrapping_add(1));
    put!(LENT_AT, tick());
    clear8(DMA_CHANNEL_ENABLE, 0x08);
    clear8(DMA_CHANNEL_ENABLE, 0x04);
    trx_off();
    write8(BASEBAND_RESET, 0x01);
    write8(BASEBAND_RESET, 0x00);
    set8(BASEBAND_RESET, 0x01);
    clear8(BASEBAND_RESET, 0x01);
    // receive buffer address, size (64 bytes) and mode in one word
    write32(DMA_RX_ADDR, rx_buffer(get!(RX_CURRENT)) as usize as u16 as u32 | 0x0104_0000);
    if get!(FIFO_STARTED) != 0 {
        write32(DMA_FIFO, get!(FIFO_WORD));
    }
    if get!(RESET_PENDING) != 0 || get!(FIFO_STARTED) == 0 {
        // a connection began or ended: the radio's FIFO is emptied [5.4, 6.1]
        put!(RESET_PENDING, 0);
        put!(FIFO_STARTED, 1);
        write8(TX_FIFO_RPTR, 0x10);
        let rptr = read8(TX_FIFO_RPTR) & 0x0f;
        put!(FIFO_RPTR, rptr);
        put!(FIFO_FREED, rptr);
    }
    // the radio is polled here: its interrupt stays masked until `leave`
    write32(reg::IRQ_MASK, read32(reg::IRQ_MASK) & !reg::IRQ_MASK_RF);
    write16(RF_IRQ_STATUS, 0xffff);
    for (addr, width, value) in TABLE {
        match width {
            1 => write8(addr as usize, value as u8),
            2 => write16(addr as usize, value as u16),
            _ => write32(addr as usize, value),
        }
    }
    for (addr, value) in TABLE_TAIL {
        write8(addr as usize, value);
    }
    set8(DMA_CHANNEL_ENABLE, 0x0c);
    // TX power: +3 dBm, the vendor's setting for BLE next to Zigbee's +10
    set8(0x1225, 0x40);
    write8(0x1226, (read8(0x1226) & 0x7f) | 0x80);
    write8(0x1227, (read8(0x1227) & 0xe0) | 0x1f);
    s
}

/// Gives the radio back to 802.15.4 [2.2].
fn leave(s: u8) {
    let (rest, at, ms) = (get!(LENT_REST), get!(LENT_AT), get!(LENT_MS));
    let this = tick().wrapping_sub(at) / TICKS_PER_US;
    if this > get!(LENT_MAX_US) {
        put!(LENT_MAX_US, this);
    }
    let ticks = rest + tick().wrapping_sub(at);
    put!(LENT_MS, ms.wrapping_add(ticks / (1000 * TICKS_PER_US)));
    put!(LENT_REST, ticks % (1000 * TICKS_PER_US));
    trx_off();
    put!(FIFO_WORD, read32(DMA_FIFO));
    clear8(DMA_CHANNEL_ENABLE, 0x04);
    radio::resume(s);
}

/// Lets an 802.15.4 frame that is arriving finish (and be acknowledged)
/// while there is time until `latest`.
fn let_zigbee_finish(latest: u32) {
    while radio::receiving() && !reached(tick(), latest) {}
}

/// Before an advertising event: lets an 802.15.4 frame that is arriving
/// finish, and the frames of the neighbours that follow it closely, while
/// there is time until `latest`. The link layer chooses its moment when the
/// radio has been quiet for a while (`ble::Hal::radio_busy`), or at the end
/// of its advDelay; but a frame may begin between that and here, and one
/// frame often brings another. The radio used to be taken right at the end
/// of the first: in the emulator's `mixed_chain` a stock router's Link
/// Status was heard and its Device_annce, sent 0.4 ms later and only once,
/// was not. (Only received frames count: what follows a frame of our own,
/// its acknowledgement, `radio::transmit` has waited for. An event that
/// waits here may start a few milliseconds after its advDelay range, as it
/// did before for a frame that was arriving; a lost broadcast costs more
/// than a late advertisement.)
fn wait_for_quiet(latest: u32) {
    while (radio::receiving() || radio::rx_quiet_ticks() < crate::ble::RADIO_QUIET) && !reached(tick(), latest) {}
}

/// One advertising channel [5.1, 5.2]: the advertisement, then a short
/// wait for an answer. Returns 0 when none came, 1 when a scan request was
/// answered, 2 when a connect request for this device is in the receive
/// buffer (`CONNECT_TICK`: when it had arrived).
#[link_section = ".ram_code"]
#[inline(never)]
extern "C" fn adv_channel(chn: u8, answer_window: u32) -> u8 {
    let p = rx_buffer(get!(RX_CURRENT));
    // SAFETY: the buffers are static and 4-byte aligned; indices are checked below.
    let adv = unsafe { addr_of!(ADV.0) as *const u8 };
    let header = || unsafe { (p.add(4) as *const u32).read_volatile() };
    let at = |i: usize| unsafe { p.add(i).read_volatile() };
    stop();
    set_channel(chn);
    write16(RF_IRQ_STATUS, STATUS_RX | STATUS_TX);
    // SAFETY: as above.
    unsafe { (p.add(4) as *mut u32).write_volatile(0) };
    command(0x87, tick().wrapping_add(100), adv);
    let t0 = tick();
    // the advertisement is at most 376 us long; the vendor waits without a limit
    while read16(RF_IRQ_STATUS) & STATUS_TX == 0 && tick().wrapping_sub(t0) < 2000 * TICKS_PER_US {}
    write16(RF_IRQ_STATUS, STATUS_TX);
    // the DMA stores an answer while it arrives: its header shows first
    while header() == 0 && tick().wrapping_sub(t0) < answer_window {}
    if header() == 0 {
        return 0;
    }
    let t1 = tick();
    // the scan response is made ready before the request has ended, and
    // started before it is known to be one; a stop takes it back
    // SAFETY: only the address is taken.
    write16(DMA_TX_ADDR, unsafe { addr_of!(SCAN_RSP.0) as usize as u16 });
    while read16(RF_IRQ_STATUS) & STATUS_RX == 0 && tick().wrapping_sub(t1) < 6400 {}
    write32(RF_START_TICK, tick().wrapping_add(48));
    write8(RF_CMD, 0x85);
    write16(RF_IRQ_STATUS, STATUS_RX);

    let dma_len = header_len(p);
    let rf_len = at(5) as usize;
    let mut ours = dma_len == rf_len + 13 && rf_len <= 37 && at(dma_len + 3) & 0x01 == 0 && at(4) & 0x80 == 0;
    let mut i = 0;
    while ours && i < 6 {
        // the advertiser's address in the answer against ours
        // SAFETY: `adv` holds 48 bytes.
        ours = at(12 + i) == unsafe { adv.add(6 + i).read_volatile() };
        i += 1;
    }
    let kind = at(4) & 0x0f;
    if ours && kind == 3 && rf_len == 12 {
        // let the response go out
        // SAFETY: index 5 is inside the buffer.
        let rsp_len = unsafe { (addr_of!(SCAN_RSP.0) as *const u8).add(5).read_volatile() } as u32;
        let t2 = tick();
        while tick().wrapping_sub(t2) < (rsp_len * 8 + 204) * TICKS_PER_US {}
        stop();
        return 1;
    }
    stop();
    if ours && kind == 5 && rf_len == CONNECT_LEN {
        put!(CONNECT_TICK, tick());
        return 2;
    }
    0
}

#[inline(always)]
fn header_len(p: *const u8) -> usize {
    // SAFETY: `p` is a DMA buffer, 4-byte aligned.
    unsafe { (p as *const u32).read_volatile() as usize }
}

/// One advertising event on channels 37, 38 and 39.
pub fn advertise(adv: &[u8], scan_rsp: &[u8], connect: &mut [u8; CONNECT_LEN]) -> Option<u32> {
    if adv.len() < 2 || adv.len() > ADV_PDU_MAX || scan_rsp.len() < 2 || scan_rsp.len() > ADV_PDU_MAX {
        return None;
    }
    // SAFETY: the radio is not transmitting from these buffers now.
    unsafe {
        let a = &mut *addr_of_mut!(ADV.0);
        a[0] = adv.len() as u8;
        a[4..4 + adv.len()].copy_from_slice(adv);
        let r = &mut *addr_of_mut!(SCAN_RSP.0);
        r[0] = scan_rsp.len() as u8;
        r[4..4 + scan_rsp.len()].copy_from_slice(scan_rsp);
    }
    #[cfg(feature = "trace")]
    let t_ev = tick();
    wait_for_quiet(tick().wrapping_add(ADV_PATIENCE));
    let s = enter();
    count!(adv_events);
    trx_off();
    write32(RF_ACCESS, ADV_ACCESS);
    write32(RF_CRC_INIT, ADV_CRC_INIT);
    write16(RF_TX_SETTLE, ADV_TX_SETTLE);
    write16(RF_IRQ_MASK, 0);
    // packet air time, settling, turnaround and the first bytes of an answer
    let answer_window = (adv[1] as u32 + 60) * 8 * TICKS_PER_US;
    // Core 5.4 Vol 6 Part B 4.4.2.2: at most one PDU on each advertising
    // channel, "usually" one on every channel. While the 802.15.4
    // neighbours may still be sending (a relay of a broadcast after the
    // relayer's own answer to it, sent once), the event is a third as long,
    // and so is the time in which such a frame is lost: in the emulator's
    // `mixed_chain` a stock router's relay of a broadcast On that came 20 ms
    // after its answer to it fell into a whole event three times in 71 runs.
    let channels: &[u8] = if radio::rx_quiet_ticks() < NEIGHBOURS_QUIET { &[37] } else { &[37, 38, 39] };
    let mut result = None;
    for &chn in channels {
        match adv_channel(chn, answer_window) {
            0 => {}
            2 => {
                // SAFETY: the receiver is stopped; the buffer holds a 34-byte payload from offset 6.
                let current = (get!(RX_CURRENT) & 1) as usize;
                let p = unsafe { &*addr_of!(RX[current].0) };
                connect.copy_from_slice(&p[6..6 + CONNECT_LEN]);
                result = Some(get!(CONNECT_TICK));
                break;
            }
            // a scan request was answered: the event ends on this channel
            _ => break,
        }
    }
    stop();
    write16(RF_IRQ_STATUS, 0xffff);
    leave(s);
    #[cfg(feature = "trace")]
    crate::trace::span(crate::trace::BLE, 0, 0, t_ev);
    result
}

/// A connection starts or has ended: the queues are empty, SN and NESN
/// start over. The radio's FIFO is emptied when BLE next has the radio.
pub fn connection_reset() {
    put!(TX_HEAD, 0);
    put!(TX_HANDED, 0);
    put!(TX_TAIL, 0);
    put!(RX_HEAD, 0);
    put!(RX_TAIL, 0);
    put!(SN_NESN, 0x10);
    put!(RESET_PENDING, 1);
}

/// What `brx` needs to know about an event.
#[repr(C)]
struct Brx {
    channel: u8,
    access: u32,
    crc_init: u32,
    crc_start: u32,
    /// When the receiver opens.
    open: u32,
    /// The central's first packet has arrived by then, or there is none.
    first_by: u32,
    deadline: u32,
    fsm_timeout_us: u32,
}

/// One connection event [6.2, 6.3]: sets the radio's link layer state
/// machine up, starts it and follows its status until the event is over.
/// Acknowledging, retransmitting and sending from the FIFO are the
/// hardware's; here each received packet is checked and queued.
#[link_section = ".ram_code"]
#[inline(never)]
extern "C" fn brx(e: *const Brx) {
    // SAFETY: the caller passes a valid `Brx`.
    let e = unsafe { &*e };
    let mut current = get!(RX_CURRENT);
    trx_off();
    stop();
    write8(RF_SN_RESET, 0x01);
    // the status bits the vendor's handler looks at; the interrupt itself is masked
    write16(RF_IRQ_MASK, 0x0477);
    write16(RF_IRQ_STATUS, 0xffff);
    write8(BASEBAND_RESET, 0x01);
    write8(BASEBAND_RESET, 0x00);
    set_channel(e.channel);
    write32(RF_ACCESS, e.access);
    set8(RF_ACCESS_CTRL, 0x80);
    write32(RF_CRC_INIT, e.crc_init);

    // SN, NESN and the FIFO's read pointer from the last event
    write8(RF_LL_CTRL, read8(RF_LL_CTRL) & 0xcf);
    write8(RF_LL_CTRL, read8(RF_LL_CTRL) | get!(SN_NESN));
    write8(TX_FIFO_RPTR, 0x40 | get!(FIFO_RPTR));
    let mut waiting = read8(TX_FIFO_WPTR).wrapping_sub(read8(TX_FIFO_RPTR)) & 0x0f;
    let mut handed = get!(TX_HANDED);
    let tail = get!(TX_TAIL);
    // SAFETY: only addresses are taken here.
    let (kinds, queue, empty) = unsafe {
        (addr_of_mut!(FIFO_KIND) as *mut u8, addr_of!(TX_QUEUE.0) as *const u8, addr_of!(EMPTY.0) as *const u8)
    };
    if waiting == 0 && handed != tail {
        // The acknowledgement of the empty packet sent last takes the head
        // of the FIFO with it: an empty packet goes in first to be taken.
        // SAFETY: the write pointer is below 16.
        unsafe { kinds.add((read8(TX_FIFO_WPTR) & 0x0f) as usize).write_volatile(0) };
        write16(TX_FIFO_PUSH, empty as usize as u16);
        waiting = 1;
    }
    while waiting < 7 && handed != tail {
        // SAFETY: as above; the slot index is below TX_SLOTS.
        unsafe {
            kinds.add((read8(TX_FIFO_WPTR) & 0x0f) as usize).write_volatile(1);
            write16(TX_FIFO_PUSH, queue.add(handed as usize % TX_SLOTS * TX_SLOT) as usize as u16);
        }
        handed = handed.wrapping_add(1);
        waiting += 1;
    }
    put!(TX_HANDED, handed);

    write16(RF_TX_SETTLE, CONN_TX_SETTLE);
    write16(RF_RX_TIMEOUT, 250);
    set8(RF_LL_CTRL, 0x01);
    write32(RF_FSM_TIMEOUT, e.fsm_timeout_us);
    write32(RF_FIRST_TIMEOUT, 0x0fff_ffff);
    write16(DMA_RX_ADDR, rx_buffer(current) as usize as u16);
    let now = tick();
    let open = if e.open.wrapping_sub(now) as i32 > 10 { e.open } else { now.wrapping_add(10) };
    command(0x82, open, empty);

    let table = addr_of!(CRC_TABLE) as *const u32;
    let mut packets = 0u32;
    let mut guard = e.first_by;
    loop {
        let status = read16(RF_IRQ_STATUS);
        if status & STATUS_RX != 0 {
            let p = rx_buffer(current);
            current ^= 1;
            // the next packet goes to the other buffer
            write16(DMA_RX_ADDR, rx_buffer(current) as usize as u16);
            write16(RF_IRQ_STATUS, STATUS_RX);
            let t = tick();
            guard = t.wrapping_add(RX_GUARD);
            // SAFETY: every index below is checked against the buffer length first.
            let at = |i: usize| unsafe { p.add(i).read_volatile() };
            let dma_len = header_len(p);
            let n = at(5) as usize;
            let first = packets == 0;
            packets += 1;
            // length consistent and the hardware's CRC flag good
            if dma_len == n + 13 && n + 17 <= RX_LEN && at(dma_len + 3) & 0x01 == 0 {
                // the vendor does not trust that flag alone: CRC-24 again in software [4.3]
                let mut crc = e.crc_start;
                let mut i = 4;
                while i < 6 + n {
                    crc ^= at(i) as u32;
                    // SAFETY: the index is below 16.
                    crc = (crc >> 4) ^ unsafe { table.add((crc & 0x0f) as usize).read_volatile() };
                    crc = (crc >> 4) ^ unsafe { table.add((crc & 0x0f) as usize).read_volatile() };
                    i += 1;
                }
                let crc_good = at(6 + n) == crc as u8 && at(7 + n) == (crc >> 8) as u8 && at(8 + n) == (crc >> 16) as u8;
                let sn = (read8(RF_SN) & 0x01) << 4;
                let nesn = (read8(RF_NESN) & 0x10) << 1;
                // the hardware moved NESN: this packet is new to it
                let fresh = nesn != get!(SN_NESN) & 0x20;
                let head = get!(RX_HEAD);
                let full = head.wrapping_sub(get!(RX_TAIL)) as usize >= RX_SLOTS;
                if !crc_good || n > LL_PAYLOAD_MAX || (fresh && n != 0 && full) {
                    // No answer and no change of SN, NESN or the read
                    // pointer: the central sends the packet again.
                    stop();
                    if crc_good {
                        count!(rx_overflows);
                    } else {
                        count!(crc_errors);
                    }
                    break;
                }
                put!(SN_NESN, sn | nesn);
                put!(FIFO_RPTR, read8(TX_FIFO_RPTR) & 0x0f);
                if first {
                    let stamp = read32(RF_TIMESTAMP);
                    if t.wrapping_sub(stamp) < 2000 * TICKS_PER_US {
                        put!(ANCHOR, stamp.wrapping_sub(TIMESTAMP_TO_ANCHOR));
                        put!(ANCHOR_SEEN, 1);
                    }
                }
                if fresh && n != 0 {
                    let slot = (addr_of_mut!(RX_QUEUE) as *mut u8).wrapping_add(head as usize % RX_SLOTS * RX_SLOT);
                    // SAFETY: the slot holds 2 + 27 bytes and `n <= 27`.
                    unsafe {
                        slot.write_volatile(at(4) & 0x03);
                        slot.add(1).write_volatile(n as u8);
                        let mut i = 0;
                        while i < n {
                            slot.add(2 + i).write_volatile(at(6 + i));
                            i += 1;
                        }
                    }
                    put!(RX_HEAD, head.wrapping_add(1));
                }
            }
        }
        if status & STATUS_TX != 0 {
            write16(RF_IRQ_STATUS, STATUS_TX);
        }
        if status & STATUS_END != 0 {
            break;
        }
        let now = tick();
        if reached(now, guard) || reached(now, e.deadline) {
            break;
        }
    }
    stop();
    write16(RF_IRQ_STATUS, 0xffff);
    put!(RX_CURRENT, current);
}

/// Runs one connection event and gives the radio back.
pub fn connection_event(ev: &ConnEvent) -> EventResult {
    let open = ev.start.wrapping_sub(RX_LEAD);
    let take = open.wrapping_sub(SETUP);
    // 802.15.4 keeps the radio until shortly before; a frame arriving then may still finish
    while !reached(tick(), take) {}
    let_zigbee_finish(open.wrapping_sub(SETUP / 4));
    #[cfg(feature = "trace")]
    let t_ev = tick();
    let s = enter();
    count!(conn_events);
    put!(ANCHOR_SEEN, 0);
    let now = tick();
    let e = Brx {
        channel: ev.channel,
        // the first byte on air goes into the most significant byte [3.2]
        access: ev.access_address.swap_bytes(),
        crc_init: ev.crc_init & 0x00ff_ffff,
        crc_start: reverse24(ev.crc_init),
        open,
        first_by: ev.start.wrapping_add(ev.window).wrapping_add(FIRST_RX_GRACE),
        deadline: ev.deadline,
        // the hardware's own limit, a little after ours
        fsm_timeout_us: ev.deadline.wrapping_sub(now) / TICKS_PER_US + 1000,
    };
    brx(&e);
    // what the central acknowledged leaves the queue
    let rptr = get!(FIFO_RPTR);
    let mut freed = get!(FIFO_FREED);
    let mut head = get!(TX_HEAD);
    while freed != rptr {
        // SAFETY: `freed` is below 16.
        if unsafe { (addr_of!(FIFO_KIND) as *const u8).add(freed as usize).read_volatile() } == 1 {
            head = head.wrapping_add(1);
        }
        freed = (freed + 1) & 0x0f;
    }
    put!(FIFO_FREED, freed);
    put!(TX_HEAD, head);
    leave(s);
    #[cfg(feature = "trace")]
    crate::trace::span(crate::trace::BLE, 1, 0, t_ev);
    if get!(ANCHOR_SEEN) != 0 {
        count!(heard);
        EventResult { anchor: Some(get!(ANCHOR)) }
    } else {
        EventResult { anchor: None }
    }
}

pub fn tx_free() -> usize {
    TX_SLOTS - get!(TX_TAIL).wrapping_sub(get!(TX_HEAD)) as usize
}

pub fn tx_idle() -> bool {
    get!(TX_TAIL) == get!(TX_HEAD)
}

pub fn tx_push(llid: u8, payload: &[u8]) -> bool {
    if tx_free() == 0 || payload.len() > LL_PAYLOAD_MAX {
        return false;
    }
    let tail = get!(TX_TAIL);
    // SAFETY: the slot is free: the radio holds no pointer to it.
    let queue = unsafe { &mut *addr_of_mut!(TX_QUEUE.0) };
    let slot = &mut queue[tail as usize % TX_SLOTS * TX_SLOT..][..TX_SLOT];
    slot[0] = payload.len() as u8 + 2;
    slot[1] = 0;
    slot[2] = 0;
    slot[3] = 0;
    slot[4] = llid & 0x03;
    slot[5] = payload.len() as u8;
    slot[6..6 + payload.len()].copy_from_slice(payload);
    put!(TX_TAIL, tail.wrapping_add(1));
    true
}

pub fn rx_pop(payload: &mut [u8; LL_PAYLOAD_MAX]) -> Option<(u8, usize)> {
    let tail = get!(RX_TAIL);
    if tail == get!(RX_HEAD) {
        return None;
    }
    // SAFETY: slots between tail and head are complete; nothing writes them outside an event.
    let slot = unsafe { &(*addr_of!(RX_QUEUE))[tail as usize % RX_SLOTS] };
    let n = (slot[1] as usize).min(LL_PAYLOAD_MAX);
    payload[..n].copy_from_slice(&slot[2..2 + n]);
    put!(RX_TAIL, tail.wrapping_add(1));
    Some((slot[0], n))
}

/// Sets the radio up for the mesh between bulbs: a data channel packet
/// format with the network's access code and the advertising CRC.
fn mesh_setup(access_code: u32) {
    trx_off();
    stop();
    write32(RF_ACCESS, access_code);
    write32(RF_CRC_INIT, ADV_CRC_INIT);
    write16(RF_TX_SETTLE, ADV_TX_SETTLE);
    write16(RF_IRQ_MASK, 0);
}

/// Sends one mesh packet on each of `channels`. False when it was left out
/// (packets of the connection wait in the radio's FIFO).
pub fn mesh_send(channels: &[u8], access_code: u32, pdu: &[u8]) -> bool {
    if pdu.len() < 2 || pdu.len() > ADV_PDU_MAX {
        return true;
    }
    // SAFETY: the radio is not transmitting from this buffer now.
    unsafe {
        let a = &mut *addr_of_mut!(ADV.0);
        a[0] = pdu.len() as u8;
        a[4..4 + pdu.len()].copy_from_slice(pdu);
    }
    #[cfg(feature = "trace")]
    let t_ev = tick();
    let_zigbee_finish(tick().wrapping_add(ADV_PATIENCE));
    let s = enter();
    if read8(TX_FIFO_WPTR) & 0x0f != read8(TX_FIFO_RPTR) & 0x0f {
        // Packets of the connection wait in the radio's FIFO. What a plain
        // transmission sends then is not documented: this burst is left out
        // until the FIFO is empty (`ble::Hal::mesh_send`).
        leave(s);
        return false;
    }
    count!(mesh_bursts);
    mesh_setup(access_code);
    // air time of the packet with preamble, access code and CRC, and the settle time
    let air = (pdu.len() as u32 + 8) * 8 + 100;
    for chn in channels {
        stop();
        set_channel(*chn);
        write16(RF_IRQ_STATUS, STATUS_TX);
        // SAFETY: only the address is taken.
        command(0x85, tick().wrapping_add(30 * TICKS_PER_US), unsafe { addr_of!(ADV.0) as *const u8 });
        let t0 = tick();
        // the end of the transmission, and not before the packet has left the air
        while (read16(RF_IRQ_STATUS) & STATUS_TX == 0 || tick().wrapping_sub(t0) < air * TICKS_PER_US)
            && tick().wrapping_sub(t0) < 1500 * TICKS_PER_US
        {}
        write16(RF_IRQ_STATUS, STATUS_TX);
    }
    stop();
    write16(RF_IRQ_STATUS, 0xffff);
    leave(s);
    #[cfg(feature = "trace")]
    crate::trace::span(crate::trace::BLE, 2, 0, t_ev);
    true
}

/// Listens on a mesh channel for up to `ticks`; the first packet with a good CRC is returned.
pub fn mesh_listen(channel: u8, access_code: u32, ticks: u32, pdu: &mut [u8; ADV_PDU_MAX]) -> Option<usize> {
    #[cfg(feature = "trace")]
    let t_ev = tick();
    let_zigbee_finish(tick().wrapping_add(ADV_PATIENCE));
    let s = enter();
    mesh_setup(access_code);
    set_channel(channel);
    let p = rx_buffer(get!(RX_CURRENT));
    write16(DMA_RX_ADDR, p as usize as u16);
    write16(RF_IRQ_STATUS, STATUS_RX);
    // receiver on, without the link layer state machine
    write8(RF_RX_CTRL, 0x81);
    write8(RF_MODE_CTRL, 0x65);
    let t0 = tick();
    let mut found = None;
    while tick().wrapping_sub(t0) < ticks {
        if read16(RF_IRQ_STATUS) & STATUS_RX == 0 {
            continue;
        }
        write16(RF_IRQ_STATUS, STATUS_RX);
        // SAFETY: the buffer was written by the DMA; lengths are checked before use.
        let current = (get!(RX_CURRENT) & 1) as usize;
        let buf = unsafe { &*addr_of!(RX[current].0) };
        let dma_len = u32::from_le_bytes([buf[0], buf[1], buf[2], buf[3]]) as usize;
        let n = buf[5] as usize;
        if dma_len == n + 13 && n + 2 <= ADV_PDU_MAX && buf[dma_len + 3] & 0x01 == 0 {
            pdu[..n + 2].copy_from_slice(&buf[4..6 + n]);
            found = Some(n + 2);
            break;
        }
        write16(DMA_RX_ADDR, p as usize as u16);
    }
    trx_off();
    write16(RF_IRQ_STATUS, 0xffff);
    if found.is_some() {
        count!(mesh_received);
    }
    leave(s);
    #[cfg(feature = "trace")]
    crate::trace::span(crate::trace::BLE, 3, 0, t_ev);
    found
}

/// How often and for how many milliseconds the radio was lent to BLE since the start.
pub fn lent() -> (u32, u32) {
    (get!(LENDS), get!(LENT_MS))
}

/// The longest single loan of the radio to BLE since the start, microseconds.
pub fn lent_max_us() -> u32 {
    get!(LENT_MAX_US)
}
