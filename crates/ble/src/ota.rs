use crate::crypto::{crc16_modbus, crc32_update};
use crate::hal::{Hal, TICKS_PER_MS};
use crate::{reached, Ble};

const SECTOR: u32 = 0x1000;
const BOOT_MARK: u32 = 8;
const ERASE_STEP: u32 = 10 * TICKS_PER_MS;
const RESTART_AFTER: u32 = 1000 * TICKS_PER_MS;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum State {
    Idle = 0,
    Erasing = 1,
    Ready = 2,
    Receiving = 3,
    Done = 4,
}

pub(crate) struct Ota {
    state: State,
    failed: bool,
    received: u16,
    next: u16,
    block: u16,
    erased: u32,
    erase_goal: u32,
    declared: u32,
    bytes: u32,
    marked: bool,
    crc: u32,
    tail: [u8; 4],
    timer: Option<u32>,
}

impl Ota {
    pub fn new() -> Ota {
        Ota {
            state: State::Idle,
            failed: false,
            received: 0,
            next: 0,
            block: 0,
            erased: 0,
            erase_goal: 0,
            declared: 0,
            bytes: 0,
            marked: true,
            crc: 0xffff_ffff,
            tail: [0; 4],
            timer: None,
        }
    }

    pub fn active(&self) -> bool {
        matches!(self.state, State::Erasing | State::Ready | State::Receiving)
    }

    fn fail(&mut self) {
        self.failed = true;
        self.state = State::Done;
    }

    fn account(&mut self, offset: u32, data: &[u8]) -> bool {
        for (i, b) in data.iter().enumerate() {
            let at = offset + i as u32;
            if (8..12).contains(&at) && *b != b"KNLT"[(at - 8) as usize] {
                self.marked = false;
            }
            if (0x18..0x1c).contains(&at) {
                self.declared |= (*b as u32) << (8 * (at - 0x18));
            }
            if at < 0x1c || at + 4 < self.declared {
                self.crc = crc32_update(self.crc, *b);
            } else if at < self.declared {
                self.tail[(at + 4 - self.declared) as usize] = *b;
            }
        }
        self.bytes = offset + data.len() as u32;
        self.marked
    }

    fn image_good(&self) -> bool {
        self.marked && self.declared >= 0x40 && u32::from_le_bytes(self.tail) == self.crc
    }
}

fn is_erase_packet(d: &[u8]) -> bool {
    d.len() >= 3 && d[0] == 0 && d[1] == 0 && d[2] != 0 && d[3..].iter().all(|b| *b == 0)
}

impl<H: Hal> Ble<H> {
    pub(crate) fn ota_status(&self) -> [u8; 20] {
        let o = &self.ota;
        let mut s = [0u8; 20];
        s[0] = o.state as u8;
        s[1] = if o.failed { 2 } else { 0 };
        s[2..4].copy_from_slice(&o.received.to_le_bytes());
        s[4..6].copy_from_slice(&o.received.to_le_bytes());
        if o.declared != 0 {
            s[7] = (o.received as u32 * o.block as u32 * 100 / o.declared).min(100) as u8;
        }
        s
    }

    fn ota_erase_to(&mut self, end: u32) {
        let end = end.min(self.hal.ota_capacity());
        while self.ota.erased < end {
            self.hal.ota_erase(self.ota.erased);
            self.ota.erased += SECTOR;
        }
    }

    pub(crate) fn ota_timers(&mut self, now: u32) {
        let Some(at) = self.ota.timer else { return };
        if !reached(now, at) {
            return;
        }
        self.ota.timer = None;
        match self.ota.state {
            State::Erasing if self.ota.erased < self.ota.erase_goal => {
                self.ota_erase_to(self.ota.erased + SECTOR);
                self.ota.timer = Some(self.hal.now().wrapping_add(ERASE_STEP));
            }
            State::Erasing => self.ota.state = State::Ready,
            State::Done if !self.ota.failed => {
                self.ota.failed = !self.hal.ota_activate();
            }
            _ => {}
        }
    }

    pub fn ota_busy(&self) -> bool {
        (self.ota.state == State::Done && !self.ota.failed)
            || (self.link.state == crate::LinkState::Connected && self.ota_this_link)
    }

    pub(crate) fn ota_link_lost(&mut self) {
        self.ota_this_link = false;
        if self.ota.active() {
            self.ota = Ota::new();
        }
    }

    pub(crate) fn ota_write(&mut self, d: &[u8]) {
        self.ota_this_link = true;
        if self.ota.state == State::Done && !self.ota.failed {
            return;
        }
        if d.len() == 4 {
            let index = u16::from_le_bytes([d[0], d[1]]);
            let good = crc16_modbus(&d[..2]) == u16::from_le_bytes([d[2], d[3]])
                && (index == 0xffff || index == self.ota.next)
                && self.ota.state == State::Receiving
                && self.ota.image_good();
            if !good {
                self.ota.fail();
                return;
            }
            self.ota.state = State::Done;
            self.ota.timer = Some(self.hal.now().wrapping_add(RESTART_AFTER));
            return;
        }
        let fresh = matches!(self.ota.state, State::Idle | State::Done);
        if fresh && is_erase_packet(d) {
            self.ota = Ota::new();
            self.ota.erase_goal = (d[2] as u32 * SECTOR).min(self.hal.ota_capacity());
            self.ota.state = State::Erasing;
            self.ota.timer = Some(self.hal.now().wrapping_add(ERASE_STEP));
            return;
        }
        if d.len() < 5 {
            return;
        }
        let (body, crc) = d.split_at(d.len() - 2);
        if crc16_modbus(body) != u16::from_le_bytes([crc[0], crc[1]]) {
            return;
        }
        let index = u16::from_le_bytes([body[0], body[1]]);
        let block = &body[2..];
        match self.ota.state {
            State::Idle | State::Done => {
                if index != 0 {
                    return;
                }
                self.ota = Ota::new();
                self.ota.state = State::Ready;
            }
            State::Erasing => {
                self.ota.state = State::Ready;
                self.ota.timer = None;
            }
            _ => {}
        }
        if self.ota.state == State::Ready {
            if index != 0 {
                self.ota.fail();
                return;
            }
            self.ota.block = block.len() as u16;
            self.ota.next = 0;
            self.ota.state = State::Receiving;
        }
        if index.wrapping_add(1) == self.ota.next {
            return;
        }
        let offset = index as u32 * self.ota.block as u32;
        let end = offset + block.len() as u32;
        if index != self.ota.next || index == u16::MAX || block.len() > self.ota.block as usize || end > self.hal.ota_capacity() {
            self.ota.fail();
            return;
        }
        let mut buf = [0u8; crate::att::MTU_MAX];
        let buf = &mut buf[..block.len()];
        buf.copy_from_slice(block);
        let good = self.ota.account(offset, buf);
        if !good || (self.ota.bytes >= 0x1c && !(0x40..=self.hal.ota_capacity()).contains(&self.ota.declared)) {
            self.ota.fail();
            return;
        }
        if offset <= BOOT_MARK && end > BOOT_MARK {
            buf[(BOOT_MARK - offset) as usize] = 0xff;
        }
        self.ota_erase_to(end);
        self.hal.ota_write(offset, buf);
        self.ota.next = index + 1;
        self.ota.received += 1;
    }
}

#[cfg(test)]
mod tests;
