use ble::hal::{ConnEvent, EventResult, ADV_PDU_MAX, CONNECT_LEN, LL_PAYLOAD_MAX};

use crate::cell::{Shared, SharedBuf};
use crate::radio;
use crate::reg::{self, clear8, modify8, read16, read32, read8, set8, tick, write16, write32, write8, TICKS_PER_US};

const RF_CMD: usize = 0xf00;
const RF_SN_RESET: usize = 0xf01;
const RF_MODE_CTRL: usize = 0xf02;
const RF_LL_CTRL: usize = 0xf03;
const RF_TX_SETTLE: usize = 0xf04;
const RF_RX_TIMEOUT: usize = 0xf0a;
const RF_CMD_CTRL: usize = 0xf16;
const RF_START_TICK: usize = 0xf18;
const RF_IRQ_MASK: usize = 0xf1c;
const RF_IRQ_STATUS: usize = 0xf20;
const RF_SN: usize = 0xf22;
const RF_NESN: usize = 0xf23;
const RF_FIRST_TIMEOUT: usize = 0xf28;
const RF_FSM_TIMEOUT: usize = 0xf2c;
const RF_ACCESS: usize = 0x408;
const RF_ACCESS_CTRL: usize = 0x405;
const RF_CHANNEL: usize = 0x40d;
const RF_CRC_INIT: usize = 0x424;
const RF_RX_CTRL: usize = 0x428;
const RF_TIMESTAMP: usize = 0x450;
const BASEBAND_RESET: usize = 0x61;
const DMA_RX_ADDR: usize = 0xc08;
const DMA_TX_ADDR: usize = 0xc0c;
const DMA_CHANNEL_ENABLE: usize = 0xc20;
const DMA_FIFO: usize = 0xc28;
const TX_FIFO_RPTR: usize = 0xc2a;
const TX_FIFO_WPTR: usize = 0xc2b;
const TX_FIFO_PUSH: usize = 0xc2c;

const STATUS_RX: u16 = 0x0001;
const STATUS_TX: u16 = 0x0002;
const STATUS_RX_TX: u16 = 0x0003;
const STATUS_END: u16 = 0x0474;

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
const ADV_TX_SETTLE: u16 = 74;
const CONN_TX_SETTLE: u16 = 75;

const RX_LEAD: u32 = 300 * TICKS_PER_US;
const SETUP: u32 = 400 * TICKS_PER_US;
const FIRST_RX_GRACE: u32 = 400 * TICKS_PER_US;
const RX_GUARD: u32 = 2100 * TICKS_PER_US;
const TIMESTAMP_TO_ANCHOR: u32 = 0x500;
const ADV_PATIENCE: u32 = 6000 * TICKS_PER_US;
const NEIGHBOURS_QUIET: u32 = 64_000 * TICKS_PER_US;

const RX_LEN: usize = 64;
static RX: [SharedBuf<RX_LEN>; 2] = [SharedBuf::new([0; RX_LEN]), SharedBuf::new([0; RX_LEN])];
static RX_CURRENT: Shared<u8> = Shared::new(0);

static ADV: SharedBuf<48> = SharedBuf::new([0; 48]);
static SCAN_RSP: SharedBuf<48> = SharedBuf::new([0; 48]);
static EMPTY: SharedBuf<8> = SharedBuf::new([2, 0, 0, 0, 1, 0, 0, 0]);

const TX_SLOTS: usize = 8;
const TX_SLOT: usize = 36;
static TX_QUEUE: SharedBuf<{ TX_SLOTS * TX_SLOT }> = SharedBuf::new([0; TX_SLOTS * TX_SLOT]);
static TX_HEAD: Shared<u8> = Shared::new(0);
static TX_HANDED: Shared<u8> = Shared::new(0);
static TX_TAIL: Shared<u8> = Shared::new(0);
static FIFO_KIND: Shared<[u8; 16]> = Shared::new([0; 16]);
static SN_NESN: Shared<u8> = Shared::new(0x10);
static FIFO_RPTR: Shared<u8> = Shared::new(0);
static FIFO_FREED: Shared<u8> = Shared::new(0);
static FIFO_WORD: Shared<u32> = Shared::new(0);
static FIFO_STARTED: Shared<u8> = Shared::new(0);
static RESET_PENDING: Shared<u8> = Shared::new(0);

