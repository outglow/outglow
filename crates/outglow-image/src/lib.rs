#[cfg(test)]
use memcap as _;
pub mod board;
pub mod elf;
pub mod ota;
pub mod telink;

pub fn crc32(data: &[u8]) -> u32 {
    let mut crc = 0xFFFF_FFFFu32;
    for &b in data {
        crc ^= b as u32;
        for _ in 0..8 {
            crc = if crc & 1 != 0 { (crc >> 1) ^ 0xEDB8_8320 } else { crc >> 1 };
        }
    }
    !crc
}

pub(crate) fn find(hay: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() || hay.len() < needle.len() {
        return None;
    }
    hay.windows(needle.len()).position(|w| w == needle)
}

#[cfg(test)]
mod tests {
    #[test]
    fn crc32_check_value() {
        assert_eq!(super::crc32(b"123456789"), 0xCBF4_3926);
    }

    #[test]
    fn find_the_first_occurrence() {
        assert_eq!(super::find(b"abab", b"ab"), Some(0));
        assert_eq!(super::find(b"xabab", b"ab"), Some(1));
        assert_eq!(super::find(b"ab", b"ab"), Some(0));
        assert_eq!(super::find(b"a", b"ab"), None);
        assert_eq!(super::find(b"abc", b"x"), None);
        assert_eq!(super::find(b"abc", b""), None);
    }
}
