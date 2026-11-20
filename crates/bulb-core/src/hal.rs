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
    fn prepare(&mut self) {}
}

pub trait Radio {
    fn poll(&mut self) -> Option<crate::command::Command>;
    fn report(&mut self, state: &crate::command::StateReport);
    fn housekeep(&mut self) -> bool {
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct White;

    impl Pwm for White {
        fn max_duty(&self) -> u16 {
            1000
        }
        fn set_duty(&mut self, _duty: u16) {}
    }

    struct Store;

    impl ConfigStore for Store {
        fn load(&self, _buf: &mut [u8]) -> Option<usize> {
            Some(8)
        }
        fn store(&mut self, _data: &[u8]) {}
    }

    #[test]
    fn a_pwm_by_default_is_one_white_channel() {
        let mut p = White;
        assert!(!p.has_cold_channel());
        assert!(!p.has_rgb());
        assert!(!p.white_tw_curve());
        assert!(!p.rgb_beside_white());
        assert_eq!(p.ct_range(), (crate::CT_MIN, crate::CT_MAX));
        assert_eq!(p.colour_default(), (170, 254));
        p.set_duty(p.max_duty());
        p.set_cold_duty(5);
        p.set_rgb(1, 2, 3);
    }

    #[test]
    fn a_store_by_default_keeps_no_colour() {
        let mut s = Store;
        assert_eq!((s.load(&mut [0; 8]), s.load_colour(&mut [0; 8])), (Some(8), None));
        s.store(&[1]);
        s.store_colour(&[1]);
        s.prepare();
    }
}
