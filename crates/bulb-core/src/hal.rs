//! Hardware abstraction layer (HAL). The core sees only these traits; the real
//! firmware implements them on Telink registers, and the emulator — on mocks.

/// PWM output driving the LED power (brightness).
pub trait Pwm {
    /// Maximum duty value (corresponds to 100% brightness).
    fn max_duty(&self) -> u16;
    /// Sets the duty 0..=`max_duty`. On a tunable-white bulb this is the
    /// warm channel.
    fn set_duty(&mut self, duty: u16);
    /// Whether there is a second, cold-white channel (tunable-white bulb).
    fn has_cold_channel(&self) -> bool {
        false
    }
    /// Sets the duty of the cold-white channel.
    fn set_cold_duty(&mut self, _duty: u16) {}
    /// The colour temperatures the two white channels span, in mireds: all
    /// cold at the first, all warm at the second. The board decides it (most
    /// tunable-white bulbs of the range: 153..=370).
    fn ct_range(&self) -> (u16, u16) {
        (crate::CT_MIN, crate::CT_MAX)
    }
    /// A colour bulb: it has red, green and blue channels.
    fn has_rgb(&self) -> bool {
        false
    }
    /// Sets the compare values of the red, green and blue channels (on the
    /// scale of `max_duty`; a value above it is the PWM held high).
    fn set_rgb(&mut self, _r: u16, _g: u16, _b: u16) {}
    /// The white channels follow the tunable-white curve (the sum of a warm
    /// and a cold channel, or the one white channel of a colour bulb) rather
    /// than the dimmable bulb's.
    fn white_tw_curve(&self) -> bool {
        self.has_cold_channel()
    }
    /// The colour channels are a light of their own beside the white one
    /// (EGLO EBF: a white ring and a colour strip): both always show, the
    /// strip at full value, the level and the colour temperature for the ring.
    fn rgb_beside_white(&self) -> bool {
        false
    }
    /// Hue and saturation of a factory-new colour bulb.
    fn colour_default(&self) -> (u8, u8) {
        (170, 254)
    }
}

/// Monotonic clock in milliseconds (e.g. the Telink system tick counter).
pub trait Clock {
    fn now_ms(&self) -> u64;
}

/// Persistent configuration store. In the original this is a flash sector with
/// key-value records; here writing/reading a single block of bytes is enough.
pub trait ConfigStore {
    /// Reads the stored block; `None` if nothing was stored or the record is bad.
    fn load(&self, buf: &mut [u8]) -> Option<usize>;
    /// Writes the block (atomically from the core's point of view).
    fn store(&mut self, data: &[u8]);
    /// The colour record of a colour bulb (`colour::ColourRecord`), kept
    /// beside the configuration; `None` without one.
    fn load_colour(&self, _buf: &mut [u8]) -> Option<usize> {
        None
    }
    fn store_colour(&mut self, _data: &[u8]) {}
}

/// Radio link (Zigbee/mesh). The core receives commands and can report state.
pub trait Radio {
    /// Fetches one pending command, if there is one.
    fn poll(&mut self) -> Option<crate::command::Command>;
    /// Sends a state report to the network (e.g. after a change).
    fn report(&mut self, state: &crate::command::StateReport);
}
