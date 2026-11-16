use std::fmt;

const PT_LOAD: u32 = 1;
const SHT_ARM_ATTRIBUTES: u32 = 0x7000_0003;
const TAG_FILE: u32 = 1;
const TAG_CPU_NAME: u32 = 5;

#[derive(Debug, PartialEq)]
pub struct Refused(pub String);

impl fmt::Display for Refused {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for Refused {}

fn u16_at(b: &[u8], i: usize) -> Result<u16, Refused> {
    b.get(i..i + 2).map(|s| u16::from_le_bytes([s[0], s[1]])).ok_or_else(|| Refused("truncated ELF".into()))
}

fn u32_at(b: &[u8], i: usize) -> Result<u32, Refused> {
    b.get(i..i + 4).map(|s| u32::from_le_bytes([s[0], s[1], s[2], s[3]])).ok_or_else(|| Refused("truncated ELF".into()))
}

fn uleb(b: &[u8], mut i: usize) -> Option<(u32, usize)> {
    let (mut v, mut shift) = (0u32, 0);
    loop {
        let byte = *b.get(i)?;
        i += 1;
        if shift < 32 {
            v |= ((byte & 0x7F) as u32) << shift;
        }
        shift += 7;
        if byte & 0x80 == 0 {
            return Some((v, i));
        }
    }
}

fn cstr(b: &[u8], i: usize) -> Option<(String, usize)> {
    let end = i + b.get(i..)?.iter().position(|&c| c == 0)?;
    Some((String::from_utf8_lossy(&b[i..end]).into_owned(), end + 1))
}

pub fn cpu_name(elf: &[u8]) -> Option<String> {
    let shoff = u32_at(elf, 0x20).ok()? as usize;
    let shentsize = u16_at(elf, 0x2E).ok()? as usize;
    let shnum = u16_at(elf, 0x30).ok()? as usize;
    for n in 0..shnum {
        let h = shoff + n * shentsize;
        if u32_at(elf, h + 4).ok()? != SHT_ARM_ATTRIBUTES {
            continue;
        }
        let (off, size) = (u32_at(elf, h + 16).ok()? as usize, u32_at(elf, h + 20).ok()? as usize);
        let a = elf.get(off..off + size)?;
        if a.first() != Some(&b'A') {
            return None;
        }
        let mut i = 1;
        while i + 4 <= a.len() {
            let length = u32_at(a, i).ok()? as usize;
            if length < 5 {
                break;
            }
            let (vendor, mut j) = cstr(a, i + 4)?;
            let end = i + length;
            while vendor == "aeabi" && j < end {
                let (tag, k) = uleb(a, j)?;
                let sub_end = j + u32_at(a, k).ok()? as usize;
                let mut k = k + 4;
                while tag == TAG_FILE && k < sub_end {
                    let (t, next) = uleb(a, k)?;
                    k = next;
                    if t == TAG_CPU_NAME {
                        return Some(cstr(a, k)?.0);
                    }
                    k = if t == 32 {
                        cstr(a, uleb(a, k)?.1)?.1
                    } else if matches!(t, 4 | 5 | 65 | 67) || (t > 32 && t & 1 == 1) {
                        cstr(a, k)?.1
                    } else {
                        uleb(a, k)?.1
                    };
                }
                j = sub_end;
            }
            i = end;
        }
        return None;
    }
    None
}

pub fn image_of(elf: &[u8]) -> Result<(Vec<u8>, usize), Refused> {
    if elf.len() < 0x34 || &elf[..4] != b"\x7fELF" || elf[4] != 1 || elf[5] != 1 {
        return Err(Refused("not a 32-bit little-endian ELF".into()));
    }
    match cpu_name(elf) {
        Some(c) if c.eq_ignore_ascii_case("tc32") => {}
        c => {
            return Err(Refused(format!(
                "the build attributes name the core {c:?}, not \"tc32\" (build with -mcpu=tc32)"
            )))
        }
    }
    let phoff = u32_at(elf, 0x1C)? as usize;
    let phentsize = u16_at(elf, 0x2A)? as usize;
    let phnum = u16_at(elf, 0x2C)? as usize;
    let mut image = Vec::new();
    let mut placed = 0;
    for n in 0..phnum {
        let h = phoff + n * phentsize;
        let (typ, off, paddr, filesz) = (u32_at(elf, h)?, u32_at(elf, h + 4)?, u32_at(elf, h + 12)?, u32_at(elf, h + 16)?);
        if typ != PT_LOAD || filesz == 0 {
            continue;
        }
        let (off, paddr, filesz) = (off as usize, paddr as usize, filesz as usize);
        let src = elf.get(off..off + filesz).ok_or_else(|| Refused("segment beyond the end of the file".into()))?;
        if image.len() < paddr + filesz {
            image.resize(paddr + filesz, 0xFF);
        }
        image[paddr..paddr + filesz].copy_from_slice(src);
        placed += 1;
    }
    if placed == 0 {
        return Err(Refused("no loadable segment with bytes in the file".into()));
    }
    Ok((image, placed))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn elf(cpu: &str, segs: &[(u32, &[u8])]) -> Vec<u8> {
        let mut attrs = vec![b'A'];
        let mut file_sub = vec![TAG_CPU_NAME as u8];
        file_sub.extend_from_slice(cpu.as_bytes());
        file_sub.push(0);
        let mut sub = vec![TAG_FILE as u8];
        sub.extend_from_slice(&((file_sub.len() + 5) as u32).to_le_bytes());
        sub.extend_from_slice(&file_sub);
        let mut vendor = b"aeabi\0".to_vec();
        vendor.extend_from_slice(&sub);
        attrs.extend_from_slice(&((vendor.len() + 4) as u32).to_le_bytes());
        attrs.extend_from_slice(&vendor);

        let phoff = 0x34usize;
        let data_at = phoff + 32 * segs.len();
        let mut out = vec![0u8; data_at];
        out[..4].copy_from_slice(b"\x7fELF");
        out[4] = 1;
        out[5] = 1;
        out[0x1C..0x20].copy_from_slice(&(phoff as u32).to_le_bytes());
        out[0x2A..0x2C].copy_from_slice(&32u16.to_le_bytes());
        out[0x2C..0x2E].copy_from_slice(&(segs.len() as u16).to_le_bytes());
        for (n, (paddr, bytes)) in segs.iter().enumerate() {
            let at = out.len();
            out.extend_from_slice(bytes);
            let h = phoff + 32 * n;
            out[h..h + 4].copy_from_slice(&PT_LOAD.to_le_bytes());
            out[h + 4..h + 8].copy_from_slice(&(at as u32).to_le_bytes());
            out[h + 12..h + 16].copy_from_slice(&paddr.to_le_bytes());
            out[h + 16..h + 20].copy_from_slice(&(bytes.len() as u32).to_le_bytes());
        }
        let attrs_at = out.len();
        out.extend_from_slice(&attrs);
        let shoff = out.len();
        out.extend_from_slice(&[0u8; 80]);
        let sh = shoff + 40;
        out[sh + 4..sh + 8].copy_from_slice(&SHT_ARM_ATTRIBUTES.to_le_bytes());
        out[sh + 16..sh + 20].copy_from_slice(&(attrs_at as u32).to_le_bytes());
        out[sh + 20..sh + 24].copy_from_slice(&(attrs.len() as u32).to_le_bytes());
        out[0x20..0x24].copy_from_slice(&(shoff as u32).to_le_bytes());
        out[0x2E..0x30].copy_from_slice(&40u16.to_le_bytes());
        out[0x30..0x32].copy_from_slice(&2u16.to_le_bytes());
        out
    }

    #[test]
    fn segments_at_their_load_address_gaps_erased() {
        let e = elf("tc32", &[(0, &[1, 2, 3]), (8, &[9, 9])]);
        assert_eq!(cpu_name(&e).as_deref(), Some("tc32"));
        let (img, n) = image_of(&e).unwrap();
        assert_eq!(n, 2);
        assert_eq!(img, vec![1, 2, 3, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 9, 9]);
    }

    #[test]
    fn a_thumb_elf_is_refused() {
        let e = elf("ARM7TDMI", &[(0, &[1])]);
        assert!(image_of(&e).unwrap_err().0.contains("ARM7TDMI"));
    }
}
