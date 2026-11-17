use zigbee::TxStatus;

use crate::cell::{SharedBuf, Shared};
use crate::flash;
use crate::reg::{self, clear8, modify16, modify8, read16, read8, set8, tick, write16, write32, write8};

const RF_MODE_CTRL: usize = 0xf02;
const RF_RX_CTRL: usize = 0x428;
const RF_IRQ_MASK: usize = 0xf1c;
const RF_IRQ_STATUS: usize = 0xf20;
const RF_IRQ_RX: u8 = 0x01;
const RF_IRQ_TX: u8 = 0x02;
const RF_RSSI: usize = 0x441;
const RF_RX_STATUS: usize = 0x448;
const DMA_RX_ADDR: usize = 0xc08;
const DMA_RX_SIZE: usize = 0xc0a;
const DMA_RX_MODE: usize = 0xc0b;
const DMA_TX_ADDR: usize = 0xc0c;
const DMA_CHANNEL_ENABLE: usize = 0xc20;
const DMA_CHANNEL_RF_RX: u8 = 0x04;
const DMA_CHANNEL_RF_TX: u8 = 0x08;
const DMA_CHANNEL_RF: u8 = 0x0c;
const DMA_IRQ_ENABLE: usize = 0xc21;
const DMA_TX_READY: usize = 0xc24;
const DMA_IRQ_STATUS: usize = 0xc26;
const DMA_TX_ADDR_HIGH: usize = 0xc43;

const INIT_TABLE: [(u16, u8); 35] = [
    (0x12d2, 0x9b),
    (0x12d3, 0x19),
    (0x127b, 0x0e),
    (0x1276, 0x50),
    (0x1277, 0x73),
    (0x0430, 0x3e),
    (0x1220, 0x04),
    (0x1221, 0x2b),
    (0x1222, 0x43),
    (0x1223, 0x86),
    (0x122a, 0x90),
    (0x1254, 0x0e),
    (0x1255, 0x09),
    (0x1256, 0x0c),
    (0x1257, 0x08),
    (0x1258, 0x09),
    (0x1259, 0x0f),
    (0x0400, 0x13),
    (0x0420, 0x18),
    (0x0402, 0x46),
    (0x0404, 0xc0),
    (0x0405, 0x04),
    (0x0421, 0x23),
    (0x0422, 0x04),
    (0x0408, 0xa7),
    (0x0409, 0x00),
    (0x040a, 0x00),
    (0x040b, 0x00),
    (0x0460, 0x36),
    (0x0461, 0x46),
    (0x0462, 0x51),
    (0x0463, 0x61),
    (0x0464, 0x6d),
    (0x0465, 0x78),
    (0x0401, 0x00),
];

const CAP_TRIM_ADDR: usize = 0xfe000;

const RX_BUF_LEN: usize = 144;
const SLOTS: usize = 16;
const SLOT_LEN: usize = 131;

const STATE_TX: u8 = 0;
const STATE_RX: u8 = 1;
const STATE_OFF: u8 = 3;

const ACK_TURNAROUND_US: u32 = 160;
const ACK_LATEST_US: u32 = 600;
const TX_SETTLE_US: u32 = 120;
const RX_SETTLE_US: u32 = 85;
const CCA_WINDOW_US: u32 = 128;
const CCA_THRESHOLD_DBM: i32 = -85;
const ACK_WAIT_US: u32 = 2000;
const MIN_BE: u32 = 3;
const MAX_BE: u32 = 5;
const MAX_CCA_ATTEMPTS: u32 = 5;
const MAX_FRAME_RETRIES: u32 = 3;
const BACKOFF_US: u32 = 320;
const RX_GAP_US: u32 = 1200;

static RX_BUF: SharedBuf<RX_BUF_LEN> = SharedBuf::new([0; RX_BUF_LEN]);
static TX_BUF: SharedBuf<132> = SharedBuf::new([0; 132]);
static ACK_BUF: SharedBuf<12> = SharedBuf::new([4, 0, 0, 0, 5, 0x02, 0x00, 0, 0, 0, 0, 0]);

