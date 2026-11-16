use crate::machine::Machine;

#[derive(Clone, Debug)]
pub struct I2cDevice {
    pub addr: u8,
    pub mem: [u8; 256],
    pub(crate) ptr: u8,
}

impl I2cDevice {
    pub fn new(addr: u8) -> I2cDevice {
        I2cDevice { addr, mem: [0; 256], ptr: 0 }
    }
}

impl Machine {
    pub(crate) fn i2c_launch(&mut self, sct1: u8) {
        if self.regs[0x03] & 0x02 == 0 {
            return;
        }
        let target = self.regs[0x01] >> 1;
        let mut ack = 0x04u8;
        let (r04, r05) = (self.regs[0x04], self.regs[0x05]);
        match self.i2c_dev.as_mut() {
            Some(d) if d.addr == target => {
                ack = 0;
                if sct1 & 0x02 != 0 {
                    d.ptr = r04;
                }
                if sct1 & 0x04 != 0 {
                    d.mem[d.ptr as usize] = r05;
                    d.ptr = d.ptr.wrapping_add(1);
                }
                if sct1 & 0x08 != 0 {
                    self.regs[0x06] = d.mem[d.ptr as usize];
                    d.ptr = d.ptr.wrapping_add(1);
                }
            }
            _ => {
                if sct1 & 0x08 != 0 {
                    self.regs[0x06] = 0xFF;
                }
            }
        }
        self.regs[0x02] = (self.regs[0x02] & !0x07) | ack;
    }
}
