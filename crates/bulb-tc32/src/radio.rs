//! IEEE 802.15.4 radio of the TLSR8258 (2.4 GHz O-QPSK, 250 kb/s), written
//! for this project.
//!
//! Telink documents the register names in its public headers
//! (`rf_drv.h`, `register.h`, `drv_radio.h`) but ships the driver only as a
//! library. The register values and the order of the writes here follow what
//! that library does on this chip (docs/rust-radio.md lists the sources);
//! `emulator/rust_radio_check.py` compares the resulting register state with
//! the vendor firmware's.
//!
//! The lower MAC is here too, because its timing lives in the radio: frame
//! filtering and the acknowledgement of received frames run in the receive
//! interrupt, and `transmit` does CSMA-CA, the transmission, the wait for
//! the acknowledgement and the retries.
//!
//! The interrupt path (`irq_handler` and what it calls) is in `.ram_code`
//! and calls nothing in the flash: flash fetches would add latency to the
//! acknowledgement turnaround.

use core::ptr::{addr_of, addr_of_mut};

use zigbee::TxStatus;

use crate::flash;
use crate::reg::{self, read16, read8, set8, clear8, tick, write16, write32, write8};

// Registers (offsets as in Telink's register.h / rf_drv.h).
const RF_MODE_CTRL: usize = 0xf02; // TX/RX state machine: 0x45 off, bit 4 TX, bit 5 RX
const RF_RX_CTRL: usize = 0x428; // bit 0: receiver enable
const RF_IRQ_MASK: usize = 0xf1c;
const RF_IRQ_STATUS: usize = 0xf20; // write one to clear
const RF_IRQ_RX: u8 = 0x01;
const RF_IRQ_TX: u8 = 0x02;
const RF_RSSI: usize = 0x441;
const RF_RX_STATUS: usize = 0x448; // bit 5: a packet is arriving
const DMA_RX_ADDR: usize = 0xc08;
const DMA_RX_SIZE: usize = 0xc0a;
const DMA_RX_MODE: usize = 0xc0b;
const DMA_TX_ADDR: usize = 0xc0c;
const DMA_CHANNEL_ENABLE: usize = 0xc20;
const DMA_CHANNEL_RF_RX: u8 = 0x04;
const DMA_CHANNEL_RF_TX: u8 = 0x08;
const DMA_IRQ_ENABLE: usize = 0xc21;
const DMA_TX_READY: usize = 0xc24;
const DMA_IRQ_STATUS: usize = 0xc26;
const DMA_TX_ADDR_HIGH: usize = 0xc43;

/// Radio setup common to all modes, then the 802.15.4 250 kb/s mode: modem
/// and synthesizer settings, 4-byte preamble, SFD 0xA7 as the access code.
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

/// Flash byte with the crystal load-capacitor trim written at the factory.
const CAP_TRIM_ADDR: usize = 0xfe000;

/// DMA receive buffer: u32 length, PHR, PSDU, timestamp, 2 bytes, RSSI, status.
const RX_BUF_LEN: usize = 144;
/// Received frames waiting for the main loop. A frame that finds the queue
/// full is not acknowledged (its sender tries again), and the main loop is
/// away for tens of milliseconds while it transmits (CSMA-CA, the wait for
/// the acknowledgement, retries): a router in the middle of a busy mesh
/// hears more than 6 frames in that time. With 6 slots, at 20 messages a
/// second in the stress house, a third of the requests went unanswered and
/// the bulbs retransmitted three times as often as stock ones; with 16
/// (2 KB of SRAM) a quarter did (12 and 24 did no better).
const SLOTS: usize = 16;
/// Queue slot: MPDU length, raw RSSI, system timer tick of the frame's end,
/// MPDU (without FCS).
const SLOT_LEN: usize = 6 + 125;

const STATE_TX: u8 = 0;
const STATE_RX: u8 = 1;
const STATE_OFF: u8 = 3;