static QUEUE: SharedBuf<{ SLOTS * SLOT_LEN }> = SharedBuf::new([0; SLOTS * SLOT_LEN]);
static HEAD: Shared<u8> = Shared::new(0);
static TAIL: Shared<u8> = Shared::new(0);

static FREQ_OFFSET: Shared<u8> = Shared::new(5);
static PAN: Shared<u16> = Shared::new(0xffff);
static PROBE_PAN: Shared<u16> = Shared::new(0xffff);
static SHORT: Shared<u16> = Shared::new(0xffff);
static EXT: Shared<[u8; 8]> = Shared::new([0; 8]);

static ACK_WANTED: Shared<u8> = Shared::new(0);
static ACK_SEQ: Shared<u8> = Shared::new(0);
static ACK_SEEN: Shared<u8> = Shared::new(0);

const PENDING_SHORT_SLOTS: usize = zigbee::stack::PENDING_SHORTS;
const PENDING_EXT_SLOTS: usize = zigbee::stack::INDIRECT;
static PENDING_SHORT: Shared<[u16; PENDING_SHORT_SLOTS]> = Shared::new([0xffff; PENDING_SHORT_SLOTS]);
static PENDING_EXT: Shared<[u8; 8 * PENDING_EXT_SLOTS]> = Shared::new([0; 8 * PENDING_EXT_SLOTS]);
static N_PENDING_SHORT: Shared<u8> = Shared::new(0);
static N_PENDING_EXT: Shared<u8> = Shared::new(0);

const CHILD_SLOTS: usize = zigbee::stack::CHILDREN;
static CHILDREN: Shared<[u16; CHILD_SLOTS]> = Shared::new([0xffff; CHILD_SLOTS]);
static N_CHILDREN: Shared<u8> = Shared::new(0);

#[cfg(feature = "trace")]
pub static LAST_TX: Shared<[u32; 3]> = Shared::new([0; 3]);

static LAST_AIR: Shared<u32> = Shared::new(0);
static LAST_RX: Shared<u32> = Shared::new(0);

#[allow(unsafe_code)]
#[no_mangle]
pub static RADIO_DROPPED: Shared<u32> = Shared::new(0);
static RX_PEAK: Shared<u8> = Shared::new(0);
static CCA_BUSY: Shared<u32> = Shared::new(0);
static TX_SENT: Shared<u8> = Shared::new(0);

#[allow(unsafe_code)]
#[cfg_attr(target_os = "none", link_section = ".ram_code")]
#[inline(never)]
extern "C" fn set_state(state: u8) {
    write8(RF_MODE_CTRL, 0x45);
    let f = 2400 + FREQ_OFFSET.get() as u16;
    let band = match f {
        ..=2434 => 0x14,
        2435..=2464 => 0x10,
        2465.. => 0x0c,
    };
    write8(0x1228, 0x13);
    write8(0x1244, (f * 4 + 1) as u8);
    modify8(0x1245, 0xc0, (f >> 6) as u8 & 0x3f);
    modify8(0x1229, 0xc3, band);
    if state == STATE_TX {
        write8(RF_MODE_CTRL, 0x55);
        clear8(RF_RX_CTRL, 0x01);
    } else if state == STATE_RX {
        write8(RF_MODE_CTRL, 0x65);
        set8(RF_RX_CTRL, 0x01);
    } else {
        write8(0xf16, 0x29);
        write8(RF_RX_CTRL, 0xe0);
        write8(RF_MODE_CTRL, 0x45);
    }
}

#[inline(always)]
fn start_tx(buf: usize) {
    write8(DMA_TX_ADDR_HIGH, 0x04);
    write16(DMA_TX_ADDR, buf as u16);
    set8(DMA_TX_READY, DMA_CHANNEL_RF_TX);
}

