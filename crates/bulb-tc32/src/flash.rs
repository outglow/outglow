//! SPI NOR flash through the MSPI registers: page program, sector erase, read.
//!
//! While a command is on the MSPI, and while the flash is busy programming or
//! erasing, code cannot be fetched from the flash. Everything from chip select
//! low until the flash is idle again therefore runs from SRAM (`.ram_code`,
//! which the boot ROM copies there) and calls nothing outside it.
//!
//! Interrupts are masked around `program` and `erase`: the interrupt path
//! must not run while the flash cannot be read. Frames that arrive meanwhile
//! are lost and their senders retry (a sector erase takes tens of ms).

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

/// Programs `len` bytes at `data` to `addr`; they must lie in one 256-byte page.
#[link_section = ".ram_code"]
#[inline(never)]
extern "C" fn program(addr: usize, data: &[u8; 32], len: usize) {
    command(CMD_WRITE_ENABLE);
    command(CMD_PAGE_PROGRAM);
    address(addr);
    let mut i = 0;
    // `get`, not indexing: a bounds-check panic would call into the flash
    while let Some(&b) = data.get(i) {
        if i >= len {
            break;
        }
        send(b);
        i += 1;
    }
    finish();
}

#[link_section = ".ram_code"]
#[inline(never)]
extern "C" fn erase(addr: usize) {
    command(CMD_WRITE_ENABLE);
    command(CMD_SECTOR_ERASE);
    address(addr);
    finish();
}

/// Writes `data` at flash address `addr` (bits can only go from 1 to 0).
pub fn write(addr: usize, data: &[u8]) {
    let mut done = 0;
    while done < data.len() {
        let at = addr + done;
        let room = PAGE - (at % PAGE);
        // Through a buffer on the stack: the source must not be in the flash.
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

/// Sector erases outside the OTA staging area since the start (the
/// settings' wear), and records written to the light's settings and the
/// power-on counter (`board.rs`): for the Diagnostics cluster.
pub static ERASES: Shared<u32> = Shared::new(0);
/// The longest sector erase since the start (the flash and the interrupts
/// are busy that long), microseconds.
pub static ERASE_MAX_US: Shared<u32> = Shared::new(0);
pub static STORES: Shared<u32> = Shared::new(0);

/// Counts a record written by a store the Zigbee stack does not see.
pub fn stored() {
    STORES.update(|n| n.wrapping_add(1));
}

/// Erases the 4 KB sector containing `addr`.
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

/// Reads flash contents; the flash is mapped at address 0.
pub fn read(addr: usize, buf: &mut [u8]) {
    for (i, b) in buf.iter_mut().enumerate() {
        // SAFETY: flash addresses are readable memory on this chip.
        *b = unsafe { ((addr + i) as *const u8).read_volatile() };
    }
}

/// True when the 4 KB sector at `sector` is erased.
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
