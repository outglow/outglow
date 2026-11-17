use super::*;
use crate::nv;
use crate::sim;
use zigbee::hal::NvItem;

fn stack_in(slot: &mut Option<Stack<Tc32Hal>>) -> &mut Stack<Tc32Hal> {
    Stack::new_in(slot, Tc32Hal::new(), crate::zb::IDENTITY)
}

fn at_ms(stack: &mut Stack<Tc32Hal>, ms: u32) {
    let now = stack.hal.now_ms();
    sim::with(|c| c.tick = c.tick.wrapping_add(ms.wrapping_sub(now).wrapping_mul(16_000)).wrapping_sub(16));
}

#[test]
fn identify_and_light_commands_from_the_page_are_handed_to_the_bulb() {
    let _s = sim::session();
    let mut slot = None;
    let stack = stack_in(&mut slot);
    let mut g = Glue::new();
    let seq = stack.event_seq();
    assert_eq!(g.execute(DiagCommand::Identify, stack), Some(Command::Identify { times: 3 }));
    assert_eq!(stack.event_seq(), seq.wrapping_add(1));
    assert_eq!(stack.event(0).map(|e| e.kind), Some(zigbee::stack::ev::IDENTIFY));
    assert_eq!(g.execute(DiagCommand::Light(Command::On), stack), Some(Command::On));
    assert_eq!(g.execute(DiagCommand::LevelOptions(0x01), stack), None);
    assert_eq!(stack.light_settings().level_options, 0x01);
    assert_eq!(g.execute(DiagCommand::ColorOptions(0x01), stack), None);
    assert_eq!(g.restart, None);
}

#[test]
fn a_restart_from_the_page_comes_500_ms_later_and_keeps_the_network() {
    let _s = sim::session();
    let mut slot = None;
    let stack = stack_in(&mut slot);
    nv::open().store(NvItem::Network as u8, &[1, 2, 3]);
    let mut g = Glue::new();
    g.restart_due(stack);
    at_ms(stack, 1_000);
    assert_eq!(g.execute(DiagCommand::Restart, stack), None);
    assert_eq!(g.refresh_at, 1_000);
    assert_eq!(g.restart, Some((1_500, false)));
    at_ms(stack, 1_499);
    g.restart_due(stack);
    at_ms(stack, 1_500);
    assert!(sim::reboots(|| g.restart_due(stack)));
    let mut buf = [0u8; 4];
    assert_eq!(nv::open().load(NvItem::Network as u8, &mut buf), Some(3));
}

#[test]
fn a_factory_reset_from_the_page_forgets_the_network_and_the_retained_state() {
    let _s = sim::session();
    let mut slot = None;
    let stack = stack_in(&mut slot);
    nv::open().store(NvItem::Network as u8, &[1, 2, 3]);
    let mut g = Glue::new();
    at_ms(stack, 2_000);
    assert_eq!(g.execute(DiagCommand::FactoryReset, stack), None);
    assert_eq!(g.restart, Some((2_500, true)));
    at_ms(stack, 2_499);
    g.restart_due(stack);
    let mut buf = [0u8; 4];
    assert_eq!(nv::open().load(NvItem::Network as u8, &mut buf), Some(3));
    at_ms(stack, 2_500);
    assert!(sim::reboots(|| g.restart_due(stack)));
    assert_eq!(nv::open().load(NvItem::Network as u8, &mut buf), None);
    let mut hal = Tc32Hal::new();
    assert_eq!(zigbee::Hal::retained(&mut hal, &mut buf), Some(0));
}

#[test]
fn frames_heard_are_logged_with_their_time_and_counted() {
    let _s = sim::session();
    let n = FRAMES_IN.get();
    frame(7, 0x1234, 0x0006, 0x01, 0x11, 200, -40);
    assert_eq!(FRAMES_IN.get(), n.wrapping_add(1));
    let before = FRAMES.get();
    frame(9, 0x4321, 0x0008, 0x04, 0x01, 100, -60);
    let mut expect = before;
    expect.push(9, 0x4321, 0x0008, 0x04, 0x01, 100, -60);
    assert_eq!(FRAMES.get().bytes(), expect.bytes());
    assert_eq!(FRAMES_IN.get(), n.wrapping_add(2));
}