pub fn init() {
    for (addr, value) in INIT_TABLE {
        write8(addr as usize, value);
    }
    set8(DMA_CHANNEL_ENABLE, DMA_CHANNEL_RF);
    let mut trim = [0u8; 1];
    flash::read(CAP_TRIM_ADDR, &mut trim);
    if trim[0] != 0xff {
        reg::analog_modify(0x8a, 0x40, trim[0] & 0x3f);
    }

    set_state(STATE_OFF);
    clear8(0x1225, 0x40);
    modify8(0x1226, 0x7f, 0x80);
    modify8(0x1227, 0xe0, 0x1f);

    RX_BUF.write(0, 0);
    RX_BUF.write(4, 0);
    write16(DMA_RX_ADDR, RX_BUF.addr() as u16);
    clear8(0xf03, 0x04);
    write8(DMA_RX_SIZE, (RX_BUF_LEN / 16) as u8);
    write8(DMA_RX_MODE, 0x01);
    clear8(DMA_IRQ_ENABLE, DMA_CHANNEL_RF);
    modify16(RF_IRQ_MASK, !0x1fff, RF_IRQ_RX as u16);
    reg::modify32(reg::IRQ_MASK, !0, reg::IRQ_MASK_RF);
    set8(DMA_CHANNEL_ENABLE, DMA_CHANNEL_RF_RX);
    set8(DMA_CHANNEL_ENABLE, DMA_CHANNEL_RF_TX);
    set8(0x430, 0x02);

    set_state(STATE_RX);
    write8(reg::IRQ_ENABLE, 1);
}

const LENT8: [u16; 9] = [0x124a, 0x124b, 0x1245, 0x40d, 0xf03, 0xf06, 0xf0c, 0xf0e, 0xf10];
const LENT16: [u16; 2] = [0xf04, 0xf0a];
const LENT32: [u16; 2] = [0x424, 0xf18];
static SAVED8: Shared<[u8; 9]> = Shared::new([0; 9]);
static SAVED16: Shared<[u16; 2]> = Shared::new([0; 2]);
static SAVED32: Shared<[u32; 2]> = Shared::new([0; 2]);

pub fn suspend() -> u8 {
    let s = reg::irq_disable();
    loop {
        set_state(STATE_OFF);
        if !rx_pending() {
            break;
        }
        irq_handler();
    }
    let mut saved8 = [0u8; 9];
    for (v, addr) in saved8.iter_mut().zip(LENT8) {
        *v = read8(addr as usize);
    }
    SAVED8.set(saved8);
    SAVED16.set([read16(LENT16[0] as usize), read16(LENT16[1] as usize)]);
    SAVED32.set([reg::read32(LENT32[0] as usize), reg::read32(LENT32[1] as usize)]);
    s
}

pub fn resume(s: u8) {
    write8(0x61, 0x01);
    write8(0x61, 0x00);
    set8(0x61, 0x01);
    clear8(0x61, 0x01);
    RX_BUF.write(0, 0);
    RX_BUF.write(4, 0);
    write16(RF_IRQ_STATUS, 0xffff);
    for (addr, value) in INIT_TABLE {
        write8(addr as usize, value);
    }
    for (addr, value) in LENT8.iter().zip(SAVED8.get()) {
        write8(*addr as usize, value);
    }
    for (addr, value) in LENT16.iter().zip(SAVED16.get()) {
        write16(*addr as usize, value);
    }
    for (addr, value) in LENT32.iter().zip(SAVED32.get()) {
        write32(*addr as usize, value);
    }
    set_state(STATE_OFF);
    clear8(0x1225, 0x40);
    modify8(0x1226, 0x7f, 0x80);
    modify8(0x1227, 0xe0, 0x1f);
    write16(DMA_RX_ADDR, RX_BUF.addr() as u16);
    write8(DMA_RX_SIZE, (RX_BUF_LEN / 16) as u8);
    write8(DMA_RX_MODE, 0x01);
    clear8(DMA_IRQ_ENABLE, DMA_CHANNEL_RF);
    modify16(RF_IRQ_MASK, !0x1fff, RF_IRQ_RX as u16);
    reg::modify32(reg::IRQ_MASK, !0, reg::IRQ_MASK_RF);
    set8(DMA_CHANNEL_ENABLE, DMA_CHANNEL_RF);
    set_state(STATE_RX);
    reg::irq_restore(s);
}

pub fn quiet_ticks() -> u32 {
    tick().wrapping_sub(LAST_AIR.get())
}

