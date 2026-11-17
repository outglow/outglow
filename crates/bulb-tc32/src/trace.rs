#![cfg_attr(not(feature = "trace"), allow(dead_code))]

pub const FLASH: u8 = 16;
pub const BLE: u8 = 17;
pub const MAC: u8 = 18;

#[cfg(feature = "trace")]
mod ring {
    use zigbee::hal::Trace;

    use crate::cell::SharedBuf;
    use crate::reg;

    pub const ENTRIES: usize = 512;

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

    #[allow(unsafe_code)]

    #[no_mangle]
    pub static ZB_TRACE: SharedBuf<LEN> = SharedBuf::new(ring());

    pub fn record(ev: Trace) {
        let s = reg::irq_disable();
        let head = ZB_TRACE.read32(4);
        let at = 8 + (head as usize % ENTRIES) * 16;
        ZB_TRACE.write32(at, reg::tick());
        ZB_TRACE.write32(at + 4, u32::from_le_bytes([ev.kind, ev.detail, ev.seq, 0]));
        let ([s0, s1], [p0, p1]) = (ev.src.to_le_bytes(), ev.peer.to_le_bytes());
        ZB_TRACE.write32(at + 8, u32::from_le_bytes([s0, s1, p0, p1]));
        ZB_TRACE.write32(at + 12, ev.aux);
        ZB_TRACE.write32(4, head.wrapping_add(1));
        reg::irq_restore(s);
    }
}

#[cfg(feature = "trace")]
pub fn tx_aux([attempts, busy, t0]: [u32; 3]) -> u32 {
    let [u0, u1, ..] = (crate::reg::tick().wrapping_sub(t0) / crate::reg::TICKS_PER_US).min(0xffff).to_le_bytes();
    u32::from_le_bytes([attempts.min(255) as u8, busy.min(255) as u8, u0, u1])
}

#[cfg(feature = "trace")]
pub use ring::record;

#[cfg(feature = "trace")]
pub fn span(kind: u8, detail: u8, peer: u16, t0: u32) {
    record(zigbee::hal::Trace { kind, detail, seq: 0, src: 0, peer, aux: crate::reg::tick().wrapping_sub(t0) });
}

#[cfg(all(test, feature = "trace"))]
#[path = "tests/trace.rs"]
mod tests;
