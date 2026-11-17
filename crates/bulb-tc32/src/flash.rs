use crate::cell::Shared;
use crate::reg;

const CMD_WRITE_ENABLE: u8 = 0x06;
const CMD_PAGE_PROGRAM: u8 = 0x02;
const CMD_SECTOR_ERASE: u8 = 0x20;
const CMD_READ_STATUS: u8 = 0x05;
const STATUS_BUSY: u8 = 0x01;
const PAGE: usize = 256;

#[inline(always)]
fn wait() {
    while reg::read8(reg::MSPI_CTRL) & reg::MSPI_BUSY != 0 {}
}

#[inline(always)]
fn send(byte: u8) {
    reg::write8(reg::MSPI_DATA, byte);
    wait();
}

#[inline(always)]
fn command(cmd: u8) {
    reg::write8(reg::MSPI_CTRL, reg::MSPI_CS_HIGH);
    reg::write8(reg::MSPI_CTRL, 0);
    send(cmd);
}

#[inline(always)]
fn address(addr: usize) {
    send((addr >> 16) as u8);
    send((addr >> 8) as u8);
    send(addr as u8);
}

#[inline(always)]
fn finish() {
    reg::write8(reg::MSPI_CTRL, reg::MSPI_CS_HIGH);
    command(CMD_READ_STATUS);
    loop {
        send(0);
        if reg::read8(reg::MSPI_DATA) & STATUS_BUSY == 0 {
            break;
        }
    }
    reg::write8(reg::MSPI_CTRL, reg::MSPI_CS_HIGH);
}

#[cfg_attr(target_os = "none", link_section = ".ram_code")]
#[inline(never)]
extern "C" fn program(addr: usize, data: &[u8; 32], len: usize) {
    command(CMD_WRITE_ENABLE);
    command(CMD_PAGE_PROGRAM);
    address(addr);
    let mut i = 0;
    while let Some(&b) = data.get(i) {
        if i >= len {
            break;
        }
        send(b);
        i += 1;
    }
    finish();
}

#[cfg_attr(target_os = "none", link_section = ".ram_code")]
#[inline(never)]
extern "C" fn erase(addr: usize) {
    command(CMD_WRITE_ENABLE);
    command(CMD_SECTOR_ERASE);
    address(addr);
    finish();
}

pub fn write(addr: usize, data: &[u8]) {
    let mut done = 0;
    while done < data.len() {
        let at = addr + done;
        let room = PAGE - (at % PAGE);
        let mut buf = [0u8; 32];
        let n = (data.len() - done).min(room).min(buf.len());
        buf[..n].copy_from_slice(&data[done..done + n]);
        #[cfg(feature = "trace")]
        let t0 = reg::tick();
        let s = reg::irq_disable();
        program(at, &buf, n);
        reg::irq_restore(s);
        #[cfg(feature = "trace")]
        crate::trace::span(crate::trace::FLASH, 0, (at >> 12) as u16, t0);
        done += n;
    }
}

pub static ERASES: Shared<u32> = Shared::new(0);
pub static ERASE_MAX_US: Shared<u32> = Shared::new(0);
pub static STORES: Shared<u32> = Shared::new(0);

pub fn stored() {
    STORES.update(|n| n.wrapping_add(1));
}

pub fn erase_sector(addr: usize) {
    if !(crate::staging::BASE..crate::staging::BASE + crate::staging::SIZE as usize).contains(&addr) {
        ERASES.update(|n| n.wrapping_add(1));
    }
    let t0 = reg::tick();
    let s = reg::irq_disable();
    erase(addr & !0xfff);
    reg::irq_restore(s);
    let us = reg::tick().wrapping_sub(t0) / reg::TICKS_PER_US;
    ERASE_MAX_US.update(|m| m.max(us));
    #[cfg(feature = "trace")]
    crate::trace::span(crate::trace::FLASH, 1, (addr >> 12) as u16, t0);
}

#[cfg(target_os = "none")]
pub fn read(addr: usize, buf: &mut [u8]) {
    for (i, b) in buf.iter_mut().enumerate() {
        *b = unsafe { ((addr + i) as *const u8).read_volatile() };
    }
}

#[cfg(not(target_os = "none"))]
pub fn read(addr: usize, buf: &mut [u8]) {
    for (i, b) in buf.iter_mut().enumerate() {
        *b = crate::sim::flash8(addr + i);
    }
}

pub fn blank(sector: usize) -> bool {
    let mut chunk = [0u8; 32];
    let mut off = 0;
    while off < 0x1000 {
        read(sector + off, &mut chunk);
        if chunk.iter().any(|&b| b != 0xff) {
            return false;
        }
        off += chunk.len();
    }
    true
}

#[cfg(test)]
#[path = "tests/flash.rs"]
mod tests;
