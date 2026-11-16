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
        ZB_TRACE.write32(at + 4, ev.kind as u32 | (ev.detail as u32) << 8 | (ev.seq as u32) << 16);
        ZB_TRACE.write32(at + 8, ev.src as u32 | (ev.peer as u32) << 16);
        ZB_TRACE.write32(at + 12, ev.aux);
        ZB_TRACE.write32(4, head.wrapping_add(1));
        reg::irq_restore(s);
    }
}

#[inline(always)]
pub fn record(_ev: zigbee::hal::Trace) {
    #[cfg(feature = "trace")]
    ring::record(_ev);
}

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