pub fn rx_quiet_ticks() -> u32 {
    tick().wrapping_sub(LAST_RX.get())
}

pub fn noise_dbm() -> Option<i32> {
    if quiet_ticks() < 3_000 * reg::TICKS_PER_US {
        return None;
    }
    let s = reg::irq_disable();
    if read8(RF_RX_STATUS) & 0x20 != 0 {
        reg::irq_restore(s);
        return None;
    }
    set_state(STATE_OFF);
    set_state(STATE_RX);
    reg::delay_us(RX_SETTLE_US);
    let t = tick();
    let mut sum = rssi_dbm();
    let mut count = 1;
    while tick().wrapping_sub(t) <= CCA_WINDOW_US * reg::TICKS_PER_US {
        sum += rssi_dbm();
        count += 1;
    }
    reg::irq_restore(s);
    Some(sum / count)
}

pub fn receiving() -> bool {
    read8(RF_RX_STATUS) & 0x20 != 0 || rssi_dbm() > CCA_THRESHOLD_DBM
}

pub fn set_channel(channel: u8) {
    if !(11..=26).contains(&channel) {
        return;
    }
    let s = reg::irq_disable();
    FREQ_OFFSET.set((channel - 10) * 5);
    set_state(STATE_RX);
    reg::irq_restore(s);
}

pub fn energy_detect(channel: u8, ms: u32) -> i32 {
    set_channel(channel);
    reg::delay_us(RX_SETTLE_US);
    let mut peak = -110;
    let t = tick();
    while tick().wrapping_sub(t) <= ms * 1000 * reg::TICKS_PER_US {
        peak = peak.max(rssi_dbm());
    }
    peak
}

pub fn set_filter(pan: u16, short: u16, ext: u64) {
    let s = reg::irq_disable();
    PAN.set(pan);
    SHORT.set(short);
    EXT.set(ext.to_le_bytes());
    reg::irq_restore(s);
}

#[cfg(feature = "sense")]
pub fn set_probe_pan(pan: u16) {
    PROBE_PAN.set(pan);
}

pub fn set_pending(shorts: &[u16], exts: &[u64]) {
    let mut short_table = [0xffffu16; PENDING_SHORT_SLOTS];
    let mut ext_table = [0u8; 8 * PENDING_EXT_SLOTS];
    let ns = shorts.len().min(PENDING_SHORT_SLOTS);
    let ne = exts.len().min(PENDING_EXT_SLOTS);
    short_table[..ns].copy_from_slice(&shorts[..ns]);
    for (slot, e) in ext_table.chunks_exact_mut(8).zip(&exts[..ne]) {
        slot.copy_from_slice(&e.to_le_bytes());
    }
    let s = reg::irq_disable();
    PENDING_SHORT.set(short_table);
    PENDING_EXT.set(ext_table);
    N_PENDING_SHORT.set(ns as u8);
    N_PENDING_EXT.set(ne as u8);
    reg::irq_restore(s);
}

pub fn set_children(shorts: &[u16]) {
    let mut table = [0xffffu16; CHILD_SLOTS];
    let n = shorts.len().min(CHILD_SLOTS);
    table[..n].copy_from_slice(&shorts[..n]);
    let s = reg::irq_disable();
    CHILDREN.set(table);
    N_CHILDREN.set(n as u8);
    reg::irq_restore(s);
}

#[allow(unsafe_code)]
#[no_mangle]
#[cfg_attr(target_os = "none", link_section = ".ram_code")]
pub extern "C" fn irq_handler() {
    if read8(RF_IRQ_STATUS) & RF_IRQ_RX != 0 {
        write8(RF_IRQ_STATUS, RF_IRQ_RX);
        if read8(RF_RX_STATUS) & 0x20 == 0 {
            clear8(DMA_CHANNEL_ENABLE, DMA_CHANNEL_RF_RX);
            on_frame();
            RX_BUF.write(0, 0);
            RX_BUF.write(4, 0);
            set8(DMA_CHANNEL_ENABLE, DMA_CHANNEL_RF_RX);
        }
    }
    write8(DMA_IRQ_STATUS, 0xfc);
}

