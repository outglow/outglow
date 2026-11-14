//! What the touchlink scenarios share: the touchlink master key read at run
//! time from the C firmware's ELF (it is not in this repository; the Telink
//! library carries it, emulator/elf_symbol.py), inter-PAN frame parsing, the
//! key transport of key index 4 and NWK frame decryption.

use tc32emu::aes128;

use crate::crypto::ccm_decrypt;
use crate::frames::{mac_parse, Mac};

pub const ZLL_PROFILE: u16 = 0xC05E;
pub const TOUCHLINK: u16 = 0x1000;

pub fn command_name(c: u8) -> Option<&'static str> {
    Some(match c {
        0x00 => "Scan Request",
        0x01 => "Scan Response",
        0x02 => "Device Information Request",
        0x03 => "Device Information Response",
        0x06 => "Identify Request",
        0x07 => "Reset to Factory New Request",
        0x10 => "Network Start Request",
        0x11 => "Network Start Response",
        0x12 => "Network Join Router Request",
        0x13 => "Network Join Router Response",
        0x14 => "Network Join End Device Request",
        0x15 => "Network Join End Device Response",
        0x16 => "Network Update Request",
        _ => return None,
    })
}

/// Contents of the data object `name` in an ELF file (`length` bytes, else its
/// symbol's size); None when there is no such symbol (`elf_symbol.symbol_bytes`).
pub fn symbol_bytes(path: &std::path::Path, name: &str, length: Option<usize>) -> Option<Vec<u8>> {
    let d = std::fs::read(path).ok()?;
    if d.get(..4)? != b"\x7fELF" || *d.get(4)? != 1 || *d.get(5)? != 1 {
        return None;
    }
    let u16_at = |i: usize| -> Option<usize> { Some(u16::from_le_bytes(d.get(i..i + 2)?.try_into().ok()?) as usize) };
    let u32_at = |i: usize| -> Option<usize> { Some(u32::from_le_bytes(d.get(i..i + 4)?.try_into().ok()?) as usize) };
    let shoff = u32_at(0x20)?;
    let (shentsize, shnum) = (u16_at(0x2E)?, u16_at(0x30)?);
    let sec = |k: usize, f: usize| u32_at(shoff + k * shentsize + 4 * f);
    for k in 0..shnum {
        if sec(k, 1)? != 2 {
            continue; // SHT_SYMTAB
        }
        let (off, size, link, entsize) = (sec(k, 4)?, sec(k, 5)?, sec(k, 6)?, sec(k, 9)?);
        let strtab = sec(link, 4)?;
        for i in 0..size / entsize {
            let e = off + i * entsize;
            let (st_name, value, st_size) = (u32_at(e)?, u32_at(e + 4)?, u32_at(e + 8)?);
            let shndx = u16_at(e + 14)?;
            let s0 = strtab + st_name;
            let end = s0 + d[s0..].iter().position(|b| *b == 0)?;
            if &d[s0..end] != name.as_bytes() || !(0 < shndx && shndx < shnum) {
                continue;
            }
            let (addr, sec_off) = (sec(shndx, 3)?, sec(shndx, 4)?);
            let at = sec_off + value - addr;
            return d.get(at..at + length.unwrap_or(st_size)).map(|b| b.to_vec());
        }
    }
    None
}

/// The master key from the ELF next to a .bin (`os.path.splitext(bin)[0] + ".elf"`).
pub fn master_key(bin: &std::path::Path) -> (std::path::PathBuf, Option<[u8; 16]>) {
    let elf = bin.with_extension("elf");
    let k = if elf.exists() { symbol_bytes(&elf, "touchLinkKeyMaster", Some(16)) } else { None };
    (elf, k.filter(|k| !k.is_empty()).and_then(|k| k.try_into().ok()))
}

/// (ZCL command, payload after it, MAC fields) of a touchlink inter-PAN frame
/// (`interpan` of rust_touchlink_initiator_sim.py).
pub fn interpan(psdu: &[u8]) -> Option<(u8, Vec<u8>, Mac)> {
    let f = mac_parse(psdu)?;
    let p = &f.payload;
    if f.ty != 1 || p.len() < 10 || p[0] & 3 != 3 || p[2] & 3 != 3 {
        return None;
    }
    let i = 3 + if (p[2] >> 2) & 3 == 3 { 2 } else { 0 };
    let cl = u16::from_le_bytes([*p.get(i)?, *p.get(i + 1)?]);
    let pr = u16::from_le_bytes([*p.get(i + 2)?, *p.get(i + 3)?]);
    if (cl, pr) != (TOUCHLINK, ZLL_PROFILE) {
        return None;
    }
    let cmd = *p.get(i + 6)?;
    let rest = p.get(i + 7..).unwrap_or(&[]).to_vec();
    Some((cmd, rest, f))
}

/// The key transport key of key index 4: the master key's encryption of the
/// transaction and response identifiers, each twice (big-endian).
pub fn transport_key(master: &[u8; 16], transaction: u32, response: u32) -> [u8; 16] {
    let (t, r) = (transaction.to_be_bytes(), response.to_be_bytes());
    let mut b = [0u8; 16];
    b[..4].copy_from_slice(&t);
    b[4..8].copy_from_slice(&t);
    b[8..12].copy_from_slice(&r);
    b[12..].copy_from_slice(&r);
    aes128::encrypt(master, &b)
}

/// The network key as an initiator sends it under key index 4.
pub fn encrypted_key(master: &[u8; 16], key: &[u8; 16], transaction: u32, response: u32) -> [u8; 16] {
    aes128::encrypt(&transport_key(master, transaction, response), key)
}

pub fn decrypted_key(master: &[u8; 16], encrypted: &[u8], transaction: u32, response: u32) -> [u8; 16] {
    aes128::decrypt(&transport_key(master, transaction, response), encrypted)
}

/// A secured NWK frame (MAC payload `q`) decrypted with `key`: (header fcf, src, dst, payload or None).
/// None when it is not a secured NWK frame of the expected length.
pub fn nwk_decrypt(q: &[u8], key: &[u8; 16]) -> Option<(u16, u16, u16, Option<Vec<u8>>)> {
    if q.len() < 8 + 14 + 4 {
        return None;
    }
    let fcf = u16::from_le_bytes([q[0], q[1]]);
    let dst = u16::from_le_bytes([q[2], q[3]]);
    let src = u16::from_le_bytes([q[4], q[5]]);
    if fcf & 0x0200 == 0 {
        return None;
    }
    let i = 8 + if fcf & 0x0800 != 0 { 8 } else { 0 } + if fcf & 0x1000 != 0 { 8 } else { 0 };
    let sc = *q.get(i)?;
    let fc = u32::from_le_bytes(q.get(i + 1..i + 5)?.try_into().ok()?);
    let sext = u64::from_le_bytes(q.get(i + 5..i + 13)?.try_into().ok()?);
    let sc5 = (sc & !7) | 5;
    let mut nonce = [0u8; 13];
    nonce[..8].copy_from_slice(&sext.to_le_bytes());
    nonce[8..12].copy_from_slice(&fc.to_le_bytes());
    nonce[12] = sc5;
    let mut aad = q[..i].to_vec();
    aad.push(sc5);
    aad.extend_from_slice(q.get(i + 1..i + 14)?);
    let body = q.get(i + 14..)?;
    Some((fcf, src, dst, ccm_decrypt(key, &nonce, &aad, body)))
}
