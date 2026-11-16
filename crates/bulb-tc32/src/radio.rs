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

use zigbee::TxStatus;

use crate::cell::{SharedBuf, Shared};
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
/// The latest an acknowledgement may still start after the end of its
/// frame. Its sender waits macAckWaitDuration, 54 symbols (864 us), from the
/// end of its frame for it to arrive (802.15.4-2006 7.4.2, 7.5.6.4.2); an
/// acknowledgement 160 us long (SHR, PHR and 5 bytes) that starts later than
/// this arrives after the sender has given up and started its retry. The
/// interrupt handler is that late only for a frame that came in while
/// interrupts were off (`transmit_at`, a flash write): it takes the frame,
/// and the sender's retry gets the acknowledgement instead.
const ACK_LATEST_US: u32 = 600;
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
/// How long after the end of a received frame a channel assessment waits
/// for the frame its sender may follow it with (`csma_transmit`): the
/// acknowledgement (12 symbols of turnaround, 11 bytes) and a turnaround
/// and assessment of the sender's after it.
const RX_GAP_US: u32 = 1200;

static RX_BUF: SharedBuf<RX_BUF_LEN> = SharedBuf::new([0; RX_BUF_LEN]);
static TX_BUF: SharedBuf<132> = SharedBuf::new([0; 132]);
/// DMA length 4, PHR 5, frame control 0x0002, sequence number; the radio appends the FCS.
static ACK_BUF: SharedBuf<12> = SharedBuf::new([4, 0, 0, 0, 5, 0x02, 0x00, 0, 0, 0, 0, 0]);

static QUEUE: SharedBuf<{ SLOTS * SLOT_LEN }> = SharedBuf::new([0; SLOTS * SLOT_LEN]);
/// Next slot the interrupt writes / the main loop reads.
static HEAD: Shared<u8> = Shared::new(0);
static TAIL: Shared<u8> = Shared::new(0);

/// Frequency as an offset from 2400 MHz.
static FREQ_OFFSET: Shared<u8> = Shared::new(5);
static PAN: Shared<u16> = Shared::new(0xffff);
/// Motion sensing: broadcasts to this PAN (the probes) pass the filter too.
#[cfg(feature = "sense")]
static PROBE_PAN: Shared<u16> = Shared::new(0xffff);
static SHORT: Shared<u16> = Shared::new(0xffff);
static EXT: Shared<[u8; 8]> = Shared::new([0; 8]);

/// Acknowledgement wait: sequence number expected, and bit 0 set by the
/// interrupt when it arrives.
static ACK_WANTED: Shared<u8> = Shared::new(0);
static ACK_SEQ: Shared<u8> = Shared::new(0);
static ACK_SEEN: Shared<u8> = Shared::new(0);

/// Addresses with frames waiting for their poll, as many as the stack can
/// name (`zigbee::stack::PENDING_SHORTS`, `INDIRECT`): short addresses, and
/// EUI-64s as they appear in a frame; the first `N_PENDING_*` entries are
/// in use. The acknowledgement of a Data Request from one of them carries
/// the frame-pending bit. The interrupt handler only looks at the entries in
/// use, so the time to the acknowledgement grows with them, not with the
/// size of the tables.
const PENDING_SHORT_SLOTS: usize = zigbee::stack::PENDING_SHORTS;
const PENDING_EXT_SLOTS: usize = zigbee::stack::INDIRECT;
static PENDING_SHORT: Shared<[u16; PENDING_SHORT_SLOTS]> = Shared::new([0xffff; PENDING_SHORT_SLOTS]);
static PENDING_EXT: Shared<[u8; 8 * PENDING_EXT_SLOTS]> = Shared::new([0; 8 * PENDING_EXT_SLOTS]);
static N_PENDING_SHORT: Shared<u8> = Shared::new(0);
static N_PENDING_EXT: Shared<u8> = Shared::new(0);

/// Short addresses of the end-device children, the first `N_CHILDREN` in use.
const CHILD_SLOTS: usize = zigbee::stack::CHILDREN;
static CHILDREN: Shared<[u16; CHILD_SLOTS]> = Shared::new([0xffff; CHILD_SLOTS]);
static N_CHILDREN: Shared<u8> = Shared::new(0);

/// The last `transmit` (`trace` builds): attempts, channel assessments
/// that found the channel busy, system timer tick at its start.
#[cfg(feature = "trace")]
pub static LAST_TX: Shared<[u32; 3]> = Shared::new([0; 3]);

