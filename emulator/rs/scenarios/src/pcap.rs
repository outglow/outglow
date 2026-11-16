use std::fs::File;
use std::io::{self, Write};
use std::path::Path;

pub struct Pcap {
    f: File,
}

impl Pcap {
    pub fn create(path: &Path) -> io::Result<Pcap> {
        let mut f = File::create(path)?;
        let mut h = Vec::new();
        h.extend_from_slice(&0xA1B2C3D4u32.to_le_bytes());
        h.extend_from_slice(&2u16.to_le_bytes());
        h.extend_from_slice(&4u16.to_le_bytes());
        h.extend_from_slice(&0i32.to_le_bytes());
        h.extend_from_slice(&0u32.to_le_bytes());
        h.extend_from_slice(&256u32.to_le_bytes());
        h.extend_from_slice(&195u32.to_le_bytes());
        f.write_all(&h)?;
        Ok(Pcap { f })
    }

    pub fn write(&mut self, ms: f64, psdu: &[u8]) -> io::Result<()> {
        let us = (ms * 1000.0) as u64;
        let mut r = Vec::with_capacity(16 + psdu.len());
        r.extend_from_slice(&((us / 1_000_000) as u32).to_le_bytes());
        r.extend_from_slice(&((us % 1_000_000) as u32).to_le_bytes());
        r.extend_from_slice(&(psdu.len() as u32).to_le_bytes());
        r.extend_from_slice(&(psdu.len() as u32).to_le_bytes());
        r.extend_from_slice(psdu);
        self.f.write_all(&r)?;
        self.f.flush()
    }
}