#[inline(always)]
fn on_frame() {
    let at = |i: usize| RX_BUF.read(i);
    let dma_len = at(0) as usize;
    let phr = at(4) as usize;
    if dma_len != phr + 9 || phr < 5 || phr > 127 || at(dma_len + 3) & 0x51 != 0x10 {
        return;
    }
    let now = tick();
    LAST_AIR.set(now);
    LAST_RX.set(now);
    let len = phr - 2;
    let mac = 5;
    let fcf0 = at(mac);
    let fcf1 = at(mac + 1);
    let seq = at(mac + 2);
    let frame_type = fcf0 & 7;
    if frame_type == 2 {
        if phr == 5 && ACK_WANTED.get() != 0 && seq == ACK_SEQ.get() {
            ACK_SEEN.set(1);
        }
        return;
    }
    if frame_type > 3 {
        return;
    }
    let mut acknowledge = false;
    if frame_type != 0 {
        let dst_mode = fcf1 & 0x0c;
        if len < 7 {
            return;
        }
        let pan = u16::from_le_bytes([at(mac + 3), at(mac + 4)]);
        if pan != 0xffff
            && pan != PAN.get()
            && !(pan == PROBE_PAN.get() && dst_mode == 0x08 && at(mac + 5) == 0xff && at(mac + 6) == 0xff)
        {
            return;
        }
        if dst_mode == 0x08 {
            let dst = u16::from_le_bytes([at(mac + 5), at(mac + 6)]);
            if dst != 0xffff {
                if dst != SHORT.get() {
                    return;
                }
                acknowledge = true;
            }
        } else if dst_mode == 0x0c {
            if len < 13 {
                return;
            }
            let mut i = 0;
            while i < 8 {
                if Some(at(mac + 5 + i)) != EXT.at(i) {
                    return;
                }
                i += 1;
            }
            acknowledge = true;
        } else {
            return;
        }
    }
    acknowledge &= fcf0 & 0x20 != 0;

    let stamp = u32::from_le_bytes([at(dma_len - 4), at(dma_len - 3), at(dma_len - 2), at(dma_len - 1)]);
    let end = stamp.wrapping_add((phr as u32 + 1) * 32 * reg::TICKS_PER_US);
    if (tick().wrapping_sub(end) as i32) > (ACK_LATEST_US * reg::TICKS_PER_US) as i32 {
        acknowledge = false;
    }

    let head = HEAD.get() as usize;
    let next = if head + 1 == SLOTS { 0 } else { head + 1 };
    if next == TAIL.get() as usize {
        let dropped = RADIO_DROPPED.get();
        RADIO_DROPPED.set(dropped.wrapping_add(1));
        return;
    }
    if acknowledge {
        set_state(STATE_TX);
    }
    let slot = head * SLOT_LEN;
    QUEUE.write(slot, len as u8);
    QUEUE.write(slot + 1, at(dma_len + 2));
    let mut i = 0;
    while i < 4 {
        QUEUE.write(slot + 2 + i, (end >> (8 * i)) as u8);
        i += 1;
    }
    i = 0;
    while i < len {
        QUEUE.write(slot + 6 + i, at(mac + i));
        i += 1;
    }
    HEAD.set(next as u8);

    if acknowledge {
        let mut pending = false;
        if frame_type == 3 {
            let mut src = mac + 3 + if fcf1 & 0x0c == 0x08 { 4 } else { 10 };
            if fcf0 & 0x40 == 0 {
                src += 2;
            }
            let src_len = match fcf1 & 0xc0 {
                0x80 => 2,
                0xc0 => 8,
                _ => 0,
            };
            if src_len != 0 && src + src_len < mac + len && at(src + src_len) == 0x04 {
                if src_len == 2 {
                    let poller = u16::from_le_bytes([at(src), at(src + 1)]);
                    let n = N_PENDING_SHORT.get() as usize;
                    let mut i = 0;
                    while i < n && !pending {
                        pending = PENDING_SHORT.at(i) == Some(poller);
                        i += 1;
                    }
                    if !pending {
                        let n = N_CHILDREN.get() as usize;
                        let mut known = false;
                        i = 0;
                        while i < n && !known {
                            known = CHILDREN.at(i) == Some(poller);
                            i += 1;
                        }
                        pending = !known;
                    }
                } else {
                    let n = N_PENDING_EXT.get() as usize;
                    let mut slot = 0;
                    while slot < n && !pending {
                        let mut i = 0;
                        while i < 8 && PENDING_EXT.at(slot * 8 + i) == Some(at(src + i)) {
                            i += 1;
                        }
                        pending = i == 8;
                        slot += 1;
                    }
                }
            }
        }
        ACK_BUF.write(5, if pending { 0x12 } else { 0x02 });
        ACK_BUF.write(7, seq);
        while (tick().wrapping_sub(end) as i32) < (ACK_TURNAROUND_US * reg::TICKS_PER_US) as i32 {}
        start_tx(ACK_BUF.addr());
        let t = tick();
        while read8(RF_IRQ_STATUS) & RF_IRQ_TX == 0 && tick().wrapping_sub(t) <= 1000 * reg::TICKS_PER_US {}
        write8(RF_IRQ_STATUS, RF_IRQ_TX);
        set_state(STATE_RX);
    }
}

