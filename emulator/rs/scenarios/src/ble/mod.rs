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

/// The BLE tests with the module each comes from (`att_read`, `ll_conn_setup`, ...), in alphabetical order.
pub fn modules() -> Vec<(&'static str, Vec<BleTest>)> {
    vec![
        ("att_commands", spec::att_commands::tests()),
        ("att_errors", spec::att_errors::tests()),
        ("att_find", spec::att_find::tests()),
        ("att_mtu", spec::att_mtu::tests()),
        ("att_read", spec::att_read::tests()),
        ("att_server_initiated", spec::att_server_initiated::tests()),
        ("att_write", spec::att_write::tests()),
        ("gap_advertising", spec::gap_advertising::tests()),
        ("gap_conn_params", spec::gap_conn_params::tests()),
        ("gatt_cccd", spec::gatt_cccd::tests()),
        ("gatt_discovery", spec::gatt_discovery::tests()),
        ("gatt_gap_service", spec::gatt_gap_service::tests()),
        ("gatt_properties", spec::gatt_properties::tests()),
        ("l2cap_frames", spec::l2cap_frames::tests()),
        ("l2cap_signaling", spec::l2cap_signaling::tests()),
        ("ll_adv_pdu", spec::ll_adv_pdu::tests()),
        ("ll_adv_requests", spec::ll_adv_requests::tests()),
        ("ll_conn_events", spec::ll_conn_events::tests()),
        ("ll_conn_setup", spec::ll_conn_setup::tests()),
        ("ll_ctrl_instant", spec::ll_ctrl_instant::tests()),
        ("ll_ctrl_procedures", spec::ll_ctrl_procedures::tests()),
        ("smp_pairing", spec::smp_pairing::tests()),
    ]
}

/// All BLE tests, modules in alphabetical order (as ble_spec_suite.py loads them).
pub fn tests() -> Vec<BleTest> {
    modules().into_iter().flat_map(|(_, v)| v).collect()
}
