use std::env;
use std::sync::OnceLock;

use crate::machine::FLASH_MEM;

fn lower(c: u8) -> u8 {
    c | (b'x' - b'X')
}

fn underscore_ok(s: &str) -> bool {
    let b = s.as_bytes();
    let mut saw = b'^';
    let mut i = 0;
    let mut s = b;
    if !s.is_empty() && (s[0] == b'-' || s[0] == b'+') {
        s = &s[1..];
    }
    let mut hex = false;
    if s.len() >= 2 && s[0] == b'0' && (lower(s[1]) == b'b' || lower(s[1]) == b'o' || lower(s[1]) == b'x') {
        i = 2;
        saw = b'0';
        hex = lower(s[1]) == b'x';
    }
    while i < s.len() {
        let c = s[i];
        if c.is_ascii_digit() || (hex && (b'a'..=b'f').contains(&lower(c))) {
            saw = b'0';
            i += 1;
            continue;
        }
        if c == b'_' {
            if saw != b'0' {
                return false;
            }
            saw = b'_';
            i += 1;
            continue;
        }
        if saw == b'_' {
            return false;
        }
        saw = b'!';
        i += 1;
    }
    saw != b'_'
}

pub fn parse_go_uint(s0: &str) -> Option<u64> {
    let mut s = s0.as_bytes();
    if s.is_empty() {
        return None;
    }
    let mut base: u64 = 10;
    if s[0] == b'0' {
        if s.len() >= 3 && lower(s[1]) == b'b' {
            base = 2;
            s = &s[2..];
        } else if s.len() >= 3 && lower(s[1]) == b'o' {
            base = 8;
            s = &s[2..];
        } else if s.len() >= 3 && lower(s[1]) == b'x' {
            base = 16;
            s = &s[2..];
        } else {
            base = 8;
            s = &s[1..];
        }
    }
    let mut n: u64 = 0;
    let mut underscores = false;
    for &c in s {
        let d = match c {
            b'_' => {
                underscores = true;
                continue;
            }
            b'0'..=b'9' => (c - b'0') as u64,
            b'a'..=b'z' => (c - b'a' + 10) as u64,
            b'A'..=b'Z' => (c - b'A' + 10) as u64,
            _ => return None,
        };
        if d >= base {
            return None;
        }
        n = n.checked_mul(base)?.checked_add(d)?;
    }
    if underscores && !underscore_ok(s0) {
        return None;
    }
    Some(n)
}

pub fn parse_go_int(s: &str) -> Option<i64> {
    if s.is_empty() {
        return None;
    }
    let (neg, rest) = match s.as_bytes()[0] {
        b'+' => (false, &s[1..]),
        b'-' => (true, &s[1..]),
        _ => (false, s),
    };
    let un = parse_go_uint(rest)?;
    let cutoff = 1u64 << 63;
    if !neg && un >= cutoff {
        return None;
    }
    if neg && un > cutoff {
        return None;
    }
    Some(if neg { (un as i64).wrapping_neg() } else { un as i64 })
}

pub fn size_from_env(name: &str, def: i64) -> i64 {
    let s = env::var(name).unwrap_or_default();
    if s.is_empty() {
        return def;
    }
    match parse_go_int(&s) {
        Some(v) if !(v < 0 || (v == 0 && def != 0)) => v,
        _ => panic!("{}: {}", name, s),
    }
}

