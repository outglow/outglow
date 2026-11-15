//! Diagnostic record of what became of frames (`zigbee::hal::Trace`), and
//! of what kept the radio busy: a ring in SRAM that the emulator's test
//! harness reads (`ZB_TRACE`). Only in builds with the `trace` feature;
//! without it `record` is empty and the firmware is the same as before.
#![cfg_attr(not(feature = "trace"), allow(dead_code))]

/// Kinds of this layer, after the stack's (`zigbee::hal::trace`).
/// A flash write (`detail` 0) or sector erase (1) with interrupts masked:
/// `peer` the sector, `aux` the time taken in system timer ticks.
pub const FLASH: u8 = 16;
/// The radio lent to BLE: an advertising event (`detail` 0), a connection
/// event (1), a mesh burst (2), a mesh listen (3); `aux` the time taken in
/// system timer ticks.
pub const BLE: u8 = 17;
/// Every MAC transmission (`radio::transmit`): `detail` the outcome
/// (`TxStatus` as a number), `seq` the MAC sequence number, `src` the
/// frame length, `peer` the MAC destination, `aux` attempts | busy channel
/// assessments << 8 | time taken in microseconds << 16.
pub const MAC: u8 = 18;

#[cfg(feature = "trace")]
mod ring {
    use zigbee::hal::Trace;

    use crate::cell::SharedBuf;
    use crate::reg;

    /// Entries in the ring, a power of two.
    pub const ENTRIES: usize = 512;

    /// The ring: `magic` (word 0), `head` (word 1), then `ENTRIES` entries
    /// of four words. `head` counts every entry ever written; entry
    /// `head % ENTRIES` is the next. An entry: system timer tick,
    /// kind | detail << 8 | seq << 16, src | peer << 16, aux.
    const LEN: usize = 8 + ENTRIES * 16;
    const MAGIC: u32 = 0x5452_4345;

    const fn ring() -> [u8; LEN] {
        let mut r = [0; LEN];
        let m = MAGIC.to_le_bytes();
        r[0] = m[0];
        r[1] = m[1];
        r[2] = m[2];
        r[3] = m[3];
        r
    }

    #[allow(unsafe_code)] // a linker attribute

    #[no_mangle]
    pub static ZB_TRACE: SharedBuf<LEN> = SharedBuf::new(ring());

    pub fn record(ev: Trace) {
        // interrupts off: an entry and the head move together (the handler never records)
        let s = reg::irq_disable();
        let head = ZB_TRACE.read32(4);
        let at = 8 + (head as usize % ENTRIES) * 16;
        ZB_TRACE.write32(at, reg::tick());
        ZB_TRACE.write32(at + 4, ev.kind as u32 | (ev.detail as u32) << 8 | (ev.seq as u32) << 16);
        ZB_TRACE.write32(at + 8, ev.src as u32 | (ev.peer as u32) << 16);
        ZB_TRACE.write32(at + 12, ev.aux);
        ZB_TRACE.write32(4, head.wrapping_add(1));
        reg::irq_restore(s);
    }
}

/// Adds one record (nothing without the `trace` feature).
#[inline(always)]
pub fn record(_ev: zigbee::hal::Trace) {
    #[cfg(feature = "trace")]
    ring::record(_ev);
}

/// Records a span of radio time taken from 802.15.4 that started at system
/// timer tick `t0` (nothing without the `trace` feature).
#[inline(always)]
pub fn span(_kind: u8, _detail: u8, _peer: u16, _t0: u32) {
    #[cfg(feature = "trace")]
    ring::record(zigbee::hal::Trace {
        kind: _kind,
        detail: _detail,
        seq: 0,
        src: 0,
        peer: _peer,
        aux: crate::reg::tick().wrapping_sub(_t0),
    });
}