pub fn counters() -> (u32, u32, u8, u8) {
    (CCA_BUSY.get(), RADIO_DROPPED.get(), RX_PEAK.get(), TX_SENT.get())
}

#[inline(always)]
pub fn has_frame() -> bool {
    HEAD.get() != TAIL.get()
}

pub fn receive(buf: &mut [u8; 127]) -> Option<(usize, i32, u32)> {
    let tail = TAIL.get() as usize;
    let head = HEAD.get() as usize;
    if tail == head {
        return None;
    }
    let waiting = if head >= tail { head - tail } else { head + SLOTS - tail };
    if waiting as u8 > RX_PEAK.get() {
        RX_PEAK.set(waiting as u8);
    }
    let slot = tail * SLOT_LEN;
    let len = (QUEUE.read(slot) as usize).min(SLOT_LEN - 6);
    QUEUE.copy_to(slot + 6, &mut buf[..len]);
    let rssi = QUEUE.read(slot + 1) as i32 - 110;
    let end = u32::from_le_bytes([QUEUE.read(slot + 2), QUEUE.read(slot + 3), QUEUE.read(slot + 4), QUEUE.read(slot + 5)]);
    TAIL.set(if tail + 1 == SLOTS { 0 } else { tail as u8 + 1 });
    Some((len, rssi, end))
}

fn rssi_dbm() -> i32 {
    read8(RF_RSSI) as i8 as i32 - 110
}

#[inline(always)]
fn rx_pending() -> bool {
    read8(RF_IRQ_STATUS) & RF_IRQ_RX != 0
}

fn channel_clear() -> bool {
    if receiving() {
        return false;
    }
    let t = tick();
    set_state(STATE_OFF);
    set_state(STATE_RX);
    reg::delay_us(RX_SETTLE_US);
    let mut sum = rssi_dbm();
    let mut count = 1;
    while tick().wrapping_sub(t) <= CCA_WINDOW_US * reg::TICKS_PER_US {
        sum += rssi_dbm();
        count += 1;
    }
    sum / count <= CCA_THRESHOLD_DBM
}

fn csma_transmit(random: &mut dyn FnMut() -> u32) -> bool {
    let mut be = MIN_BE;
    for _attempt in 0..MAX_CCA_ATTEMPTS {
        #[cfg(feature = "trace")]
        if _attempt > 0 {
            LAST_TX.update(|[a, b, t]| [a, b + 1, t]);
        }
        reg::delay_us((random() % (1 << be)) * BACKOFF_US);
        be = (be + 1).min(MAX_BE);
        while rx_quiet_ticks() < RX_GAP_US * reg::TICKS_PER_US {}
        let s = reg::irq_disable();
        if channel_clear() && !rx_pending() {
            ACK_SEEN.set(0);
            set_state(STATE_TX);
            reg::delay_us(TX_SETTLE_US);
            write8(RF_IRQ_STATUS, RF_IRQ_TX);
            start_tx(TX_BUF.addr());
            let t = tick();
            while read8(RF_IRQ_STATUS) & RF_IRQ_TX == 0 && tick().wrapping_sub(t) <= 10_000 * reg::TICKS_PER_US {}
            write8(RF_IRQ_STATUS, RF_IRQ_TX);
            set_state(STATE_RX);
            LAST_AIR.set(tick());
            reg::irq_restore(s);
            let sent = TX_SENT.get();
            TX_SENT.set(sent.saturating_add(1));
            return true;
        }
        reg::irq_restore(s);
        let busy = CCA_BUSY.get();
        CCA_BUSY.set(busy.wrapping_add(1));
    }
    false
}

