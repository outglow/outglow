use crate::sim::*;
use zigbee::stack::{Stack, State};

const BASIC: u16 = 0x0000;
const DIAG: u16 = 0x0B05;
const MFR: u16 = 0x1141;

fn frame_counter(net: &mut Net, short: u16) -> u32 {
    let seq = net.coord.zcl_mfr(short, DIAG, 0x00, MFR, 0x00, &0x5035u16.to_le_bytes());
    net.run(50);
    let rsp = net.coord.last_zcl_mfr(short, DIAG, 0x01).unwrap();
    assert_eq!(rsp[..9], [0x1C, 0x41, 0x11, seq, 0x01, 0x35, 0x50, 0x00, 0x23]);
    u32::from_le_bytes([rsp[9], rsp[10], rsp[11], rsp[12]])
}

fn aps_counters(net: &Net, n: usize, since: u32) -> Vec<u32> {
    net.nwk_sent_by(net.nodes[n].id, since)
        .into_iter()
        .filter(|(_, h, p)| h.dst == 0x0000 && p[0] & 0x20 != 0 && p[0] & 3 == 0)
        .map(|(_, _, p)| u32::from_le_bytes([p[9], p[10], p[11], p[12]]))
        .collect()
}

#[test]
fn frame_counters_used_past_the_stored_step_are_never_used_again_after_a_power_cut() {
    let mut net = Net::new();
    let n = net.join_node();
    let short = net.nodes[n].short();
    let t = net.now();
    for _ in 0..1100 {
        net.coord.zcl_encrypted(short, BASIC, 0x00, 0x00, &[0x00, 0x00]);
        net.run(4);
    }
    net.run(500);
    let before = aps_counters(&net, n, t);
    assert!(before.len() >= 1050, "{} secured answers", before.len());
    let last_aps = *before.last().unwrap();
    assert!(last_aps > 1024);
    let last_nwk = frame_counter(&mut net, short);
    assert!(last_nwk > 1100);
    net.power_cycle(n);
    net.run(3_000);
    assert_eq!(net.nodes[n].stack.state(), State::Up);
    let t = net.now();
    net.coord.zcl_encrypted(short, BASIC, 0x00, 0x00, &[0x00, 0x00]);
    net.run(200);
    let after = aps_counters(&net, n, t);
    assert!(after[0] > last_aps, "APS frame counter {} after {last_aps}", after[0]);
    assert!(frame_counter(&mut net, short) > last_nwk);
}

#[test]
fn a_stack_made_in_place_comes_back_up_like_one_made_by_value() {
    let mut net = Net::new();
    let n = net.join_node();
    let short = net.nodes[n].short();
    let id = net.nodes[n].id;
    let mut slot = None;
    let made = Stack::new_in(&mut slot, SimHal { air: net.air.clone(), id, bx: Box::new(zigbee::BlackBox::EMPTY) }, IDENTITY);
    assert_eq!((made.state(), made.short_address(), made.pan_id()), (State::Up, short, PAN));
    net.nodes[n].stack = slot.take().unwrap();
    net.run(1_000);
    let seq = net.coord.zcl(short, BASIC, 0x00, 0x00, &[0x00, 0x00]);
    net.run(100);
    assert_eq!(net.coord.last_zcl(short, BASIC, 0x01).unwrap()[..3], [0x18, seq, 0x01]);
}
