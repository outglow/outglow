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
    b.get(i..).and_then(<[u8]>::first_chunk).map(|s| u16::from_le_bytes(*s)).ok_or_else(|| Refused("truncated ELF".into()))
}

fn u32_at(b: &[u8], i: usize) -> Result<u32, Refused> {
    b.get(i..).and_then(<[u8]>::first_chunk).map(|s| u32::from_le_bytes(*s)).ok_or_else(|| Refused("truncated ELF".into()))
}

fn uleb(b: &[u8], i: usize) -> Option<(u32, usize)> {
    let mut v = 0u32;
    for (n, &byte) in b.get(i..)?.iter().enumerate() {
        v |= ((byte & 0x7F) as u32).checked_shl(7 * n as u32).unwrap_or(0);
        if byte & 0x80 == 0 {
            return Some((v, i + n + 1));
        }
    }
    None
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
                    } else if t == 4 || (t >= 33 && t & 1 == 1) {
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
        image.resize(image.len().max(paddr + filesz), 0xFF);
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

    const PT_NOTE: u32 = 4;
    const CLANG_TC32_ATTRIBUTES: &[u8] = &[
        0x41, 0x1d, 0x00, 0x00, 0x00, 0x61, 0x65, 0x61, 0x62, 0x69, 0x00, 0x01, 0x13, 0x00, 0x00, 0x00, 0x05, 0x74,
        0x63, 0x33, 0x32, 0x00, 0x06, 0x02, 0x08, 0x00, 0x09, 0x01, 0x22, 0x00,
    ];

    fn sub(tag: u8, attrs: &[u8]) -> Vec<u8> {
        let mut v = vec![tag];
        v.extend_from_slice(&((attrs.len() + 5) as u32).to_le_bytes());
        v.extend_from_slice(attrs);
        v
    }

    fn vendor(name: &str, subs: &[u8]) -> Vec<u8> {
        let mut v = ((name.len() + 5 + subs.len()) as u32).to_le_bytes().to_vec();
        v.extend_from_slice(name.as_bytes());
        v.push(0);
        v.extend_from_slice(subs);
        v
    }

    fn attributes(vendors: &[Vec<u8>]) -> Vec<u8> {
        let mut v = vec![b'A'];
        for x in vendors {
            v.extend_from_slice(x);
        }
        v
    }

    fn cpu(name: &str) -> Vec<u8> {
        let mut v = vec![TAG_CPU_NAME as u8];
        v.extend_from_slice(name.as_bytes());
        v.push(0);
        v
    }

    fn aeabi_cpu(name: &str) -> Vec<u8> {
        vendor("aeabi", &sub(TAG_FILE as u8, &cpu(name)))
    }

    fn build(attrs: &[u8], attrs_section: usize, sections: usize, segs: &[(u32, u32, &[u8])]) -> Vec<u8> {
        let phoff = 0x34usize;
        let data_at = phoff + 32 * segs.len();
        let mut out = vec![0u8; data_at];
        out[..4].copy_from_slice(b"\x7fELF");
        out[4] = 1;
        out[5] = 1;
        out[0x1C..0x20].copy_from_slice(&(phoff as u32).to_le_bytes());
        out[0x2A..0x2C].copy_from_slice(&32u16.to_le_bytes());
        out[0x2C..0x2E].copy_from_slice(&(segs.len() as u16).to_le_bytes());
        for (n, (typ, paddr, bytes)) in segs.iter().enumerate() {
            let at = out.len();
            out.extend_from_slice(bytes);
            let h = phoff + 32 * n;
            out[h..h + 4].copy_from_slice(&typ.to_le_bytes());
            out[h + 4..h + 8].copy_from_slice(&(at as u32).to_le_bytes());
            out[h + 12..h + 16].copy_from_slice(&paddr.to_le_bytes());
            out[h + 16..h + 20].copy_from_slice(&(bytes.len() as u32).to_le_bytes());
        }
        let attrs_at = out.len();
        out.extend_from_slice(attrs);
        let shoff = out.len();
        out.extend_from_slice(&vec![0x5Au8; 40 * sections]);
        for n in 0..sections {
            let sh = shoff + 40 * n;
            let typ: u32 = if n == attrs_section { SHT_ARM_ATTRIBUTES } else { 1 };
            out[sh + 4..sh + 8].copy_from_slice(&typ.to_le_bytes());
        }
        let sh = shoff + 40 * attrs_section;
        out[sh + 16..sh + 20].copy_from_slice(&(attrs_at as u32).to_le_bytes());
        out[sh + 20..sh + 24].copy_from_slice(&(attrs.len() as u32).to_le_bytes());
        out[0x20..0x24].copy_from_slice(&(shoff as u32).to_le_bytes());
        out[0x2E..0x30].copy_from_slice(&40u16.to_le_bytes());
        out[0x30..0x32].copy_from_slice(&(sections as u16).to_le_bytes());
        out
    }

    fn elf(cpu: &str, segs: &[(u32, &[u8])]) -> Vec<u8> {
        let segs: Vec<(u32, u32, &[u8])> = segs.iter().map(|&(p, b)| (PT_LOAD, p, b)).collect();
        build(&attributes(&[aeabi_cpu(cpu)]), 1, 2, &segs)
    }

    fn cpu_of(attrs: &[u8]) -> Option<String> {
        cpu_name(&build(attrs, 1, 2, &[]))
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
    fn a_segment_below_the_end_so_far_overwrites_in_place() {
        let e = elf("tc32", &[(0, &[1, 2, 3, 4]), (1, &[7, 7]), (4, &[5])]);
        assert_eq!(image_of(&e).unwrap(), (vec![1, 7, 7, 4, 5], 3));
    }

    #[test]
    fn only_loadable_segments_with_file_bytes_are_placed() {
        let attrs = attributes(&[aeabi_cpu("tc32")]);
        let e = build(&attrs, 1, 2, &[(PT_NOTE, 0, &[9, 9]), (PT_LOAD, 2, &[1]), (PT_LOAD, 64, &[])]);
        assert_eq!(image_of(&e).unwrap(), (vec![0xFF, 0xFF, 1], 1));
        let e = build(&attrs, 1, 2, &[(PT_NOTE, 0, &[9, 9]), (PT_LOAD, 64, &[])]);
        assert_eq!(image_of(&e), Err(Refused("no loadable segment with bytes in the file".into())));
    }

    #[test]
    fn a_segment_past_the_end_of_the_file_is_refused() {
        let mut e = elf("tc32", &[(0, &[1, 2])]);
        e[0x34 + 16..0x34 + 20].copy_from_slice(&0x1000u32.to_le_bytes());
        assert_eq!(image_of(&e), Err(Refused("segment beyond the end of the file".into())));
    }

    #[test]
    fn a_thumb_elf_is_refused() {
        let e = elf("ARM7TDMI", &[(0, &[1])]);
        assert_eq!(
            image_of(&e),
            Err(Refused("the build attributes name the core Some(\"ARM7TDMI\"), not \"tc32\" (build with -mcpu=tc32)".into()))
        );
        assert!(image_of(&elf("TC32", &[(0, &[1])])).is_ok());
    }

    #[test]
    fn only_a_32_bit_little_endian_elf_is_read() {
        let good = elf("tc32", &[(0, &[1])]);
        let refused = Err(Refused("not a 32-bit little-endian ELF".into()));
        assert_eq!(image_of(&good[..0x33]), refused);
        assert_eq!(image_of(b"\x7fELF\x01\x01\0\0"), refused);
        for (at, value) in [(0, b'E'), (3, b'G'), (4, 2), (5, 2)] {
            let mut e = good.clone();
            e[at] = value;
            assert_eq!(image_of(&e), refused, "byte {at} = {value}");
        }
        let mut header_only = good[..0x34].to_vec();
        header_only[0x30..0x32].copy_from_slice(&0u16.to_le_bytes());
        assert_eq!(image_of(&header_only), Err(Refused("the build attributes name the core None, not \"tc32\" (build with -mcpu=tc32)".into())));
    }

    #[test]
    fn the_attributes_clang_writes_for_tc32() {
        assert_eq!(cpu_of(CLANG_TC32_ATTRIBUTES).as_deref(), Some("tc32"));
    }

    #[test]
    fn the_attributes_section_is_found_at_any_index() {
        let attrs = attributes(&[aeabi_cpu("tc32")]);
        for (at, of) in [(0, 1), (0, 3), (2, 3), (7, 9)] {
            assert_eq!(cpu_name(&build(&attrs, at, of, &[])).as_deref(), Some("tc32"), "section {at} of {of}");
        }
        let mut none = build(&attrs, 1, 2, &[]);
        let shoff = u32::from_le_bytes(none[0x20..0x24].try_into().unwrap()) as usize;
        none[shoff + 44..shoff + 48].copy_from_slice(&1u32.to_le_bytes());
        assert_eq!(cpu_name(&none), None);
    }

    #[test]
    fn truncated_section_headers_or_attributes_name_no_core() {
        let e = elf("tc32", &[]);
        let shoff = u32::from_le_bytes(e[0x20..0x24].try_into().unwrap()) as usize;
        assert_eq!(cpu_name(&e[..0x2F]), None);
        assert_eq!(cpu_name(&e[..shoff + 40 + 20]), None);
        let mut short = e.clone();
        short[shoff + 60..shoff + 64].copy_from_slice(&0x1000u32.to_le_bytes());
        assert_eq!(cpu_name(&short), None);
    }

    #[test]
    fn attributes_not_in_the_a_format_name_no_core() {
        let mut attrs = attributes(&[aeabi_cpu("tc32")]);
        attrs[0] = b'B';
        assert_eq!(cpu_of(&attrs), None);
    }

    #[test]
    fn other_vendors_are_skipped() {
        let decoy = vendor("gnu", &sub(TAG_FILE as u8, &cpu("bogus")));
        assert_eq!(cpu_of(&attributes(&[decoy.clone(), aeabi_cpu("tc32")])).as_deref(), Some("tc32"));
        assert_eq!(cpu_of(&attributes(&[vendor("", &[]), aeabi_cpu("tc32")])).as_deref(), Some("tc32"));
        assert_eq!(cpu_of(&attributes(&[decoy])), None);
    }

    #[test]
    fn a_vendor_section_shorter_than_its_header_ends_the_attributes() {
        let mut attrs = vec![b'A', 4, 0, 0, 0];
        attrs.extend_from_slice(&aeabi_cpu("tc32"));
        assert_eq!(cpu_of(&attrs), None);
        let mut attrs = vec![b'A', 0, 0, 0, 0];
        attrs.extend_from_slice(&aeabi_cpu("tc32"));
        assert_eq!(cpu_of(&attrs), None);
    }

    #[test]
    fn only_file_scope_attributes_name_the_core() {
        let mut subs = sub(2, &[0, 0, 0, 0, TAG_CPU_NAME as u8, b'x', 0]);
        subs.extend_from_slice(&sub(3, &[0, 0, 0, 0, TAG_CPU_NAME as u8, b'y', 0]));
        subs.extend_from_slice(&sub(TAG_FILE as u8, &cpu("tc32")));
        assert_eq!(cpu_of(&attributes(&[vendor("aeabi", &subs)])).as_deref(), Some("tc32"));
    }

    #[test]
    fn the_core_after_a_file_section_without_one() {
        let mut subs = sub(TAG_FILE as u8, &[6, 2]);
        subs.extend_from_slice(&sub(TAG_FILE as u8, &cpu("tc32")));
        assert_eq!(cpu_of(&attributes(&[vendor("aeabi", &subs)])).as_deref(), Some("tc32"));
        let first = vendor("aeabi", &sub(TAG_FILE as u8, &[6, 2]));
        assert_eq!(cpu_of(&attributes(&[first.clone(), vendor("", &[]), aeabi_cpu("tc32")])).as_deref(), Some("tc32"));
        assert_eq!(cpu_of(&attributes(&[first])), None);
    }

    #[test]
    fn every_attribute_form_is_stepped_over() {
        let mut file = vec![];
        file.extend_from_slice(&[4]);
        file.extend_from_slice(b"ARM\0");
        file.extend_from_slice(&[6, 0x82, 0x01]);
        file.extend_from_slice(&[7, 0x4D]);
        file.extend_from_slice(&[9, 1]);
        file.extend_from_slice(&[32, 0x81, 0x00]);
        file.extend_from_slice(b"gnu\0");
        file.extend_from_slice(&[33]);
        file.extend_from_slice(b"odd\0");
        file.extend_from_slice(&[34, 0x05]);
        file.extend_from_slice(&[65]);
        file.extend_from_slice(b"\x06\x05\0");
        file.extend_from_slice(&[67]);
        file.extend_from_slice(b"2.09\0");
        file.extend_from_slice(&[0x81, 0x01]);
        file.extend_from_slice(b"s\0");
        file.extend_from_slice(&[0x80, 0x01, 0x05]);
        file.extend_from_slice(&cpu("tc32"));
        assert_eq!(cpu_of(&attributes(&[vendor("aeabi", &sub(TAG_FILE as u8, &file))])).as_deref(), Some("tc32"));
    }

    #[test]
    fn an_attribute_cut_short_names_no_core() {
        for file in [&[6, 0x82][..], &[4, b'A'], &[32, 0x81], &[32, 1, b'x'], &[5, b't']] {
            let attrs = attributes(&[vendor("aeabi", &sub(TAG_FILE as u8, file))]);
            assert_eq!(cpu_of(&attrs), None, "{file:?}");
        }
        let mut attrs = attributes(&[aeabi_cpu("tc32")]);
        attrs.truncate(10);
        assert_eq!(cpu_of(&attrs), None);
    }

    #[test]
    fn uleb128_values() {
        assert_eq!(uleb(&[0x05], 0), Some((5, 1)));
        assert_eq!(uleb(&[0x00, 0x85, 0x01], 1), Some((133, 3)));
        assert_eq!(uleb(&[0xE5, 0x8E, 0x26], 0), Some((624_485, 3)));
        assert_eq!(uleb(&[0xFF, 0xFF, 0xFF, 0xFF, 0x0F], 0), Some((u32::MAX, 5)));
        assert_eq!(uleb(&[0x80, 0x80, 0x80, 0x80, 0x80, 0x01], 0), Some((0, 6)));
        assert_eq!(uleb(&[0x80], 0), None);
    }

    #[test]
    fn a_refusal_reads_as_its_reason() {
        assert_eq!(Refused("no loadable segment".into()).to_string(), "no loadable segment");
    }
}