/// Time from the end of a received frame to the start of its acknowledgement.
const ACK_TURNAROUND_US: u32 = 160;
/// Settling time after switching the synthesizer to transmit.
const TX_SETTLE_US: u32 = 120;
/// Settling time of the receiver before the RSSI means anything.
const RX_SETTLE_US: u32 = 85;
/// Clear channel assessment: averaging window and threshold. The threshold
/// is where the channel counts as busy: low enough to hear every neighbour
/// this radio can receive from, so that it does not start on top of a frame
/// for itself or for a neighbour (802.15.4-2006 6.9.9: at most 10 dB above
/// the reference sensitivity of -85 dBm). The vendor driver's -70 dBm is
/// above that: in the stress house (docs/stress-results.md) a bulb could
/// not hear neighbours 4-6.5 m away, transmitted over their frames and its
/// own receivers' transmissions, and a third of its transmissions had to be
/// repeated. The cost is that interference down to -85 dBm (Wi-Fi) also
/// holds the bulb back.
const CCA_WINDOW_US: u32 = 128;
const CCA_THRESHOLD_DBM: i32 = -85;
/// How long to wait for the acknowledgement of a transmitted frame.
const ACK_WAIT_US: u32 = 2000;
/// CSMA-CA and retry parameters, the 802.15.4 defaults (macMinBE, macMaxBE,
/// macMaxCSMABackoffs, macMaxFrameRetries).
const MIN_BE: u32 = 3;
const MAX_BE: u32 = 5;
const MAX_CCA_ATTEMPTS: u32 = 5;
const MAX_FRAME_RETRIES: u32 = 3;
const BACKOFF_US: u32 = 320;

#[repr(C, align(4))]
struct Aligned<const N: usize>([u8; N]);

static mut RX_BUF: Aligned<RX_BUF_LEN> = Aligned([0; RX_BUF_LEN]);
static mut TX_BUF: Aligned<132> = Aligned([0; 132]);
/// DMA length 4, PHR 5, frame control 0x0002, sequence number; the radio appends the FCS.
static mut ACK_BUF: Aligned<12> = Aligned([4, 0, 0, 0, 5, 0x02, 0x00, 0, 0, 0, 0, 0]);

static mut QUEUE: [[u8; SLOT_LEN]; SLOTS] = [[0; SLOT_LEN]; SLOTS];
/// Next slot the interrupt writes / the main loop reads.
static mut HEAD: u8 = 0;
static mut TAIL: u8 = 0;

/// Frequency as an offset from 2400 MHz.
static mut FREQ_OFFSET: u8 = 5;
static mut PAN: u16 = 0xffff;
static mut SHORT: u16 = 0xffff;
static mut EXT: [u8; 8] = [0; 8];

/// Acknowledgement wait: sequence number expected, and bit 0 set by the
/// interrupt when it arrives.
static mut ACK_WANTED: u8 = 0;
static mut ACK_SEQ: u8 = 0;
static mut ACK_SEEN: u8 = 0;

/// Addresses with frames waiting for their poll, as many as the stack can
/// name (`zigbee::stack::PENDING_SHORTS`, `INDIRECT`): short addresses, and
/// EUI-64s as they appear in a frame; the first `N_PENDING_*` entries are
/// in use. The acknowledgement of a Data Request from one of them carries
/// the frame-pending bit. The interrupt handler only looks at the entries in
/// use, so the time to the acknowledgement grows with them, not with the
/// size of the tables.
const PENDING_SHORT_SLOTS: usize = zigbee::stack::PENDING_SHORTS;
const PENDING_EXT_SLOTS: usize = zigbee::stack::INDIRECT;
static mut PENDING_SHORT: [u16; PENDING_SHORT_SLOTS] = [0xffff; PENDING_SHORT_SLOTS];
static mut PENDING_EXT: [[u8; 8]; PENDING_EXT_SLOTS] = [[0; 8]; PENDING_EXT_SLOTS];
static mut N_PENDING_SHORT: u8 = 0;
static mut N_PENDING_EXT: u8 = 0;

/// Short addresses of the end-device children, the first `N_CHILDREN` in use.
const CHILD_SLOTS: usize = zigbee::stack::CHILDREN;
static mut CHILDREN: [u16; CHILD_SLOTS] = [0xffff; CHILD_SLOTS];
static mut N_CHILDREN: u8 = 0;

/// The last `transmit` (`trace` builds): attempts, channel assessments
/// that found the channel busy, system timer tick at its start.
#[cfg(feature = "trace")]
pub static mut LAST_TX: [u32; 3] = [0; 3];

/// System timer tick of the last 802.15.4 traffic: the end of a frame
/// received (whoever it was for) or of one sent (`quiet_ticks`).
static mut LAST_AIR: u32 = 0;

/// Frames lost because the queue was full.
#[no_mangle]
pub static mut RADIO_DROPPED: u32 = 0;
/// The most frames that waited in the queue at once.
static mut RX_PEAK: u8 = 0;
/// Clear channel assessments that found the channel busy, since the start.
static mut CCA_BUSY: u32 = 0;
/// How often the last `transmit` put its frame on the air.
static mut TX_SENT: u8 = 0;

macro_rules! get {
    ($s:ident) => {
        // SAFETY: plain data shared with the interrupt handler; every access is volatile.
        unsafe { addr_of!($s).read_volatile() }
    };
}
macro_rules! put {
    ($s:ident, $v:expr) => {
        // SAFETY: as in `get!`.
        unsafe { addr_of_mut!($s).write_volatile($v) }
    };
}