pub fn transmit_at(mpdu: &[u8], at: u32) {
    let len = mpdu.len();
    let until = |t: u32| at.wrapping_sub(t) as i32;
    if !(3..=125).contains(&len) || until(tick()) < 0 {
        return;
    }
    TX_BUF.write32(0, len as u32 + 1);
    TX_BUF.write(4, len as u8 + 2);
    TX_BUF.copy_from(5, mpdu);
    while until(tick()) > (2 * TX_SETTLE_US * reg::TICKS_PER_US) as i32 {}
    let s = reg::irq_disable();
    set_state(STATE_TX);
    while until(tick()) > 0 {}
    write8(RF_IRQ_STATUS, RF_IRQ_TX);
    start_tx(TX_BUF.addr());
    let t = tick();
    while read8(RF_IRQ_STATUS) & RF_IRQ_TX == 0 && tick().wrapping_sub(t) <= 10_000 * reg::TICKS_PER_US {}
    write8(RF_IRQ_STATUS, RF_IRQ_TX);
    set_state(STATE_RX);
    reg::irq_restore(s);
}

pub fn transmit(mpdu: &[u8], random: &mut dyn FnMut() -> u32) -> TxStatus {
    let status = transmit_frame(mpdu, random);
    #[cfg(feature = "trace")]
    {
        let dst = if mpdu.len() >= 7 { u16::from_le_bytes([mpdu[5], mpdu[6]]) } else { 0xffff };
        crate::trace::record(zigbee::hal::Trace {
            kind: crate::trace::MAC,
            detail: status as u8,
            seq: mpdu.get(2).copied().unwrap_or(0),
            src: mpdu.len() as u16,
            peer: dst,
            aux: crate::trace::tx_aux(LAST_TX.get()),
        });
    }
    status
}

#[inline(always)]
fn transmit_frame(mpdu: &[u8], random: &mut dyn FnMut() -> u32) -> TxStatus {
    let len = mpdu.len();
    if !(3..=125).contains(&len) {
        return TxStatus::Ok;
    }
    TX_BUF.write32(0, len as u32 + 1);
    TX_BUF.write(4, len as u8 + 2);
    TX_BUF.copy_from(5, mpdu);
    let wants_ack = mpdu[0] & 0x20 != 0;
    ACK_SEQ.set(mpdu[2]);
    TX_SENT.set(0);
    #[cfg(feature = "trace")]
    LAST_TX.set([0, 0, tick()]);
    let mut busy = false;
    for _ in 0..=MAX_FRAME_RETRIES {
        ACK_WANTED.set(wants_ack as u8);
        busy = !csma_transmit(random);
        #[cfg(feature = "trace")]
        LAST_TX.update(|[a, b, t]| [a + 1, b + busy as u32, t]);
        if busy {
            continue;
        }
        if !wants_ack {
            return TxStatus::Ok;
        }
        let t = tick();
        while tick().wrapping_sub(t) <= ACK_WAIT_US * reg::TICKS_PER_US {
            if ACK_SEEN.get() != 0 {
                ACK_WANTED.set(0);
                return TxStatus::Ok;
            }
        }
    }
    ACK_WANTED.set(0);
    if busy {
        TxStatus::ChannelBusy
    } else {
        TxStatus::NoAck
    }
}

#[cfg(test)]
#[path = "tests/radio.rs"]
pub(crate) mod tests;