/// System timer tick of the last 802.15.4 traffic: the end of a frame
/// received (whoever it was for) or of one sent (`quiet_ticks`).
static LAST_AIR: Shared<u32> = Shared::new(0);
/// System timer tick of the end of the last frame received (`rx_quiet_ticks`).
static LAST_RX: Shared<u32> = Shared::new(0);

/// Frames lost because the queue was full.
#[allow(unsafe_code)] // a linker attribute
#[no_mangle]
pub static RADIO_DROPPED: Shared<u32> = Shared::new(0);
/// The most frames that waited in the queue at once.
static RX_PEAK: Shared<u8> = Shared::new(0);
/// Clear channel assessments that found the channel busy, since the start.
static CCA_BUSY: Shared<u32> = Shared::new(0);
/// How often the last `transmit` put its frame on the air.
static TX_SENT: Shared<u8> = Shared::new(0);


/// Switches the radio's state machine, re-tuning the synthesizer as the
/// vendor driver does on every switch.
#[allow(unsafe_code)] // a linker attribute
#[link_section = ".ram_code"]
#[inline(never)]
extern "C" fn set_state(state: u8) {
    write8(RF_MODE_CTRL, 0x45);
    let f = 2400 + FREQ_OFFSET.get() as u16;
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
fn start_tx(buf: usize) {
    write8(DMA_TX_ADDR_HIGH, 0x04);
    write16(DMA_TX_ADDR, buf as u16);
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

    // the receiver is off; nothing else uses the buffer yet
    RX_BUF.write(0, 0);
    RX_BUF.write(4, 0);
    write16(DMA_RX_ADDR, RX_BUF.addr() as u16);
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
static SAVED8: Shared<[u8; 9]> = Shared::new([0; 9]);
static SAVED16: Shared<[u16; 2]> = Shared::new([0; 2]);
static SAVED32: Shared<[u32; 2]> = Shared::new([0; 2]);

/// Lends the radio to BLE (`ble_radio`): interrupts go off and stay off
/// until `resume`, which takes the returned state. A frame that is arriving
/// is lost; its sender retries. One that has arrived is taken first: BLE
/// clears the radio's status when it sets the radio up, and a frame whose
/// interrupt had not run by then would be lost without a trace. (The
/// receiver goes off first, so that none can end after the look; the
/// handler turns it on again when it acknowledges a frame.)
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

/// Takes the radio back from BLE: baseband reset, the 802.15.4 mode
/// settings, receive buffer, interrupt mask and TX power as `init` sets
/// them, then the receiver on the current channel.
pub fn resume(s: u8) {
    write8(0x61, 0x01);
    write8(0x61, 0x00);
    set8(0x61, 0x01);
    clear8(0x61, 0x01);
    // the receiver is off and interrupts are disabled
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
    write8(0x1226, (read8(0x1226) & 0x7f) | 0x80);
    write8(0x1227, (read8(0x1227) & 0xe0) | 0x1f);
    write16(DMA_RX_ADDR, RX_BUF.addr() as u16);
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
    tick().wrapping_sub(LAST_AIR.get())
}

/// System timer ticks since the end of the last frame received (`LAST_RX`):
/// how long the neighbours have been silent.
pub fn rx_quiet_ticks() -> u32 {
    tick().wrapping_sub(LAST_RX.get())
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
    FREQ_OFFSET.set((channel - 10) * 5);
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
    PAN.set(pan);
    SHORT.set(short);
    EXT.set(ext.to_le_bytes());
    reg::irq_restore(s);
}

/// Motion sensing: the filter also takes broadcasts to `pan` (0xFFFF: none),
/// and acknowledges none of them.
#[cfg(feature = "sense")]
pub fn set_probe_pan(pan: u16) {
    PROBE_PAN.set(pan);
}

/// Sets the addresses whose Data Requests are acknowledged with the
/// frame-pending bit.
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
    // interrupts are off, so the handler does not read the tables halfway written
    PENDING_SHORT.set(short_table);
    PENDING_EXT.set(ext_table);
    N_PENDING_SHORT.set(ns as u8);
    N_PENDING_EXT.set(ne as u8);
    reg::irq_restore(s);
}

/// Sets the short addresses of the end-device children: a Data Request
/// from any other short address is acknowledged with the frame-pending bit.
pub fn set_children(shorts: &[u16]) {
    let mut table = [0xffffu16; CHILD_SLOTS];
    let n = shorts.len().min(CHILD_SLOTS);
    table[..n].copy_from_slice(&shorts[..n]);
    let s = reg::irq_disable();
    // interrupts are off, so the handler does not read the table halfway written
    CHILDREN.set(table);
    N_CHILDREN.set(n as u8);
    reg::irq_restore(s);
}

/// The interrupt entry, called from `__irq` in start.S. Only the radio's
/// "frame received" interrupt is enabled.
#[allow(unsafe_code)] // a linker attribute
#[no_mangle]
#[link_section = ".ram_code"]
pub extern "C" fn irq_handler() {
    if read8(RF_IRQ_STATUS) & RF_IRQ_RX != 0 {
        write8(RF_IRQ_STATUS, RF_IRQ_RX);
        // another frame is already coming in: its interrupt will follow
        if read8(RF_RX_STATUS) & 0x20 == 0 {
            // the DMA wrote the buffer; the handler has it until it re-enables the channel
            clear8(DMA_CHANNEL_ENABLE, DMA_CHANNEL_RF_RX);
            on_frame();
            RX_BUF.write(0, 0);
            RX_BUF.write(4, 0);
            set8(DMA_CHANNEL_ENABLE, DMA_CHANNEL_RF_RX);
        }
    }
    write8(DMA_IRQ_STATUS, 0xfc);
}

/// Filters the frame in the receive buffer, queues it and acknowledges it.
#[inline(always)]
fn on_frame() {
    let at = |i: usize| RX_BUF.read(i);
    let dma_len = at(0) as usize;
    let phr = at(4) as usize;
    // length consistent, CRC good
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
        // an acknowledgement: only of interest while `transmit` waits for it
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
        // data and command frames must be addressed to us or to everyone
        let dst_mode = fcf1 & 0x0c;
        if dst_mode == 0 || len < 7 {
            return;
        }
        let pan = at(mac + 3) as u16 | (at(mac + 4) as u16) << 8;
        #[cfg(not(feature = "sense"))]
        if pan != 0xffff && pan != PAN.get() {
            return;
        }
        // a probe of the motion sensing: a broadcast to the probes' PAN
        #[cfg(feature = "sense")]
        if pan != 0xffff
            && pan != PAN.get()
            && !(pan == PROBE_PAN.get() && dst_mode == 0x08 && at(mac + 5) == 0xff && at(mac + 6) == 0xff)
        {
            return;
        }
        if dst_mode == 0x08 {
            let dst = at(mac + 5) as u16 | (at(mac + 6) as u16) << 8;
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

    // the timestamp marks the end of the SFD; the frame ends PHR + 1 bytes later
    let stamp = at(dma_len - 4) as u32
        | (at(dma_len - 3) as u32) << 8
        | (at(dma_len - 2) as u32) << 16
        | (at(dma_len - 1) as u32) << 24;
    let end = stamp.wrapping_add((phr as u32 + 1) * 32 * reg::TICKS_PER_US);
    // too late for its sender to take the acknowledgement: none is sent
    if (tick().wrapping_sub(end) as i32) > (ACK_LATEST_US * reg::TICKS_PER_US) as i32 {
        acknowledge = false;
    }

    let head = HEAD.get() as usize;
    let next = if head + 1 == SLOTS { 0 } else { head + 1 };
    if next == TAIL.get() as usize {
        // nowhere to put it: drop it unacknowledged, the sender will retry
        let dropped = RADIO_DROPPED.get();
        RADIO_DROPPED.set(dropped.wrapping_add(1));
        return;
    }
    if acknowledge {
        // start the synthesizer now so that it has settled when the wait is over
        set_state(STATE_TX);
    }
    // `head < SLOTS` and `len <= 125`: everything stays inside the slot
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
                    let n = bounded(N_PENDING_SHORT.get(), PENDING_SHORT_SLOTS);
                    let mut i = 0;
                    while i < n && !pending {
                        pending = PENDING_SHORT.at(i) == Some(poller);
                        i += 1;
                    }
                    if !pending {
                        // A poll under a short address that is no child's: the
                        // stack answers it with a Leave ("rejoin"), which only
                        // a device that stays awake will hear.
                        let n = bounded(N_CHILDREN.get(), CHILD_SLOTS);
                        let mut known = false;
                        i = 0;
                        while i < n && !known {
                            known = CHILDREN.at(i) == Some(poller);
                            i += 1;
                        }
                        pending = !known;
                    }
                } else {
                    let n = bounded(N_PENDING_EXT.get(), PENDING_EXT_SLOTS);
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

/// What the driver counted since the start, for the Diagnostics cluster:
/// busy channel assessments, frames lost to a full queue, the queue's
/// high-water mark, and how often the last `transmit` sent its frame.
pub fn counters() -> (u32, u32, u8, u8) {
    (CCA_BUSY.get(), RADIO_DROPPED.get(), RX_PEAK.get(), TX_SENT.get())
}

/// Takes the next received MPDU; returns its length, the RSSI in dBm and
/// the system timer tick at which the frame ended.
/// Whether a received frame waits in the queue (`receive`).
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
    // how many the main loop finds waiting (the high-water mark is taken
    // here, not in the interrupt, which is on its way to an acknowledgement)
    let waiting = if head >= tail { head - tail } else { head + SLOTS - tail };
    if waiting as u8 > RX_PEAK.get() {
        RX_PEAK.set(waiting as u8);
    }
    // the interrupt never writes the slot at TAIL (it stops before it)
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

/// A received frame is in the DMA buffer, waiting for the interrupt handler
/// (interrupts are off).
#[inline(always)]
fn rx_pending() -> bool {
    read8(RF_IRQ_STATUS) & RF_IRQ_RX != 0
}

/// Clear channel assessment, with interrupts off: restarts the receiver,
/// lets it settle and averages the RSSI. A frame that is coming in (the
/// receiver has found its start, or its signal shows: `receiving`) makes
/// the channel busy at once, and the receiver is not restarted under it: a
/// restart drops the frame. (The emulator shows the signal but not the
/// start bit; a neighbour's frame for us that ended while the receiver was
/// being restarted for an assessment was lost there, in `mixed_chain`.)
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

/// One CSMA-CA round: backs off, and when the channel is clear transmits
/// the frame in `TX_BUF`. False when the channel stayed busy.
fn csma_transmit(random: &mut impl FnMut() -> u32) -> bool {
    let mut be = MIN_BE;
    for _attempt in 0..MAX_CCA_ATTEMPTS {
        #[cfg(feature = "trace")]
        if _attempt > 0 {
            LAST_TX.update(|[a, b, t]| [a, b + 1, t]);
        }
        // Unslotted CSMA-CA: a random number of backoff periods before every
        // assessment, the first included. (The vendor stack skips the first
        // one; with it, two nodes answering the same frame do not collide.)
        reg::delay_us((random() % (1 << be)) * BACKOFF_US);
        be = (be + 1).min(MAX_BE);
        // A neighbour that has just sent a frame often sends the next one
        // right after it: a stock router's Device_annce follows its Link
        // Status 0.4 ms after its end (the vendor's CSMA-CA skips the first
        // backoff), and the frame after one that was acknowledged comes
        // that long after the acknowledgement. An assessment in that gap
        // finds the channel clear and the transmission takes the radio from
        // the frame that follows: the assessment waits until the gap is
        // over, and then hears it.
        while rx_quiet_ticks() < RX_GAP_US * reg::TICKS_PER_US {}
        let s = reg::irq_disable();
        // A frame that ended while interrupts were off for the assessment
        // (in the receiver's settling time, before the averaging starts) is
        // waiting in the receive buffer: the channel was busy after all, and
        // the frame is taken first, when interrupts come back on. It used to
        // be thrown away here with its status bit, unacknowledged. In the
        // emulator's `mixed_chain` that was a stock bulb's answer to a frame
        // the Rust bulb had just relayed to it: it ended during the
        // assessment for the relay of the next one, the stock bulb's retry
        // collided with a frame of the other neighbour, which it cannot
        // hear, and the answer never reached the coordinator. The receive
        // status is not cleared before the transmission either: whatever is
        // set then is a frame for the handler.
        if channel_clear() && !rx_pending() {
            ACK_SEEN.set(0);
            set_state(STATE_TX);
            reg::delay_us(TX_SETTLE_US);
            write8(RF_IRQ_STATUS, RF_IRQ_TX);
            // the buffer was filled by `transmit` and is not touched during the transmission
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
    TX_BUF.write32(0, len as u32 + 1);
    TX_BUF.write(4, len as u8 + 2);
    TX_BUF.copy_from(5, mpdu);
    // keep receiving until shortly before; then tune the transmitter and hit the moment
    while until(tick()) > (2 * TX_SETTLE_US * reg::TICKS_PER_US) as i32 {}
    let s = reg::irq_disable();
    set_state(STATE_TX);
    while until(tick()) > 0 {}
    // (a frame received just before stays for the interrupt handler, as in `csma_transmit`)
    write8(RF_IRQ_STATUS, RF_IRQ_TX);
    // the buffer is not touched during the transmission
    start_tx(TX_BUF.addr());
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
        let [attempts, busy, t0] = LAST_TX.get();
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
    // only `transmit` writes the buffer, and it does not run concurrently with itself
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
        busy = !csma_transmit(&mut random);
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
