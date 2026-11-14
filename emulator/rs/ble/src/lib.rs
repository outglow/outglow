// SPDX-License-Identifier: Apache-2.0
//! Rust port of tc32-devtools `go/ble`: the TLSR8278 RF block as a BLE
//! peripheral's link layer drives it (`radio`), a scripted BLE central
//! (`central`), the air between them (`air`), Python's random.Random
//! (`pyrandom`) and the LE Secure Connections functions (`sc`). The block
//! cipher, CCM and the legacy security functions are tc32emu's `aes128`.

pub mod air;
pub mod central;
pub mod gofmt;
pub mod pyrandom;
pub mod radio;
pub mod sc;
pub mod settings;

#[cfg(test)]
mod tests;

pub use air::{millionths, Air, AirLoss, MILLION, TO_HOST, TO_KEYBOARD, WAY_NAMES};
pub use central::{
    default_options, fmt_pdu, AttScript, Central, CentralRc, Notification, Options, PairResult, ParamRequest, Rec, ScriptValue,
    DEFAULT_IRK,
};
pub use radio::{airtime_us, same_peer, Brx, BrxRc, Peer, PeerRc, Radio, RadioRef, TpllPeer, ADV_AA};

/// math.Round(x*1000) / 1000 (half away from zero, as Go's math.Round).
pub fn round3(x: f64) -> f64 {
    (x * 1000.0).round() / 1000.0
}
