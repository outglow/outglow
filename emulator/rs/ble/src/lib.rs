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

pub fn round3(x: f64) -> f64 {
    (x * 1000.0).round() / 1000.0
}
