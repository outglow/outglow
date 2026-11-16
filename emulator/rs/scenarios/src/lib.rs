//! Spec scenarios run in-process on any bulb image.
//!
//! The bulb is the zbnode emulator node ([`zbnode::Node`]) loaded with any
//! .bin (stock, the C firmware, the Rust firmware); it shares an 802.15.4 air
//! ([`air`]) with a coordinator and Trust Center ([`coord`]) written apart from
//! `crates/zigbee`, and a test ([`spec`]) drives it through a [`session`]
//! exactly as emulator/spec/ does in Python: black-box, over the radio and the
//! light outputs only.
//!
//! `cargo test -p scenarios` runs the image x test matrix ([`runner`]).

pub mod air;
pub mod ble;
pub mod chan;
pub mod coord;
pub mod crypto;
pub mod frames;
pub mod image;
mod par;
pub mod pcap;
pub mod peer;
pub mod profile;
pub mod py;
pub mod runner;
pub mod session;
pub mod sims;
pub mod spec;
pub mod station;
