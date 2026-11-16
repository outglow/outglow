//! The spec tests: each states where its expectation comes from (document,
//! clause, requirement in one sentence), as the Python suite in emulator/spec/
//! does. Tests of one session run in order on one joined bulb.

pub mod aps_data;
pub mod bdb_join;
pub mod misc_ota_gp;
pub mod nwk_network;
pub mod nwk_routing;
pub mod parent_role;
pub mod security_keys;
pub mod zcl_basic_identify;
pub mod zcl_color;
pub mod zcl_diagnostics;
pub mod zcl_foundation;
pub mod zcl_groups_scenes;
pub mod zcl_level;
pub mod zcl_onoff;
pub mod zdo_services;

use crate::coord::Coord;
use crate::session::{Session, R};

#[derive(Clone, Copy)]
pub struct Test {
    pub id: &'static str,
    pub doc: &'static str,
    pub clause: &'static str,
    pub req: &'static str,
    /// the shared session the test runs in; "" = a session of its own
    pub session: &'static str,
    pub run: fn(&mut Session) -> R<String>,
    /// mandatory for a Zigbee 3.0 router/light
    pub mandatory: bool,
    /// why the clause can be read in more than one way
    pub ambiguous: Option<&'static str>,
    /// options of the test's session (the first test of a shared session decides)
    pub setup: SessionDef,
}

fn not_set(_: &mut Session) -> R<String> {
    Err(crate::session::Stop::Error("test without a body".into()))
}

impl Test {
    /// Defaults for the fields a test does not set: `Test { id, .., ..Test::D }`.
    pub const D: Test = Test {
        id: "",
        doc: "",
        clause: "",
        req: "",
        session: "",
        run: not_set,
        mandatory: true,
        ambiguous: None,
        setup: SessionDef::D,
    };

    /// The session key: the shared session's name or `own:ID`.
    pub fn group(&self) -> String {
        if self.session.is_empty() {
            format!("own:{}", self.id)
        } else {
            self.session.to_string()
        }
    }
}

/// Options of a session (`define_session` in Python).
#[derive(Clone, Copy, Debug)]
pub struct SessionDef {
    pub join: bool,
    /// light on at full level before every test of a shared session
    pub baseline: bool,
    /// check after every test that the bulb still answers
    pub alive_check: bool,
    /// changes to the Trust Center before it starts (`tc=dict(...)`)
    pub tc: Option<fn(&mut Coord)>,
}

impl SessionDef {
    pub const D: SessionDef = SessionDef { join: true, baseline: true, alive_check: true, tc: None };
}

impl Default for SessionDef {
    fn default() -> Self {
        SessionDef::D
    }
}

/// The tests with the module each comes from (the module names the area: `nwk_routing`, `zcl_level`, ...).
pub fn modules() -> Vec<(&'static str, Vec<Test>)> {
    vec![
        ("aps_data", aps_data::tests()),
        ("bdb_join", bdb_join::tests()),
        ("misc_ota_gp", misc_ota_gp::tests()),
        ("nwk_network", nwk_network::tests()),
        ("nwk_routing", nwk_routing::tests()),
        ("parent_role", parent_role::tests()),
        ("security_keys", security_keys::tests()),
        ("zcl_basic_identify", zcl_basic_identify::tests()),
        ("zcl_color", zcl_color::tests()),
        ("zcl_diagnostics", zcl_diagnostics::tests()),
        ("zcl_foundation", zcl_foundation::tests()),
        ("zcl_groups_scenes", zcl_groups_scenes::tests()),
        ("zcl_level", zcl_level::tests()),
        ("zcl_onoff", zcl_onoff::tests()),
        ("zdo_services", zdo_services::tests()),
    ]
}

pub fn tests() -> Vec<Test> {
    modules().into_iter().flat_map(|(_, v)| v).collect()
}