const RX_SLOTS: usize = 8;
const RX_SLOT: usize = 2 + LL_PAYLOAD_MAX;
static RX_QUEUE: SharedBuf<{ RX_SLOTS * RX_SLOT }> = SharedBuf::new([0; RX_SLOTS * RX_SLOT]);
static RX_HEAD: Shared<u8> = Shared::new(0);
static RX_TAIL: Shared<u8> = Shared::new(0);

static CONNECT_TICK: Shared<u32> = Shared::new(0);
static ANCHOR: Shared<u32> = Shared::new(0);
static ANCHOR_SEEN: Shared<u8> = Shared::new(0);

static CRC_TABLE: Shared<[u32; 16]> = Shared::new(crc_table());

#[allow(unsafe_code)]
#[no_mangle]
pub static BLE_RADIO: Shared<[u32; 7]> = Shared::new([0; 7]);
const ADV_EVENTS: usize = 0;
const CONN_EVENTS: usize = 1;
const HEARD: usize = 2;
const CRC_ERRORS: usize = 3;
const RX_OVERFLOWS: usize = 4;
const MESH_BURSTS: usize = 5;
const MESH_RECEIVED: usize = 6;

#[inline(always)]
fn count(i: usize) {
    if let Some(n) = BLE_RADIO.at(i) {
        BLE_RADIO.set_at(i, n.wrapping_add(1));
    }
}

static LENDS: Shared<u32> = Shared::new(0);
static LENT_MS: Shared<u32> = Shared::new(0);
static LENT_MAX_US: Shared<u32> = Shared::new(0);
static LENT_REST: Shared<u32> = Shared::new(0);
static LENT_AT: Shared<u32> = Shared::new(0);

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

fn reverse24(v: u32) -> u32 {
    (v << 8).reverse_bits() & 0x00ff_ffff
}

#[inline(always)]
fn rx_buffer(i: u8) -> &'static SharedBuf<RX_LEN> {
    if i & 1 == 0 { &RX[0] } else { &RX[1] }
}

#[inline(always)]
fn reached(now: u32, t: u32) -> bool {
    now.wrapping_sub(t) as i32 >= 0
}

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

#[inline(always)]
fn command(cmd: u8, at: u32, tx: usize) {
    write32(RF_START_TICK, at);
    set8(RF_CMD_CTRL, 0x04);
    write8(RF_CMD, cmd);
    write16(DMA_TX_ADDR, tx as u16);
}

#[allow(unsafe_code)]
#[cfg_attr(target_os = "none", link_section = ".ram_code")]
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
    let band = if f > 2464 {
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
    modify8(0x1228, 0xc0, idx);
    modify8(0x1229, 0xc3, band);
}

fn enter() -> u8 {
    let s = radio::suspend();
    let lends = LENDS.get();
    LENDS.set(lends.wrapping_add(1));
    LENT_AT.set(tick());
    clear8(DMA_CHANNEL_ENABLE, 0x08);
    clear8(DMA_CHANNEL_ENABLE, 0x04);
    trx_off();
    write8(BASEBAND_RESET, 0x01);
    write8(BASEBAND_RESET, 0x00);
    set8(BASEBAND_RESET, 0x01);
    clear8(BASEBAND_RESET, 0x01);
    let [lo, hi] = (rx_buffer(RX_CURRENT.get()).addr() as u16).to_le_bytes();
    write32(DMA_RX_ADDR, u32::from_le_bytes([lo, hi, 0x04, 0x01]));
    if FIFO_STARTED.get() != 0 {
        write32(DMA_FIFO, FIFO_WORD.get());
    }
    if RESET_PENDING.get() != 0 || FIFO_STARTED.get() == 0 {
        RESET_PENDING.set(0);
        FIFO_STARTED.set(1);
        write8(TX_FIFO_RPTR, 0x10);
        let rptr = read8(TX_FIFO_RPTR) & 0x0f;
        FIFO_RPTR.set(rptr);
        FIFO_FREED.set(rptr);
    }
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
    set8(0x1225, 0x40);
    modify8(0x1226, 0x7f, 0x80);
    modify8(0x1227, 0xe0, 0x1f);
    s
}

