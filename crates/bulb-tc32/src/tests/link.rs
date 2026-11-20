use super::*;
use bulb_core::command::{ColorMode, PowerOnBehavior, Shown};

fn command(seq: u8, payload: &[u8]) {
    BULB_CMD.write(CMD_SEQ, seq);
    BULB_CMD.copy_from(CMD_PAYLOAD, payload);
}

#[test]
fn the_debug_link_takes_each_command_once_and_counts_the_rejected() {
    let _lock = crate::sim::session();
    let mut l = DebugLink;
    BULB_STATE.write(STATE_DONE, 0);
    BULB_STATE.write(STATE_REJECTED, 0);
    BULB_CMD.write(CMD_SEQ, 0);
    assert_eq!(l.poll(), None);
    command(1, &[1, 0, 0, 0, 0, 0, 0]);
    assert_eq!(l.poll(), Some(Command::On));
    assert_eq!(l.poll(), None);
    command(2, &[0xfe, 0, 0, 0, 0, 0, 0]);
    assert_eq!(l.poll(), None);
    assert_eq!(BULB_STATE.read(STATE_REJECTED), 1);
    assert_eq!(BULB_STATE.read(STATE_DONE), 2);
}

#[test]
fn reports_and_network_state_are_published() {
    let _lock = crate::sim::session();
    let mut l = DebugLink;
    let r = StateReport {
        on: true,
        level: 77,
        power_on_behavior: PowerOnBehavior::Previous,
        startup_level: 9,
        color_temp: 0,
        startup_color_temp: 0,
        color_mode: ColorMode::Temperature,
        hue: 0,
        sat: 0,
        x: 0,
        y: 0,
        hue_moving: false,
        level_fade: None,
        ct_fade_ms: None,
        colour_fading: false,
        candle: false,
        shown: Shown::default(),
    };
    let n = BULB_STATE.read(STATE_REPORTS);
    l.report(&r);
    assert_eq!(BULB_STATE.read(STATE_ON), 1);
    assert_eq!(BULB_STATE.read(STATE_LEVEL), 77);
    assert_eq!(BULB_STATE.read(STATE_POWER_ON_BEHAVIOR), PowerOnBehavior::Previous.to_u8());
    assert_eq!(BULB_STATE.read(STATE_STARTUP_LEVEL), 9);
    assert_eq!(BULB_STATE.read(STATE_REPORTS), n.wrapping_add(1));
    publish_network(2, 0x1234, 15);
    assert_eq!([BULB_NET.read(0), BULB_NET.read(1), BULB_NET.read(2), BULB_NET.read(3)], [2, 15, 0x34, 0x12]);
    publish_start(3, true, false);
    assert_eq!([BULB_NET.read(4), BULB_NET.read(5), BULB_NET.read(6)], [3, 1, 0]);
    publish_reset_step(4);
    assert_eq!(BULB_NET.read(NET_RESET_STEP), 4);
}

#[test]
fn the_hang_command_stops_the_loop_without_feeding_the_watchdog() {
    let _lock = crate::sim::session();
    BULB_STATE.write(STATE_DONE, 0);
    command(7, &[OP_HANG, 0, 0, 0, 0, 0, 0]);
    let mut reads = 0;
    crate::sim::with(|c| {
        c.on(move |_, op| {
            if let crate::sim::Op::R32(crate::reg::SYSTEM_TICK, _) = op {
                reads += 1;
                if reads == 100 {
                    panic!("hung");
                }
            }
        })
    });
    let hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(|_| {}));
    let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| DebugLink.poll()));
    std::panic::set_hook(hook);
    assert_eq!(r.err().and_then(|e| e.downcast_ref::<&str>().copied()), Some("hung"));
    assert_eq!(BULB_STATE.read(STATE_DONE), 7);
    assert!(crate::sim::with(|c| c.writes()).is_empty());
}
