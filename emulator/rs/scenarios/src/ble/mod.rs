//! The BLE conformance tests (emulator/spec_ble/ in Python): the bulb as an LE
//! peripheral, advertiser and GATT server, driven by a scripted central.
//! `cargo test -p scenarios --test ble` runs them on every image.

pub mod harness;
pub mod mesh;
pub mod runner;
pub mod spec;
pub mod util;

use harness::BleSession;

use crate::session::R;

/// How a failure counts (`level` in Python).
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Level {
    Shall,
    /// a recommendation: not following it gives NOTE, not FAIL
    Should,
    /// the failure depends on how an ambiguous clause is read: NOTE too
    Reading,
}

#[derive(Clone, Copy)]
pub struct BleTest {
    pub id: &'static str,
    pub doc: &'static str,
    pub clause: &'static str,
    pub req: &'static str,
    /// a session shared with other tests; "" = a boot of its own
    pub session: &'static str,
    pub run: fn(&mut BleSession) -> R<String>,
    pub mandatory: bool,
    pub level: Level,
    pub ambiguous: Option<&'static str>,
}

fn not_set(_: &mut BleSession) -> R<String> {
    Err(crate::session::Stop::Error("test without a body".into()))
}

impl BleTest {
    pub const D: BleTest = BleTest {
        id: "",
        doc: "",
        clause: "",
        req: "",
        session: "",
        run: not_set,
        mandatory: true,
        level: Level::Shall,
        ambiguous: None,
    };

    pub fn group(&self) -> String {
        if self.session.is_empty() {
            format!("own:{}", self.id)
        } else {
            self.session.to_string()
        }
    }
}

/// All BLE tests, modules in alphabetical order (as ble_spec_suite.py loads them).
pub fn tests() -> Vec<BleTest> {
    let mut v = Vec::new();
    v.extend(spec::att_commands::tests());
    v.extend(spec::att_errors::tests());
    v.extend(spec::att_find::tests());
    v.extend(spec::att_mtu::tests());
    v.extend(spec::att_read::tests());
    v.extend(spec::att_server_initiated::tests());
    v.extend(spec::att_write::tests());
    v.extend(spec::gap_advertising::tests());
    v.extend(spec::gap_conn_params::tests());
    v.extend(spec::gatt_cccd::tests());
    v.extend(spec::gatt_discovery::tests());
    v.extend(spec::gatt_gap_service::tests());
    v.extend(spec::gatt_properties::tests());
    v.extend(spec::l2cap_frames::tests());
    v.extend(spec::l2cap_signaling::tests());
    v.extend(spec::ll_adv_pdu::tests());
    v.extend(spec::ll_adv_requests::tests());
    v.extend(spec::ll_conn_events::tests());
    v.extend(spec::ll_conn_setup::tests());
    v.extend(spec::ll_ctrl_instant::tests());
    v.extend(spec::ll_ctrl_procedures::tests());
    v.extend(spec::smp_pairing::tests());
    v
}