fn leave(s: u8) {
    let (rest, at, ms) = (LENT_REST.get(), LENT_AT.get(), LENT_MS.get());
    let this = tick().wrapping_sub(at) / TICKS_PER_US;
    LENT_MAX_US.update(|m| m.max(this));
    let ticks = rest + tick().wrapping_sub(at);
    LENT_MS.set(ms.wrapping_add(ticks / (1000 * TICKS_PER_US)));
    LENT_REST.set(ticks % (1000 * TICKS_PER_US));
    trx_off();
    FIFO_WORD.set(read32(DMA_FIFO));
    clear8(DMA_CHANNEL_ENABLE, 0x04);
    radio::resume(s);
}

fn let_zigbee_finish(latest: u32) {
    while radio::receiving() && !reached(tick(), latest) {}
}

fn wait_for_quiet(latest: u32) {
    while (radio::receiving() || radio::rx_quiet_ticks() < crate::ble::RADIO_QUIET) && !reached(tick(), latest) {}
}

#[allow(unsafe_code)]
#[cfg_attr(target_os = "none", link_section = ".ram_code")]
#[inline(never)]
extern "C" fn adv_channel(chn: u8, answer_window: u32) -> u8 {
    let p = rx_buffer(RX_CURRENT.get());
    let header = || p.read32(4);
    let at = |i: usize| p.read(i);
    stop();
    set_channel(chn);
    write16(RF_IRQ_STATUS, STATUS_RX_TX);
    p.write32(4, 0);
    command(0x87, tick().wrapping_add(100), ADV.addr());
    let t0 = tick();
    while read16(RF_IRQ_STATUS) & STATUS_TX == 0 && tick().wrapping_sub(t0) < 2000 * TICKS_PER_US {}
    write16(RF_IRQ_STATUS, STATUS_TX);
    while header() == 0 && tick().wrapping_sub(t0) < answer_window {}
    if header() == 0 {
        return 0;
    }
    let t1 = tick();
    write16(DMA_TX_ADDR, SCAN_RSP.addr() as u16);
    while read16(RF_IRQ_STATUS) & STATUS_RX == 0 && tick().wrapping_sub(t1) < 6400 {}
    write32(RF_START_TICK, tick().wrapping_add(48));
    write8(RF_CMD, 0x85);
    write16(RF_IRQ_STATUS, STATUS_RX);

    let dma_len = header_len(p);
    let rf_len = at(5) as usize;
    let mut ours = dma_len == rf_len + 13 && at(dma_len + 3) & 0x01 == 0 && at(4) & 0x80 == 0;
    let mut i = 0;
    while ours && i < 6 {
        ours = at(12 + i) == ADV.read(6 + i);
        i += 1;
    }
    let kind = at(4) & 0x0f;
    if ours && kind == 3 && rf_len == 12 {
        let rsp_len = SCAN_RSP.read(5) as u32;
        let t2 = tick();
        while tick().wrapping_sub(t2) < (rsp_len * 8 + 204) * TICKS_PER_US {}
        stop();
        return 1;
    }
    stop();
    if ours && kind == 5 && rf_len == CONNECT_LEN {
        CONNECT_TICK.set(tick());
        return 2;
    }
    0
}

#[inline(always)]
fn header_len(p: &SharedBuf<RX_LEN>) -> usize {
    p.read32(0) as usize
}

