#![forbid(unsafe_code)]
#![no_std]
//! Zigbee PRO router stack for the bulb, independent of the hardware.
//!
//! Layers, bottom up: IEEE 802.15.4 MAC framing ([`mac`]), the network layer
//! with frame security and mesh routing, APS, the ZDO server and the ZCL
//! clusters of a dimmable light ([`stack`]). The radio, the clock, the random
//! source and the non-volatile store sit behind [`hal::Hal`], so the whole
//! stack runs in host tests against a simulated network.

#[cfg(test)]
extern crate std;

pub mod aes;
pub mod crypto;
pub mod hal;
pub mod mac;
pub mod nwk;
pub mod stack;
pub mod util;

pub use hal::{Hal, RxMeta, TxStatus};
pub use stack::blackbox::BlackBox;
pub use stack::Stack;