/// Switches the radio's state machine, re-tuning the synthesizer as the
/// vendor driver does on every switch.
#[link_section = ".ram_code"]
#[inline(never)]
extern "C" fn set_state(state: u8) {
    write8(RF_MODE_CTRL, 0x45);
    let f = 2400 + get!(FREQ_OFFSET) as u16;
    let band = if f > 2549 {
        0x00
    } else if f > 2519 {
        0x04
    } else if f > 2494 {
        0x08
    } else if f > 2464 {
        0x0c
    } else if f > 2434 {
        0x10
    } else if f > 2404 {
        0x14
    } else if f > 2379 {
        0x18
    } else {
        0x1c
    };
    write8(0x1228, 0x13);
    write8(0x1244, ((f << 2) as u8) | 0x01);
    write8(0x1245, (read8(0x1245) & 0xc0) | ((f >> 6) as u8 & 0x3f));
    write8(0x1229, (read8(0x1229) & 0xc3) | band);
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

/// Hands a DMA buffer to the transmitter; the transmission starts at once.
#[inline(always)]
fn start_tx(buf: *const u8) {
    write8(DMA_TX_ADDR_HIGH, 0x04);
    write16(DMA_TX_ADDR, buf as usize as u16);
    set8(DMA_TX_READY, DMA_CHANNEL_RF_TX);
}

/// Brings the radio up in receive mode on the current channel and enables
/// its interrupt.
pub fn init() {
    for (addr, value) in INIT_TABLE {
        write8(addr as usize, value);
    }
    set8(DMA_CHANNEL_ENABLE, DMA_CHANNEL_RF_RX | DMA_CHANNEL_RF_TX);
    let mut trim = [0u8; 1];
    flash::read(CAP_TRIM_ADDR, &mut trim);
    if trim[0] != 0xff {
        reg::analog_write(0x8a, (reg::analog_read(0x8a) & 0x40) | (trim[0] & 0x3f));
    }

    set_state(STATE_OFF);
    // TX power: the highest setting of the VBAT range (about +10 dBm)
    clear8(0x1225, 0x40);
    write8(0x1226, (read8(0x1226) & 0x7f) | 0x80);
    write8(0x1227, (read8(0x1227) & 0xe0) | 0x1f);

    // SAFETY: the receiver is off; nothing else uses the buffer yet.
    let rx = unsafe { addr_of_mut!(RX_BUF.0) as *mut u8 };
    // SAFETY: indices 0 and 4 are inside the buffer.
    unsafe {
        rx.write_volatile(0);
        rx.add(4).write_volatile(0);
    }
    write16(DMA_RX_ADDR, rx as usize as u16);
    clear8(0xf03, 0x04); // no receive timeout
    write8(DMA_RX_SIZE, (RX_BUF_LEN / 16) as u8);
    write8(DMA_RX_MODE, 0x01);
    clear8(DMA_IRQ_ENABLE, DMA_CHANNEL_RF_RX | DMA_CHANNEL_RF_TX);
    // of the radio's interrupt sources only "frame received"; the end of a
    // transmission is polled
    write16(RF_IRQ_MASK, (read16(RF_IRQ_MASK) & !0x1fff) | RF_IRQ_RX as u16);
    write32(reg::IRQ_MASK, reg::read32(reg::IRQ_MASK) | reg::IRQ_MASK_RF);
    set8(DMA_CHANNEL_ENABLE, DMA_CHANNEL_RF_RX);
    set8(DMA_CHANNEL_ENABLE, DMA_CHANNEL_RF_TX);
    set8(0x430, 0x02); // timestamp of every received frame

    set_state(STATE_RX);
    write8(reg::IRQ_ENABLE, 1);
}

/// Registers the BLE side writes that this driver never does: saved when
/// the radio is lent to BLE and put back afterwards, so that the 802.15.4
/// state is what `init` left (reference/radio/tlsr8258-ble-spec.md, 1.1).
const LENT8: [u16; 9] = [0x124a, 0x124b, 0x1245, 0x40d, 0xf03, 0xf06, 0xf0c, 0xf0e, 0xf10];
const LENT16: [u16; 2] = [0xf04, 0xf0a];
const LENT32: [u16; 2] = [0x424, 0xf18];
static mut SAVED8: [u8; 9] = [0; 9];
static mut SAVED16: [u16; 2] = [0; 2];
static mut SAVED32: [u32; 2] = [0; 2];

/// Lends the radio to BLE (`ble_radio`): interrupts go off and stay off
/// until `resume`, which takes the returned state. A frame that is arriving
/// is lost; its sender retries.
pub fn suspend() -> u8 {
    let s = reg::irq_disable();
    let mut saved8 = [0u8; 9];
    for (v, addr) in saved8.iter_mut().zip(LENT8) {
        *v = read8(addr as usize);
    }
    put!(SAVED8, saved8);
    put!(SAVED16, [read16(LENT16[0] as usize), read16(LENT16[1] as usize)]);
    put!(SAVED32, [reg::read32(LENT32[0] as usize), reg::read32(LENT32[1] as usize)]);
    s
}

/// Takes the radio back from BLE: baseband reset, the 802.15.4 mode
/// settings, receive buffer, interrupt mask and TX power as `init` sets
/// them, then the receiver on the current channel.
pub fn resume(s: u8) {
    write8(0x61, 0x01);
    write8(0x61, 0x00);
    set8(0x61, 0x01);
    clear8(0x61, 0x01);
    // SAFETY: the receiver is off and interrupts are disabled.
    let rx = unsafe { addr_of_mut!(RX_BUF.0) as *mut u8 };
    // SAFETY: indices 0 and 4 are inside the buffer.
    unsafe {
        rx.write_volatile(0);
        rx.add(4).write_volatile(0);
    }
    write16(RF_IRQ_STATUS, 0xffff);
    for (addr, value) in INIT_TABLE {
        write8(addr as usize, value);
    }
    for (addr, value) in LENT8.iter().zip(get!(SAVED8)) {
        write8(*addr as usize, value);
    }
    for (addr, value) in LENT16.iter().zip(get!(SAVED16)) {
        write16(*addr as usize, value);
    }
    for (addr, value) in LENT32.iter().zip(get!(SAVED32)) {
        write32(*addr as usize, value);
    }
    set_state(STATE_OFF);
    clear8(0x1225, 0x40);
    write8(0x1226, (read8(0x1226) & 0x7f) | 0x80);
    write8(0x1227, (read8(0x1227) & 0xe0) | 0x1f);
    write16(DMA_RX_ADDR, rx as usize as u16);
    write8(DMA_RX_SIZE, (RX_BUF_LEN / 16) as u8);
    write8(DMA_RX_MODE, 0x01);
    clear8(DMA_IRQ_ENABLE, DMA_CHANNEL_RF_RX | DMA_CHANNEL_RF_TX);
    write16(RF_IRQ_MASK, (read16(RF_IRQ_MASK) & !0x1fff) | RF_IRQ_RX as u16);
    write32(reg::IRQ_MASK, reg::read32(reg::IRQ_MASK) | reg::IRQ_MASK_RF);
    set8(DMA_CHANNEL_ENABLE, DMA_CHANNEL_RF_RX | DMA_CHANNEL_RF_TX);
    set_state(STATE_RX);
    reg::irq_restore(s);
}

/// System timer ticks since the last 802.15.4 traffic (`LAST_AIR`).
pub fn quiet_ticks() -> u32 {
    tick().wrapping_sub(get!(LAST_AIR))
}

/// The noise floor of the channel the receiver listens on: the signal
/// strength averaged as the channel assessment does it (receiver restarted,
/// let settle, 128 µs), which is how the RSSI register gives a value at all
/// (left listening, it rests at its lowest). The same step goes before every
/// transmission; here once a minute. `None` while a frame arrives or within
/// 3 ms of the last one.
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

/// Whether a frame is arriving right now: the receiver has found its start,
/// or there is a signal on the channel strong enough to be one (what the
/// channel assessment takes for busy). The second covers the part of a
/// frame before the receiver has synchronised, and a radio model that
/// shows only the signal strength: BLE, which waits for this before it
/// takes the radio, took it from frames that were already arriving, and a
/// broadcast sent once was lost (the spec suite at --cpi 3, "receiver off").
pub fn receiving() -> bool {
    read8(RF_RX_STATUS) & 0x20 != 0 || rssi_dbm() > CCA_THRESHOLD_DBM
}

/// Tunes to an 802.15.4 channel (11..=26) and listens there.
pub fn set_channel(channel: u8) {
    if !(11..=26).contains(&channel) {
        return;
    }
    let s = reg::irq_disable();
    put!(FREQ_OFFSET, (channel - 10) * 5);
    set_state(STATE_RX);
    reg::irq_restore(s);
}

/// Energy detection: tunes to the channel and returns the strongest RSSI
/// seen within `ms` milliseconds, in dBm.
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

/// `n`, at most `max` (inline: the interrupt path calls nothing in flash).
#[inline(always)]
fn bounded(n: u8, max: usize) -> usize {
    if n as usize > max {
        max
    } else {
        n as usize
    }
}

/// Sets the addresses the receive filter accepts and acknowledges.
pub fn set_filter(pan: u16, short: u16, ext: u64) {
    let s = reg::irq_disable();
    put!(PAN, pan);
    put!(SHORT, short);
    put!(EXT, ext.to_le_bytes());
    reg::irq_restore(s);
}

/// Sets the addresses whose Data Requests are acknowledged with the
/// frame-pending bit.
pub fn set_pending(shorts: &[u16], exts: &[u64]) {
    let mut short_table = [0xffffu16; PENDING_SHORT_SLOTS];
    let mut ext_table = [[0u8; 8]; PENDING_EXT_SLOTS];
    let ns = shorts.len().min(PENDING_SHORT_SLOTS);
    let ne = exts.len().min(PENDING_EXT_SLOTS);
    short_table[..ns].copy_from_slice(&shorts[..ns]);
    for (slot, e) in ext_table.iter_mut().zip(&exts[..ne]) {
        *slot = e.to_le_bytes();
    }
    let s = reg::irq_disable();
    // SAFETY: interrupts are off, so the handler does not read the tables meanwhile.
    unsafe {
        addr_of_mut!(PENDING_SHORT).write_volatile(short_table);
        addr_of_mut!(PENDING_EXT).write_volatile(ext_table);
    }
    put!(N_PENDING_SHORT, ns as u8);
    put!(N_PENDING_EXT, ne as u8);
    reg::irq_restore(s);
}

/// Sets the short addresses of the end-device children: a Data Request
/// from any other short address is acknowledged with the frame-pending bit.
pub fn set_children(shorts: &[u16]) {
    let mut table = [0xffffu16; CHILD_SLOTS];
    let n = shorts.len().min(CHILD_SLOTS);
    table[..n].copy_from_slice(&shorts[..n]);
    let s = reg::irq_disable();
    // SAFETY: interrupts are off, so the handler does not read the table meanwhile.
    unsafe { addr_of_mut!(CHILDREN).write_volatile(table) };
    put!(N_CHILDREN, n as u8);
    reg::irq_restore(s);
}

/// The interrupt entry, called from `__irq` in start.S. Only the radio's
/// "frame received" interrupt is enabled.
#[no_mangle]
#[link_section = ".ram_code"]
pub extern "C" fn irq_handler() {
    if read8(RF_IRQ_STATUS) & RF_IRQ_RX != 0 {
        write8(RF_IRQ_STATUS, RF_IRQ_RX);
        // SAFETY: the DMA wrote the buffer; the handler owns it until it re-enables the channel.
        let p = unsafe { addr_of_mut!(RX_BUF.0) as *mut u8 };
        // another frame is already coming in: its interrupt will follow
        if read8(RF_RX_STATUS) & 0x20 == 0 {
            clear8(DMA_CHANNEL_ENABLE, DMA_CHANNEL_RF_RX);
            // SAFETY: `p` points to RX_BUF_LEN bytes.
            unsafe {
                on_frame(p);
                p.write_volatile(0);
                p.add(4).write_volatile(0);
            }
            set8(DMA_CHANNEL_ENABLE, DMA_CHANNEL_RF_RX);
        }
    }
    write8(DMA_IRQ_STATUS, 0xfc);
}

/// Filters one received frame, queues it and acknowledges it.
///
/// # Safety
/// `p` must point to the DMA receive buffer.
#[inline(always)]
unsafe fn on_frame(p: *mut u8) {
    let at = |i: usize| -> u8 {
        // SAFETY: every index below is checked against the buffer length first.
        unsafe { p.add(i).read_volatile() }
    };
    let dma_len = at(0) as usize;
    let phr = at(4) as usize;
    // length consistent, CRC good
    if dma_len != phr + 9 || phr < 5 || phr > 127 || at(dma_len + 3) & 0x51 != 0x10 {
        return;
    }
    put!(LAST_AIR, tick());
    let len = phr - 2;
    let mac = 5;
    let fcf0 = at(mac);
    let fcf1 = at(mac + 1);
    let seq = at(mac + 2);
    let frame_type = fcf0 & 7;
    if frame_type == 2 {
        // an acknowledgement: only of interest while `transmit` waits for it
        if phr == 5 && get!(ACK_WANTED) != 0 && seq == get!(ACK_SEQ) {
            put!(ACK_SEEN, 1);
        }
        return;
    }
    if frame_type > 3 {
        return;
    }
    let mut acknowledge = false;
    if frame_type != 0 {
        // data and command frames must be addressed to us or to everyone
        let dst_mode = fcf1 & 0x0c;
        if dst_mode == 0 || len < 7 {
            return;
        }
        let pan = at(mac + 3) as u16 | (at(mac + 4) as u16) << 8;
        if pan != 0xffff && pan != get!(PAN) {
            return;
        }
        if dst_mode == 0x08 {
            let dst = at(mac + 5) as u16 | (at(mac + 6) as u16) << 8;
            if dst != 0xffff {
                if dst != get!(SHORT) {
                    return;
                }
                acknowledge = true;
            }
        } else if dst_mode == 0x0c {
            if len < 13 {
                return;
            }
            let ext = addr_of!(EXT) as *const u8;
            let mut i = 0;
            while i < 8 {
                // SAFETY: `ext` points to 8 bytes.
                if at(mac + 5 + i) != unsafe { ext.add(i).read_volatile() } {
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

    // the timestamp marks the end of the SFD; the frame ends PHR + 1 bytes later
    let stamp = at(dma_len - 4) as u32
        | (at(dma_len - 3) as u32) << 8
        | (at(dma_len - 2) as u32) << 16
        | (at(dma_len - 1) as u32) << 24;
    let end = stamp.wrapping_add((phr as u32 + 1) * 32 * reg::TICKS_PER_US);

    let head = get!(HEAD) as usize;
    let next = if head + 1 == SLOTS { 0 } else { head + 1 };
    if next == get!(TAIL) as usize {
        // nowhere to put it: drop it unacknowledged, the sender will retry
        let dropped = get!(RADIO_DROPPED);
        put!(RADIO_DROPPED, dropped.wrapping_add(1));
        return;
    }
    if acknowledge {
        // start the synthesizer now so that it has settled when the wait is over
        set_state(STATE_TX);
    }
    let slot = addr_of_mut!(QUEUE) as *mut u8;
    // SAFETY: `head < SLOTS` and `len <= 125`, so everything stays inside the slot.
    unsafe {
        let slot = slot.add(head * SLOT_LEN);
        slot.write_volatile(len as u8);
        slot.add(1).write_volatile(at(dma_len + 2));
        let mut i = 0;
        while i < 4 {
            slot.add(2 + i).write_volatile((end >> (8 * i)) as u8);
            i += 1;
        }
        i = 0;
        while i < len {
            slot.add(6 + i).write_volatile(at(mac + i));
            i += 1;
        }
    }
    put!(HEAD, next as u8);

    if acknowledge {
        // A Data Request from a child with frames waiting is acknowledged
        // with the frame-pending bit, so that the child stays awake for them.
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
                    let poller = at(src) as u16 | (at(src + 1) as u16) << 8;
                    let table = addr_of!(PENDING_SHORT) as *const u16;
                    let n = bounded(get!(N_PENDING_SHORT), PENDING_SHORT_SLOTS);
                    let mut i = 0;
                    while i < n && !pending {
                        // SAFETY: `i` is below PENDING_SHORT_SLOTS.
                        pending = unsafe { table.add(i).read_volatile() } == poller;
                        i += 1;
                    }
                    if !pending {
                        // A poll under a short address that is no child's: the
                        // stack answers it with a Leave ("rejoin"), which only
                        // a device that stays awake will hear.
                        let children = addr_of!(CHILDREN) as *const u16;
                        let n = bounded(get!(N_CHILDREN), CHILD_SLOTS);
                        let mut known = false;
                        i = 0;
                        while i < n && !known {
                            // SAFETY: `i` is below CHILD_SLOTS.
                            known = unsafe { children.add(i).read_volatile() } == poller;
                            i += 1;
                        }
                        pending = !known;
                    }
                } else {
                    let table = addr_of!(PENDING_EXT) as *const u8;
                    let n = bounded(get!(N_PENDING_EXT), PENDING_EXT_SLOTS);
                    let mut slot = 0;
                    while slot < n && !pending {
                        let mut i = 0;
                        // SAFETY: `slot * 8 + i` stays inside the table for i < 8.
                        while i < 8 && unsafe { table.add(slot * 8 + i).read_volatile() } == at(src + i) {
                            i += 1;
                        }
                        pending = i == 8;
                        slot += 1;
                    }
                }
            }
        }
        let ack = addr_of_mut!(ACK_BUF.0) as *mut u8;
        // SAFETY: indices 5 and 7 are inside the 12-byte buffer.
        unsafe {
            ack.add(5).write_volatile(if pending { 0x12 } else { 0x02 });
            ack.add(7).write_volatile(seq);
        }
        while (tick().wrapping_sub(end) as i32) < (ACK_TURNAROUND_US * reg::TICKS_PER_US) as i32 {}
        start_tx(ack);
        let t = tick();
        while read8(RF_IRQ_STATUS) & RF_IRQ_TX == 0 && tick().wrapping_sub(t) <= 1000 * reg::TICKS_PER_US {}
        write8(RF_IRQ_STATUS, RF_IRQ_TX);
        set_state(STATE_RX);
    }
}

/// What the driver counted since the start, for the Diagnostics cluster:
/// busy channel assessments, frames lost to a full queue, the queue's
/// high-water mark, and how often the last `transmit` sent its frame.
pub fn counters() -> (u32, u32, u8, u8) {
    (get!(CCA_BUSY), get!(RADIO_DROPPED), get!(RX_PEAK), get!(TX_SENT))
}

/// Takes the next received MPDU; returns its length, the RSSI in dBm and
/// the system timer tick at which the frame ended.
pub fn receive(buf: &mut [u8; 127]) -> Option<(usize, i32, u32)> {
    let tail = get!(TAIL) as usize;
    let head = get!(HEAD) as usize;
    if tail == head {
        return None;
    }
    // how many the main loop finds waiting (the high-water mark is taken
    // here, not in the interrupt, which is on its way to an acknowledgement)
    let waiting = if head >= tail { head - tail } else { head + SLOTS - tail };
    if waiting as u8 > get!(RX_PEAK) {
        put!(RX_PEAK, waiting as u8);
    }
    // SAFETY: the interrupt never touches the slot at TAIL (it stops before it).
    let slot = unsafe { &*(addr_of!(QUEUE) as *const [u8; SLOT_LEN]).add(tail) };
    let len = (slot[0] as usize).min(SLOT_LEN - 6);
    buf[..len].copy_from_slice(&slot[6..6 + len]);
    let rssi = slot[1] as i32 - 110;
    let end = u32::from_le_bytes([slot[2], slot[3], slot[4], slot[5]]);
    put!(TAIL, if tail + 1 == SLOTS { 0 } else { tail as u8 + 1 });
    Some((len, rssi, end))
}

fn rssi_dbm() -> i32 {
    read8(RF_RSSI) as i8 as i32 - 110
}

/// Clear channel assessment, with interrupts off: restarts the receiver
/// (unless a frame is coming in), lets it settle and averages the RSSI.
fn channel_clear() -> bool {
    let t = tick();
    if read8(RF_RX_STATUS) & 0x20 == 0 {
        set_state(STATE_OFF);
        set_state(STATE_RX);
        reg::delay_us(RX_SETTLE_US);
    }
    let mut sum = rssi_dbm();
    let mut count = 1;
    while tick().wrapping_sub(t) <= CCA_WINDOW_US * reg::TICKS_PER_US {
        sum += rssi_dbm();
        count += 1;
    }
    sum / count <= CCA_THRESHOLD_DBM
}

/// One CSMA-CA round: backs off, and when the channel is clear transmits
/// the frame in `TX_BUF`. False when the channel stayed busy.
fn csma_transmit(random: &mut impl FnMut() -> u32) -> bool {
    let mut be = MIN_BE;
    for _attempt in 0..MAX_CCA_ATTEMPTS {
        // SAFETY: main loop only.
        #[cfg(feature = "trace")]
        unsafe {
            if _attempt > 0 {
                (*addr_of_mut!(LAST_TX))[1] += 1;
            }
        }
        // Unslotted CSMA-CA: a random number of backoff periods before every
        // assessment, the first included. (The vendor stack skips the first
        // one; with it, two nodes answering the same frame do not collide.)
        reg::delay_us((random() % (1 << be)) * BACKOFF_US);
        be = (be + 1).min(MAX_BE);
        let s = reg::irq_disable();
        if channel_clear() {
            put!(ACK_SEEN, 0);
            set_state(STATE_TX);
            reg::delay_us(TX_SETTLE_US);
            write8(RF_IRQ_STATUS, RF_IRQ_TX);
            write8(RF_IRQ_STATUS, RF_IRQ_RX);
            // SAFETY: the buffer was filled by `transmit` and is not touched during the transmission.
            start_tx(unsafe { addr_of!(TX_BUF.0) as *const u8 });
            let t = tick();
            while read8(RF_IRQ_STATUS) & RF_IRQ_TX == 0 && tick().wrapping_sub(t) <= 10_000 * reg::TICKS_PER_US {}
            write8(RF_IRQ_STATUS, RF_IRQ_TX);
            set_state(STATE_RX);
            put!(LAST_AIR, tick());
            reg::irq_restore(s);
            let sent = get!(TX_SENT);
            put!(TX_SENT, sent.saturating_add(1));
            return true;
        }
        reg::irq_restore(s);
        let busy = get!(CCA_BUSY);
        put!(CCA_BUSY, busy.wrapping_add(1));
    }
    false
}

/// Transmits an MPDU at system timer tick `at`, without channel assessment
/// or acknowledgement: the answer to a Green Power device, which listens
/// for a moment at a fixed time after its own frame. Does nothing when the
/// moment has passed.
pub fn transmit_at(mpdu: &[u8], at: u32) {
    let len = mpdu.len();
    let until = |t: u32| at.wrapping_sub(t) as i32;
    if !(3..=125).contains(&len) || until(tick()) < 0 {
        return;
    }
    // SAFETY: as in `transmit`.
    let tx = unsafe { &mut *addr_of_mut!(TX_BUF.0) };
    tx[0] = len as u8 + 1;
    tx[1] = 0;
    tx[2] = 0;
    tx[3] = 0;
    tx[4] = len as u8 + 2;
    tx[5..5 + len].copy_from_slice(mpdu);
    // keep receiving until shortly before; then tune the transmitter and hit the moment
    while until(tick()) > (2 * TX_SETTLE_US * reg::TICKS_PER_US) as i32 {}
    let s = reg::irq_disable();
    set_state(STATE_TX);
    while until(tick()) > 0 {}
    write8(RF_IRQ_STATUS, RF_IRQ_TX);
    write8(RF_IRQ_STATUS, RF_IRQ_RX);
    // SAFETY: the buffer is not touched during the transmission.
    start_tx(unsafe { addr_of!(TX_BUF.0) as *const u8 });
    let t = tick();
    while read8(RF_IRQ_STATUS) & RF_IRQ_TX == 0 && tick().wrapping_sub(t) <= 10_000 * reg::TICKS_PER_US {}
    write8(RF_IRQ_STATUS, RF_IRQ_TX);
    set_state(STATE_RX);
    reg::irq_restore(s);
}

/// Transmits an MPDU (the radio appends the FCS): CSMA-CA, and for frames
/// that request an acknowledgement, the wait for it and up to three retries.
pub fn transmit(mpdu: &[u8], random: impl FnMut() -> u32) -> TxStatus {
    let status = transmit_frame(mpdu, random);
    #[cfg(feature = "trace")]
    {
        // SAFETY: main loop only.
        let [attempts, busy, t0] = unsafe { *addr_of!(LAST_TX) };
        let us = tick().wrapping_sub(t0) / reg::TICKS_PER_US;
        let dst = if mpdu.len() >= 7 { u16::from_le_bytes([mpdu[5], mpdu[6]]) } else { 0xffff };
        crate::trace::record(zigbee::hal::Trace {
            kind: crate::trace::MAC,
            detail: status as u8,
            seq: mpdu.get(2).copied().unwrap_or(0),
            src: mpdu.len() as u16,
            peer: dst,
            aux: attempts.min(255) | busy.min(255) << 8 | us.min(0xffff) << 16,
        });
    }
    status
}

#[inline(always)]
fn transmit_frame(mpdu: &[u8], mut random: impl FnMut() -> u32) -> TxStatus {
    let len = mpdu.len();
    if !(3..=125).contains(&len) {
        return TxStatus::Ok;
    }
    // SAFETY: only `transmit` writes the buffer, and it does not run concurrently with itself.
    let tx = unsafe { &mut *addr_of_mut!(TX_BUF.0) };
    tx[0] = len as u8 + 1;
    tx[1] = 0;
    tx[2] = 0;
    tx[3] = 0;
    tx[4] = len as u8 + 2;
    tx[5..5 + len].copy_from_slice(mpdu);
    let wants_ack = mpdu[0] & 0x20 != 0;
    put!(ACK_SEQ, mpdu[2]);
    put!(TX_SENT, 0);
    // SAFETY: main loop only.
    #[cfg(feature = "trace")]
    unsafe {
        *addr_of_mut!(LAST_TX) = [0, 0, tick()];
    }
    let mut busy = false;
    for _ in 0..=MAX_FRAME_RETRIES {
        put!(ACK_WANTED, wants_ack as u8);
        busy = !csma_transmit(&mut random);
        // SAFETY: main loop only.
        #[cfg(feature = "trace")]
        unsafe {
            (*addr_of_mut!(LAST_TX))[0] += 1;
            if busy {
                (*addr_of_mut!(LAST_TX))[1] += 1;
            }
        }
        if busy {
            continue;
        }
        if !wants_ack {
            return TxStatus::Ok;
        }
        let t = tick();
        while tick().wrapping_sub(t) <= ACK_WAIT_US * reg::TICKS_PER_US {
            if get!(ACK_SEEN) != 0 {
                put!(ACK_WANTED, 0);
                return TxStatus::Ok;
            }
        }
    }
    put!(ACK_WANTED, 0);
    if busy {
        TxStatus::ChannelBusy
    } else {
        TxStatus::NoAck
    }
}
