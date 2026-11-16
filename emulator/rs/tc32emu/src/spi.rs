use crate::error::{emu_err, Error};
use crate::machine::Machine;

pub const SPI_FIFO_MAX: usize = 4096;

#[derive(Clone, Debug, Default)]
pub struct SpiDevice {
    pub mode: i64,
    pub k: u8,
    pub cs: i64,
    pub high: bool,
    pub fifo: Vec<u8>,
    pub count: i64,
    pub transactions: i64,
    pub(crate) answered: i64,
    pub(crate) prev: u8,
    pub(crate) selected: bool,
}

impl Machine {
    pub(crate) fn spi_running(&self) -> bool {
        let r = &self.regs;
        self.spi_dev.is_some() && r[0x09] & 0x02 != 0 && r[0x0A] & 0x80 != 0 && r[0x63] & 0x01 != 0 && r[0x60] & 0x01 == 0
    }

    fn spi_pin(&self, port: usize, pin: usize, func: usize) -> bool {
        let r = &self.regs;
        let mux = ((r[0x5A8 + 2 * port + (pin >> 2)] >> (2 * (pin & 3))) & 3) as usize;
        mux == func && (r[0x586 + 8 * port] >> pin) & 1 == 0
    }

    fn spi_routes(&self) -> (bool, bool, bool) {
        let r = &self.regs;
        let ck = (self.spi_pin(0, 4, 0) && r[0x5B6] & 0x20 != 0) || (self.spi_pin(3, 7, 0) && r[0x5B6] & 0x80 != 0);
        let d_o = self.spi_pin(0, 2, 0) || self.spi_pin(1, 7, 1);
        let di = (self.spi_pin(0, 3, 0) && r[0x5B7] & 0x01 != 0 && r[0x581] & 0x08 != 0)
            || (self.spi_pin(1, 6, 1) && r[0x5B7] & 0x04 != 0 && r[0x589] & 0x40 != 0);
        (ck, d_o, di)
    }

    fn spi_selected(&mut self) -> bool {
        let (cs, high) = match &self.spi_dev {
            Some(d) => (d.cs, d.high),
            None => return false,
        };
        if cs == 0xFF {
            return self.spi_pin(3, 2, 0) && self.regs[0x09] & 0x01 == 0;
        }
        let (port, bit) = ((cs >> 3) as usize, (cs & 7) as u32);
        let (lvl, flt) = self.pad_levels(port);
        (flt >> bit) & 1 == 0 && (((lvl >> bit) & 1 == 1) == high)
    }

    pub(crate) fn spi_cs_update(&mut self) {
        if self.spi_dev.is_none() {
            return;
        }
        let sel = self.spi_selected();
        if let Some(d) = self.spi_dev.as_mut() {
            if sel && !d.selected {
                d.transactions += 1;
                d.prev = 0xFF;
            }
            d.selected = sel;
        }
    }

    pub(crate) fn spi_octet(&mut self, out: i64) {
        let (ck, d_o, di) = self.spi_routes();
        self.spi_busy_until = self.cycles + 16 * ((self.regs[0x0A] & 0x7F) as i64 + 1);
        let mut ans = 0u8;
        if self.spi_dev.is_some() && ck {
            self.spi_cs_update();
            if let Some(d) = self.spi_dev.as_mut() {
                if d.selected {
                    ans = match d.mode {
                        0 => d.prev,
                        1 => d.k,
                        _ => (d.k as i64).wrapping_add(d.answered) as u8,
                    };
                    d.answered += 1;
                    if d_o && out >= 0 {
                        d.count += 1;
                        if d.fifo.len() < SPI_FIFO_MAX {
                            d.fifo.push(out as u8);
                        }
                        d.prev = out as u8;
                    }
                }
            }
        }
        self.spi_rx = if ck && di { ans } else { 0 };
    }

    pub(crate) fn spi_attach(&mut self, val: u32) -> Result<(), Error> {
        if val & 0xFF > 2 {
            return Err(emu_err(format!("0xffd8: no SPI device answer {}", val & 0xFF)));
        }
        self.spi_dev = Some(SpiDevice {
            mode: (val & 0xFF) as i64,
            k: (val >> 8) as u8,
            cs: ((val >> 16) & 0xFF) as i64,
            high: (val >> 24) & 1 != 0,
            prev: 0xFF,
            ..Default::default()
        });
        let sel = self.spi_selected();
        if let Some(d) = self.spi_dev.as_mut() {
            d.selected = sel;
        }
        if self.has_event() {
            let high = if self.spi_dev.as_ref().map(|d| d.high).unwrap_or(false) { " active high" } else { "" };
            let text = format!("SPI device: answer {}, chip select 0x{:02x}{}", val & 0xFF, (val >> 16) & 0xFF, high);
            self.event(&text);
        }
        Ok(())
    }
}
