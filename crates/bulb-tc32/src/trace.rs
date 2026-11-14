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
    use core::ptr::addr_of_mut;
    use zigbee::hal::Trace;

    use crate::reg;

    /// Entries in the ring, a power of two.
    pub const ENTRIES: usize = 512;

    /// `head` counts every entry ever written; entry `head % ENTRIES` is the
    /// next. An entry: system timer tick, kind | detail << 8 | seq << 16,
    /// src | peer << 16, aux.
    #[repr(C)]
    pub struct Ring {
        pub magic: u32,
        pub head: u32,
        pub entries: [[u32; 4]; ENTRIES],
    }

    #[no_mangle]
    pub static mut ZB_TRACE: Ring = Ring { magic: 0x5452_4345, head: 0, entries: [[0; 4]; ENTRIES] };

    pub fn record(ev: Trace) {
        let s = reg::irq_disable();
        // SAFETY: interrupts are off and the interrupt handler never records.
        unsafe {
            let r = &mut *addr_of_mut!(ZB_TRACE);
            let i = r.head as usize % ENTRIES;
            r.entries[i] = [
                reg::tick(),
                ev.kind as u32 | (ev.detail as u32) << 8 | (ev.seq as u32) << 16,
                ev.src as u32 | (ev.peer as u32) << 16,
                ev.aux,
            ];
            core::ptr::write_volatile(&mut r.head, r.head.wrapping_add(1));
        }
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
