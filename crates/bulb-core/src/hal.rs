pub trait Pwm {
    fn max_duty(&self) -> u16;
    fn set_duty(&mut self, duty: u16);
    fn has_cold_channel(&self) -> bool {
        false
    }
    fn set_cold_duty(&mut self, _duty: u16) {}
    fn ct_range(&self) -> (u16, u16) {
        (crate::CT_MIN, crate::CT_MAX)
    }
    fn has_rgb(&self) -> bool {
        false
    }
    fn set_rgb(&mut self, _r: u16, _g: u16, _b: u16) {}
    fn white_tw_curve(&self) -> bool {
        self.has_cold_channel()
    }
    fn rgb_beside_white(&self) -> bool {
        false
    }
    fn colour_default(&self) -> (u8, u8) {
        (170, 254)
    }
}

pub trait Clock {
    fn now_ms(&self) -> u64;
}

pub trait ConfigStore {
    fn load(&self, buf: &mut [u8]) -> Option<usize>;
    fn store(&mut self, data: &[u8]);
    fn load_colour(&self, _buf: &mut [u8]) -> Option<usize> {
        None
    }
    fn store_colour(&mut self, _data: &[u8]) {}
}

pub trait Radio {
    fn poll(&mut self) -> Option<crate::command::Command>;
    fn report(&mut self, state: &crate::command::StateReport);
}