fn mode_from_env(var: &str, names: &[&str]) -> (usize, u32) {
    let v = env::var(var).unwrap_or_default();
    if v.is_empty() {
        return (0, 0);
    }
    let (name, param) = match v.find(':') {
        Some(i) => (&v[..i], &v[i + 1..]),
        None => (&v[..], ""),
    };
    let mut mode: i64 = -1;
    for (i, n) in names.iter().enumerate() {
        if *n == name {
            mode = i as i64;
        }
    }
    if mode < 0 {
        panic!("{}: {}", var, name);
    }
    if param.is_empty() {
        return (mode as usize, 0);
    }
    match parse_go_int(param) {
        Some(p) => (mode as usize, (p as u32) & 0xFFFFFF),
        None => panic!("{}: {}", var, v),
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Config {
    pub sram_size: usize,
    pub flash_size: usize,
    pub sram_seed: u32,
    pub timer_capture_below: String,
    pub adc_mode: usize,
    pub adc_param: u32,
    pub adc_state_hz: i64,
    pub pke_us: i64,
    pub pke_pver_us: i64,
    pub rbg_mode: usize,
    pub rbg_param: u32,
    pub k32_jitter_ns: i64,
    pub k32_jitter_seed: u64,
    pub k32_frozen: bool,
    pub icache_miss: i64,
    pub cycle_accurate: bool,
    pub pad_c_pf: i64,
    pub pad_r_pct: i64,
    pub flash_beyond: String,
    pub flash_tpp_us: i64,
    pub flash_tse_us: i64,
    pub flash_tbe32_us: i64,
    pub flash_tbe64_us: i64,
    pub tick_offset: u32,
}

impl Config {
    fn from_env() -> Config {
        let sram_size = {
            let s = env::var("TC32EMU_SRAM_SIZE").unwrap_or_default();
            if s.is_empty() {
                0x10000
            } else {
                match parse_go_int(&s) {
                    Some(v) if v > 0 => v as usize,
                    _ => panic!("TC32EMU_SRAM_SIZE: {}", s),
                }
            }
        };
        let timer_capture_below = {
            let v = env::var("TC32EMU_TIMER_CAPTURE_BELOW").unwrap_or_default();
            if v.is_empty() {
                "stop".to_string()
            } else if v != "stop" && v != "wrap" {
                panic!("TC32EMU_TIMER_CAPTURE_BELOW: {}", v)
            } else {
                v
            }
        };
        let (adc_mode, adc_param) = mode_from_env("TC32EMU_ADC", crate::adc::ADC_MODES);
        let adc_state_hz = {
            let v = env::var("TC32EMU_ADC_STATE_HZ").unwrap_or_default();
            if v.is_empty() {
                12_000_000
            } else {
                match parse_go_int(&v) {
                    Some(hz) if hz > 0 => hz,
                    _ => panic!("TC32EMU_ADC_STATE_HZ: {}", v),
                }
            }
        };
        let (rbg_mode, rbg_param) = mode_from_env("TC32EMU_RBG", crate::rbg::RBG_MODES);
        let k32_jitter_seed = {
            let s = env::var("TC32EMU_K32_JITTER_SEED").unwrap_or_default();
            if s.is_empty() {
                1
            } else {
                match parse_go_uint(&s) {
                    Some(v) => v,
                    None => panic!("TC32EMU_K32_JITTER_SEED: {}", s),
                }
            }
        };
        let frozen = env::var("TC32EMU_K32_FROZEN").unwrap_or_default();
        let flash_beyond = {
            let mode = env::var("TC32EMU_FLASH_BEYOND").unwrap_or_default();
            let mode = if mode.is_empty() { "stop".to_string() } else { mode };
            if mode != "stop" && mode != "ff" && mode != "wrap" {
                panic!("TC32EMU_FLASH_BEYOND: {}", mode);
            }
            mode
        };
        Config {
            sram_size,
            flash_size: size_from_env("TC32EMU_FLASH_SIZE", FLASH_MEM as i64) as usize,
            sram_seed: size_from_env("TC32EMU_SRAM_SEED", 0) as u32,
            timer_capture_below,
            adc_mode,
            adc_param,
            adc_state_hz,
            pke_us: size_from_env("TC32EMU_PKE_US", 30000),
            pke_pver_us: size_from_env("TC32EMU_PKE_PVER_US", 200),
            rbg_mode,
            rbg_param,
            k32_jitter_ns: size_from_env("TC32EMU_K32_JITTER_NS", 0),
            k32_jitter_seed,
            k32_frozen: !frozen.is_empty() && frozen != "0",
            icache_miss: size_from_env("TC32EMU_ICACHE_MISS", 0),
            cycle_accurate: env::var("TC32EMU_CYCLE_ACCURATE").unwrap_or_default() == "1",
            pad_c_pf: size_from_env("TC32EMU_PAD_C_PF", 0),
            pad_r_pct: size_from_env("TC32EMU_PAD_R_PCT", 100),
            flash_beyond,
            flash_tpp_us: size_from_env("TC32EMU_FLASH_TPP_US", 0),
            flash_tse_us: size_from_env("TC32EMU_FLASH_TSE_US", 0),
            flash_tbe32_us: size_from_env("TC32EMU_FLASH_TBE32_US", 0),
            flash_tbe64_us: size_from_env("TC32EMU_FLASH_TBE64_US", 0),
            tick_offset: {
                let s = env::var("TC32EMU_TICK_OFFSET").unwrap_or_default();
                if s.is_empty() {
                    0
                } else {
                    match parse_go_uint(&s) {
                        Some(v) => v as u32,
                        None => panic!("TC32EMU_TICK_OFFSET: {}", s),
                    }
                }
            },
        }
    }

    pub fn env() -> Config {
        static CFG: OnceLock<Config> = OnceLock::new();
        CFG.get_or_init(Config::from_env).clone()
    }
}