pub fn advertise(adv: &[u8], scan_rsp: &[u8], connect: &mut [u8; CONNECT_LEN]) -> Option<u32> {
    if adv.len() < 2 || adv.len() > ADV_PDU_MAX || scan_rsp.len() < 2 || scan_rsp.len() > ADV_PDU_MAX {
        return None;
    }
    ADV.write(0, adv.len() as u8);
    ADV.copy_from(4, adv);
    SCAN_RSP.write(0, scan_rsp.len() as u8);
    SCAN_RSP.copy_from(4, scan_rsp);
    #[cfg(feature = "trace")]
    let t_ev = tick();
    wait_for_quiet(tick().wrapping_add(ADV_PATIENCE));
    let s = enter();
    count(ADV_EVENTS);
    trx_off();
    write32(RF_ACCESS, ADV_ACCESS);
    write32(RF_CRC_INIT, ADV_CRC_INIT);
    write16(RF_TX_SETTLE, ADV_TX_SETTLE);
    write16(RF_IRQ_MASK, 0);
    let answer_window = (adv[1] as u32 + 60) * 8 * TICKS_PER_US;
    let channels: &[u8] = if radio::rx_quiet_ticks() < NEIGHBOURS_QUIET { &[37] } else { &[37, 38, 39] };
    let mut result = None;
    for &chn in channels {
        match adv_channel(chn, answer_window) {
            0 => {}
            2 => {
                rx_buffer(RX_CURRENT.get()).copy_to(6, connect);
                result = Some(CONNECT_TICK.get());
                break;
            }
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

pub fn connection_reset() {
    TX_HEAD.set(0);
    TX_HANDED.set(0);
    TX_TAIL.set(0);
    RX_HEAD.set(0);
    RX_TAIL.set(0);
    SN_NESN.set(0x10);
    RESET_PENDING.set(1);
}

#[repr(C)]
struct Brx {
    channel: u8,
    access: u32,
    crc_init: u32,
    crc_start: u32,
    open: u32,
    first_by: u32,
    deadline: u32,
    fsm_timeout_us: u32,
}

#[allow(unsafe_code)]
#[cfg_attr(target_os = "none", link_section = ".ram_code")]
#[inline(never)]
extern "C" fn brx(e: &Brx) {
    let mut current = RX_CURRENT.get();
    trx_off();
    stop();
    write8(RF_SN_RESET, 0x01);
    write16(RF_IRQ_MASK, 0x0477);
    write16(RF_IRQ_STATUS, 0xffff);
    write8(BASEBAND_RESET, 0x01);
    write8(BASEBAND_RESET, 0x00);
    set_channel(e.channel);
    write32(RF_ACCESS, e.access);
    set8(RF_ACCESS_CTRL, 0x80);
    write32(RF_CRC_INIT, e.crc_init);

    clear8(RF_LL_CTRL, 0x30);
    set8(RF_LL_CTRL, SN_NESN.get());
    write8(TX_FIFO_RPTR, 0x40 + FIFO_RPTR.get());
    let mut waiting = read8(TX_FIFO_WPTR).wrapping_sub(read8(TX_FIFO_RPTR)) & 0x0f;
    let mut handed = TX_HANDED.get();
    let tail = TX_TAIL.get();
    let empty = EMPTY.addr();
    if waiting == 0 && handed != tail {
        FIFO_KIND.set_at((read8(TX_FIFO_WPTR) & 0x0f) as usize, 0);
        write16(TX_FIFO_PUSH, empty as u16);
        waiting = 1;
    }
    while waiting < 7 && handed != tail {
        FIFO_KIND.set_at((read8(TX_FIFO_WPTR) & 0x0f) as usize, 1);
        write16(TX_FIFO_PUSH, (TX_QUEUE.addr() + handed as usize % TX_SLOTS * TX_SLOT) as u16);
        handed = handed.wrapping_add(1);
        waiting += 1;
    }
    TX_HANDED.set(handed);

    write16(RF_TX_SETTLE, CONN_TX_SETTLE);
    write16(RF_RX_TIMEOUT, 250);
    set8(RF_LL_CTRL, 0x01);
    write32(RF_FSM_TIMEOUT, e.fsm_timeout_us);
    write32(RF_FIRST_TIMEOUT, 0x0fff_ffff);
    write16(DMA_RX_ADDR, rx_buffer(current).addr() as u16);
    let now = tick();
    let open = if e.open.wrapping_sub(now) as i32 <= 10 { now.wrapping_add(10) } else { e.open };
    command(0x82, open, empty);

    let mut packets = 0u32;
    let mut guard = e.first_by;
    loop {
        let status = read16(RF_IRQ_STATUS);
        if status & STATUS_RX != 0 {
            let p = rx_buffer(current);
            current ^= 1;
            write16(DMA_RX_ADDR, rx_buffer(current).addr() as u16);
            write16(RF_IRQ_STATUS, STATUS_RX);
            let t = tick();
            guard = t.wrapping_add(RX_GUARD);
            let at = |i: usize| p.read(i);
            let dma_len = header_len(p);
            let n = at(5) as usize;
            let first = packets == 0;
            packets += 1;
            if dma_len == n + 13 && n + 17 <= RX_LEN && at(dma_len + 3) & 0x01 == 0 {
                let mut crc = e.crc_start;
                let mut i = 4;
                while i < 6 + n {
                    crc ^= at(i) as u32;
                    crc = (crc >> 4) ^ CRC_TABLE.at((crc & 0x0f) as usize).unwrap_or(0);
                    crc = (crc >> 4) ^ CRC_TABLE.at((crc & 0x0f) as usize).unwrap_or(0);
                    i += 1;
                }
                let crc_good = at(6 + n) == crc as u8 && at(7 + n) == (crc >> 8) as u8 && at(8 + n) == (crc >> 16) as u8;
                let sn = (read8(RF_SN) & 0x01) << 4;
                let nesn = (read8(RF_NESN) & 0x10) << 1;
                let fresh = nesn != SN_NESN.get() & 0x20;
                let head = RX_HEAD.get();
                let full = head.wrapping_sub(RX_TAIL.get()) as usize >= RX_SLOTS;
                if !crc_good || n > LL_PAYLOAD_MAX || (fresh && n != 0 && full) {
                    stop();
                    if crc_good {
                        count(RX_OVERFLOWS);
                    } else {
                        count(CRC_ERRORS);
                    }
                    break;
                }
                SN_NESN.set(sn + nesn);
                FIFO_RPTR.set(read8(TX_FIFO_RPTR) & 0x0f);
                if first {
                    let stamp = read32(RF_TIMESTAMP);
                    if t.wrapping_sub(stamp) < 2000 * TICKS_PER_US {
                        ANCHOR.set(stamp.wrapping_sub(TIMESTAMP_TO_ANCHOR));
                        ANCHOR_SEEN.set(1);
                    }
                }
                if fresh && n != 0 {
                    let slot = head as usize % RX_SLOTS * RX_SLOT;
                    RX_QUEUE.write(slot, at(4) & 0x03);
                    RX_QUEUE.write(slot + 1, n as u8);
                    let mut i = 0;
                    while i < n {
                        RX_QUEUE.write(slot + 2 + i, at(6 + i));
                        i += 1;
                    }
                    RX_HEAD.set(head.wrapping_add(1));
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
    RX_CURRENT.set(current);
}

pub fn connection_event(ev: &ConnEvent) -> EventResult {
    let open = ev.start.wrapping_sub(RX_LEAD);
    let take = open.wrapping_sub(SETUP);
    while !reached(tick(), take) {}
    let_zigbee_finish(open.wrapping_sub(SETUP / 4));
    #[cfg(feature = "trace")]
    let t_ev = tick();
    let s = enter();
    count(CONN_EVENTS);
    ANCHOR_SEEN.set(0);
    let now = tick();
    let e = Brx {
        channel: ev.channel,
        access: ev.access_address.swap_bytes(),
        crc_init: ev.crc_init & 0x00ff_ffff,
        crc_start: reverse24(ev.crc_init),
        open,
        first_by: ev.start.wrapping_add(ev.window).wrapping_add(FIRST_RX_GRACE),
        deadline: ev.deadline,
        fsm_timeout_us: ev.deadline.wrapping_sub(now) / TICKS_PER_US + 1000,
    };
    brx(&e);
    let rptr = FIFO_RPTR.get();
    let mut freed = FIFO_FREED.get();
    let mut head = TX_HEAD.get();
    while freed != rptr {
        if FIFO_KIND.at(freed as usize) == Some(1) {
            head = head.wrapping_add(1);
        }
        freed = (freed + 1) & 0x0f;
    }
    FIFO_FREED.set(freed);
    TX_HEAD.set(head);
    leave(s);
    #[cfg(feature = "trace")]
    crate::trace::span(crate::trace::BLE, 1, 0, t_ev);
    if ANCHOR_SEEN.get() != 0 {
        count(HEARD);
        EventResult { anchor: Some(ANCHOR.get()) }
    } else {
        EventResult { anchor: None }
    }
}

pub fn tx_free() -> usize {
    TX_SLOTS - TX_TAIL.get().wrapping_sub(TX_HEAD.get()) as usize
}

pub fn tx_idle() -> bool {
    TX_TAIL.get() == TX_HEAD.get()
}

pub fn tx_push(llid: u8, payload: &[u8]) -> bool {
    if tx_free() == 0 || payload.len() > LL_PAYLOAD_MAX {
        return false;
    }
    let tail = TX_TAIL.get();
    let slot = tail as usize % TX_SLOTS * TX_SLOT;
    TX_QUEUE.write32(slot, payload.len() as u32 + 2);
    TX_QUEUE.write(slot + 4, llid & 0x03);
    TX_QUEUE.write(slot + 5, payload.len() as u8);
    TX_QUEUE.copy_from(slot + 6, payload);
    TX_TAIL.set(tail.wrapping_add(1));
    true
}

pub fn rx_pop(payload: &mut [u8; LL_PAYLOAD_MAX]) -> Option<(u8, usize)> {
    let tail = RX_TAIL.get();
    if tail == RX_HEAD.get() {
        return None;
    }
    let slot = tail as usize % RX_SLOTS * RX_SLOT;
    let n = (RX_QUEUE.read(slot + 1) as usize).min(LL_PAYLOAD_MAX);
    RX_QUEUE.copy_to(slot + 2, &mut payload[..n]);
    RX_TAIL.set(tail.wrapping_add(1));
    Some((RX_QUEUE.read(slot), n))
}

fn mesh_setup(access_code: u32) {
    trx_off();
    stop();
    write32(RF_ACCESS, access_code);
    write32(RF_CRC_INIT, ADV_CRC_INIT);
    write16(RF_TX_SETTLE, ADV_TX_SETTLE);
    write16(RF_IRQ_MASK, 0);
}

pub fn mesh_send(channels: &[u8], access_code: u32, pdu: &[u8]) -> bool {
    if pdu.len() < 2 || pdu.len() > ADV_PDU_MAX {
        return true;
    }
    ADV.write(0, pdu.len() as u8);
    ADV.copy_from(4, pdu);
    #[cfg(feature = "trace")]
    let t_ev = tick();
    let_zigbee_finish(tick().wrapping_add(ADV_PATIENCE));
    let s = enter();
    if read8(TX_FIFO_WPTR) & 0x0f != read8(TX_FIFO_RPTR) & 0x0f {
        leave(s);
        return false;
    }
    count(MESH_BURSTS);
    mesh_setup(access_code);
    let air = (pdu.len() as u32 + 8) * 8 + 100;
    for chn in channels {
        stop();
        set_channel(*chn);
        write16(RF_IRQ_STATUS, STATUS_TX);
        command(0x85, tick().wrapping_add(30 * TICKS_PER_US), ADV.addr());
        let t0 = tick();
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

pub fn mesh_listen(channel: u8, access_code: u32, ticks: u32, pdu: &mut [u8; ADV_PDU_MAX]) -> Option<usize> {
    #[cfg(feature = "trace")]
    let t_ev = tick();
    let_zigbee_finish(tick().wrapping_add(ADV_PATIENCE));
    let s = enter();
    mesh_setup(access_code);
    set_channel(channel);
    let p = rx_buffer(RX_CURRENT.get());
    write16(DMA_RX_ADDR, p.addr() as u16);
    write16(RF_IRQ_STATUS, STATUS_RX);
    write8(RF_RX_CTRL, 0x81);
    write8(RF_MODE_CTRL, 0x65);
    let t0 = tick();
    let mut found = None;
    while tick().wrapping_sub(t0) < ticks {
        if read16(RF_IRQ_STATUS) & STATUS_RX == 0 {
            continue;
        }
        write16(RF_IRQ_STATUS, STATUS_RX);
        let dma_len = header_len(p);
        let n = p.read(5) as usize;
        if dma_len == n + 13 && n + 2 <= ADV_PDU_MAX && p.read(dma_len + 3) & 0x01 == 0 {
            p.copy_to(4, &mut pdu[..n + 2]);
            found = Some(n + 2);
            break;
        }
        write16(DMA_RX_ADDR, p.addr() as u16);
    }
    trx_off();
    write16(RF_IRQ_STATUS, 0xffff);
    if found.is_some() {
        count(MESH_RECEIVED);
    }
    leave(s);
    #[cfg(feature = "trace")]
    crate::trace::span(crate::trace::BLE, 3, 0, t_ev);
    found
}

pub fn lent() -> (u32, u32) {
    (LENDS.get(), LENT_MS.get())
}

pub fn lent_max_us() -> u32 {
    LENT_MAX_US.get()
}

#[cfg(test)]
#[path = "tests/ble_radio.rs"]
mod tests;
