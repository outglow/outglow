use super::super::address_tests::{Scripted, IDENTITY};
use super::{Commissioning, Forward, Gpd, Pending};
use crate::mac::Addr;
use crate::stack::{Stack, State};
use crate::util::Reader;
extern crate std;
use std::vec::Vec;

#[test]
fn a_device_address_is_read_for_the_source_id_and_ieee_application_ids_only() {
    let b = [0x45, 0x23, 0x71, 0x01, 0x11, 0x22, 0x33, 0x44, 0x07];
    assert!(Gpd::read(0, &mut Reader::new(&b)) == Some((Gpd::Src(0x0171_2345), 0)));
    assert!(Gpd::read(2, &mut Reader::new(&b)) == Some((Gpd::Ieee(0x4433_2211_0171_2345), 0x07)));
    for app in [1, 3, 7] {
        assert!(Gpd::read(app, &mut Reader::new(&b)).is_none(), "{app}");
    }
}

#[test]
fn a_commissioning_frame_finds_no_room_while_two_notifications_wait() {
    let mut s = Stack::new(Scripted { random: Vec::new(), now: 1_000, sent: Vec::new(), deaf: Vec::new() }, IDENTITY);
    s.state = State::Up;
    s.gp_mode = Some(Commissioning { until: 60_000, sink: 0x0000, unicast: true, exit_on_pairing: false });
    let waiting = Pending { at: 50_000, to: Forward::Sink(0x0000), alias: 0, seq: 0, then: 0, len: 3, zcl: [0; 96] };
    s.gp_pending = [Some(waiting), Some(waiting)];
    let _ = s.on_gpdf(0x31, Addr::None, &[0x0C, 0x45, 0x23, 0x71, 0x01, 0xE0, 0x02, 0x00, 0x00], 200, 0);
    assert!(s.gp_pending.iter().all(|p| p.is_some_and(|p| p.at == 50_000)));
    s.gp_pending = [Some(waiting), None];
    let _ = s.on_gpdf(0x32, Addr::None, &[0x0C, 0x45, 0x23, 0x71, 0x01, 0xE0, 0x02, 0x00, 0x00], 200, 0);
    assert!(s.gp_pending[1].is_some(), "with room it waits");
}
