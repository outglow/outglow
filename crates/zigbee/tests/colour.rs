use bulb_core::command::{Command, HueDirection};
use crate::sim::*;

const IDENTIFY: u16 = 0x0003;
const LEVEL: u16 = 0x0008;
const COLOR: u16 = 0x0300;
const SCENES: u16 = 0x0005;
const GROUPS: u16 = 0x0004;

fn colour_bulb() -> (Net, usize, u16) {
    let mut net = Net::new();
    net.identity = IDENTITY_COLOR;
    let n = net.join_node();
    let short = net.nodes[n].short();
    (net, n, short)
}

fn read(net: &mut Net, short: u16, ids: &[u16]) -> Vec<u8> {
    let p: Vec<u8> = ids.iter().flat_map(|i| i.to_le_bytes()).collect();
    let seq = net.coord.zcl(short, COLOR, 0x00, 0x00, &p);
    net.run(50);
    let rsp = net.coord.last_zcl(short, COLOR, 0x01).unwrap();
    assert_eq!(&rsp[..3], &[0x18, seq, 0x01]);
    rsp[3..].to_vec()
}

fn last_command(net: &Net, n: usize) -> Command {
    *net.nodes[n].commands.last().unwrap()
}

#[test]
fn a_colour_bulb_is_an_extended_color_light_with_hue_saturation_and_xy() {
    let (mut net, _, short) = colour_bulb();
    let s = short.to_le_bytes();
    net.coord.zdo(short, 0x0004, &[s[0], s[1], 1]);
    net.run(50);
    let d = net.coord.last_zdo(short, 0x8004).unwrap();
    assert_eq!(&d[8..10], &[0x0D, 0x01], "device 0x010D");
    assert_eq!(read(&mut net, short, &[0x400A, 0x0008, 0x4001]), vec![0x0A, 0x40, 0, 0x19, 0x19, 0x00, 0x08, 0x00, 0, 0x30, 2, 0x01, 0x40, 0, 0x30, 2]);
    assert_eq!(read(&mut net, short, &[0x0000, 0x0001, 0x0003, 0x0004]),
               vec![0, 0, 0, 0x20, 170, 1, 0, 0, 0x20, 254, 3, 0, 0, 0x21, 0, 0, 4, 0, 0, 0x21, 0, 0]);
    let seq = net.coord.zcl(short, COLOR, 0x00, 0x11, &[0, 20]);
    net.run(50);
    let rsp = net.coord.last_zcl(short, COLOR, 0x12).unwrap();
    assert_eq!(rsp, vec![0x18, seq, 0x12, 1, 0x00, 0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08, 0x09, 0x0A, 0x47, 0x4B, 0x4C]);
}

#[test]
fn a_tunable_white_bulb_has_none_of_the_colour() {
    let mut net = Net::new();
    net.identity = IDENTITY_TW;
    let n = net.join_node();
    let short = net.nodes[n].short();
    assert_eq!(read(&mut net, short, &[0x0000, 0x0003]), vec![0, 0, 0x86, 3, 0, 0x86]);
    let seq = net.coord.zcl(short, COLOR, 0x01, 0x06, &[40, 200, 0, 0]);
    net.run(50);
    assert_eq!(net.coord.last_zcl(short, COLOR, 0x0B).unwrap(), vec![0x18, seq, 0x0B, 0x06, 0x81]);
    let seq = net.coord.zcl(short, COLOR, 0x00, 0x02, &[0x00, 0x00, 0x20, 5]);
    net.run(50);
    assert_eq!(net.coord.last_zcl(short, COLOR, 0x04).unwrap(), vec![0x18, seq, 0x04, 0x86, 0x00, 0x00]);
}

#[test]
fn hue_and_saturation_commands() {
    let (mut net, n, short) = colour_bulb();
    net.coord.zcl(short, COLOR, 0x11, 0x06, &[40, 200, 10, 0]);
    net.run(50);
    assert_eq!(last_command(&net, n), Command::MoveToHueSaturation { hue: 40, sat: 200, direction: HueDirection::Shortest, transition_ms: 1000 });
    assert_eq!(read(&mut net, short, &[0x0000, 0x0001, 0x0008]), vec![0, 0, 0, 0x20, 40, 1, 0, 0, 0x20, 200, 8, 0, 0, 0x30, 0]);
    net.coord.zcl(short, COLOR, 0x11, 0x00, &[100, 3, 0, 0]);
    net.run(50);
    assert_eq!(last_command(&net, n), Command::MoveToHueSaturation { hue: 100, sat: 200, direction: HueDirection::Down, transition_ms: 0 });
    net.coord.zcl(short, COLOR, 0x11, 0x02, &[1, 200, 5]);
    net.run(50);
    assert_eq!(last_command(&net, n), Command::MoveToHueSaturation { hue: 45, sat: 200, direction: HueDirection::Up, transition_ms: 500 });
    net.coord.zcl(short, COLOR, 0x11, 0x03, &[30, 0, 0]);
    net.run(50);
    assert_eq!(last_command(&net, n), Command::MoveToHueSaturation { hue: 45, sat: 30, direction: HueDirection::Shortest, transition_ms: 0 });
    net.coord.zcl(short, COLOR, 0x11, 0x05, &[3, 100, 0]);
    net.run(50);
    assert_eq!(last_command(&net, n), Command::MoveToHueSaturation { hue: 45, sat: 0, direction: HueDirection::Shortest, transition_ms: 0 });
    net.coord.zcl(short, COLOR, 0x11, 0x04, &[1, 127]);
    net.run(50);
    assert_eq!(last_command(&net, n), Command::MoveToHueSaturation { hue: 45, sat: 254, direction: HueDirection::Shortest, transition_ms: 2000 });
    net.coord.zcl(short, COLOR, 0x11, 0x01, &[1, 254]);
    net.run(50);
    assert_eq!(last_command(&net, n), Command::MoveHue { up: true, rate: 254 << 8 }, "ZCL 5.2.2.3.5: on until stopped");
    net.coord.zcl(short, COLOR, 0x11, 0x01, &[0, 0]);
    net.run(50);
    assert_eq!(last_command(&net, n), Command::StopColorTransition);
    for (cmd, p) in [(0x00u8, vec![255u8, 0, 0, 0]), (0x00, vec![10, 4, 0, 0]), (0x06, vec![10, 255, 0, 0]), (0x01, vec![1, 0]), (0x02, vec![2, 1, 0])] {
        let seq = net.coord.zcl(short, COLOR, 0x01, cmd, &p);
        net.run(50);
        assert_eq!(net.coord.last_zcl(short, COLOR, 0x0B).unwrap(), vec![0x18, seq, 0x0B, cmd, 0x85], "command {cmd:#x}");
    }
}

#[test]
fn xy_commands() {
    let (mut net, n, short) = colour_bulb();
    net.coord.zcl(short, COLOR, 0x11, 0x07, &[0xCD, 0x4C, 0x99, 0x99, 20, 0]);
    net.run(50);
    assert_eq!(last_command(&net, n), Command::MoveToXy { x: 0x4CCD, y: 0x9999, transition_ms: 2000 });
    assert_eq!(read(&mut net, short, &[0x0003, 0x0004, 0x0008]), vec![3, 0, 0, 0x21, 0xCD, 0x4C, 4, 0, 0, 0x21, 0x99, 0x99, 8, 0, 0, 0x30, 1]);
    net.coord.zcl(short, COLOR, 0x11, 0x09, &[0x00, 0x10, 0x00, 0x80, 0, 0]);
    net.run(50);
    assert_eq!(last_command(&net, n), Command::MoveToXy { x: 0x5CCD, y: 0x1999, transition_ms: 0 });
    net.coord.zcl(short, COLOR, 0x11, 0x08, &[0x00, 0x10, 0x00, 0x00]);
    net.run(50);
    let Command::MoveToXy { x, y, transition_ms } = last_command(&net, n) else { panic!() };
    assert_eq!((x, y), (0xFEFE, 0x1999));
    assert_eq!(transition_ms, (0xFEFF - 0x5CCD) * 1000 / 0x1000);
    net.coord.zcl(short, COLOR, 0x11, 0x0A, &[250, 0, 0, 0]);
    net.run(50);
    assert_eq!(read(&mut net, short, &[0x0008, 0x4001]), vec![8, 0, 0, 0x30, 2, 0x01, 0x40, 0, 0x30, 2]);
}

#[test]
fn a_light_that_is_off_takes_a_colour_only_with_execute_if_off() {
    let (mut net, n, short) = colour_bulb();
    net.coord.zcl(short, 0x0006, 0x11, 0x00, &[]);
    net.run(50);
    let before = net.nodes[n].commands.len();
    net.coord.zcl(short, COLOR, 0x11, 0x06, &[40, 200, 10, 0, 0x01, 0x00]);
    net.run(50);
    assert_eq!(net.nodes[n].commands.len(), before, "masked off: nothing");
    net.coord.zcl(short, COLOR, 0x11, 0x07, &[1, 2, 3, 4, 10, 0, 0x01, 0x01]);
    net.run(50);
    assert_eq!(last_command(&net, n), Command::MoveToXy { x: 0x0201, y: 0x0403, transition_ms: 0 }, "at once");
}

#[test]
fn scenes_keep_the_colour_and_its_mode() {
    let (mut net, n, short) = colour_bulb();
    net.coord.zcl(short, GROUPS, 0x01, 0x00, &[0x34, 0x12, 0]);
    net.run(50);
    net.coord.zcl(short, COLOR, 0x11, 0x06, &[40, 200, 0, 0]);
    net.run(50);
    net.coord.zcl(short, SCENES, 0x01, 0x04, &[0x34, 0x12, 1]);
    net.run(50);
    net.coord.zcl(short, COLOR, 0x11, 0x07, &[0xCD, 0x4C, 0x99, 0x99, 0, 0]);
    net.run(50);
    net.coord.zcl(short, SCENES, 0x01, 0x04, &[0x34, 0x12, 2]);
    net.run(50);
    net.coord.zcl(short, COLOR, 0x11, 0x0A, &[250, 0, 0, 0]);
    net.run(50);
    let before = net.nodes[n].commands.len();
    net.coord.zcl(short, SCENES, 0x01, 0x05, &[0x34, 0x12, 1]);
    net.run(50);
    assert!(net.nodes[n].commands[before..].contains(&Command::MoveToHueSaturation { hue: 40, sat: 200, direction: HueDirection::Shortest, transition_ms: 0 }));
    let before = net.nodes[n].commands.len();
    net.coord.zcl(short, SCENES, 0x01, 0x05, &[0x34, 0x12, 2]);
    net.run(50);
    assert!(net.nodes[n].commands[before..].contains(&Command::MoveToXy { x: 0x4CCD, y: 0x9999, transition_ms: 0 }));
    let seq = net.coord.zcl(short, SCENES, 0x01, 0x01, &[0x34, 0x12, 2]);
    net.run(50);
    let rsp = net.coord.last_zcl(short, SCENES, 0x01).unwrap();
    let at = rsp.windows(2).position(|w| w == [0x00, 0x03]).unwrap();
    assert_eq!(rsp[1], seq);
    assert_eq!(&rsp[at..at + 17], &[0x00, 0x03, 14, 0xCD, 0x4C, 0x99, 0x99, 0, 40, 200, 0, 0, 0, 0, 0x4D, 0x01, 1]);
    net.power_cycle(n);
    net.run(1000);
    let before = net.nodes[n].commands.len();
    net.coord.zcl(short, SCENES, 0x01, 0x05, &[0x34, 0x12, 1]);
    net.run(50);
    assert!(net.nodes[n].commands[before..].contains(&Command::MoveToHueSaturation { hue: 40, sat: 200, direction: HueDirection::Shortest, transition_ms: 0 }));
    let mut add = vec![0x34, 0x12, 3, 0, 0, 0];
    add.extend_from_slice(&[0x00, 0x03, 14, 0x00, 0x20, 0x00, 0x30, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1]);
    net.coord.zcl(short, SCENES, 0x01, 0x00, &add);
    net.run(50);
    let before = net.nodes[n].commands.len();
    net.coord.zcl(short, SCENES, 0x01, 0x05, &[0x34, 0x12, 3]);
    net.run(50);
    assert!(net.nodes[n].commands[before..].contains(&Command::MoveToXy { x: 0x2000, y: 0x3000, transition_ms: 0 }));
}

#[test]
fn the_colour_is_reported_once_configured() {
    let (mut net, n, short) = colour_bulb();
    let ext = net.air.borrow().ports[net.nodes[n].id].ext;
    let mut bind = ext.to_le_bytes().to_vec();
    bind.push(1);
    bind.extend_from_slice(&COLOR.to_le_bytes());
    bind.push(3);
    bind.extend_from_slice(&COORD_EXT.to_le_bytes());
    bind.push(1);
    net.coord.zdo(short, 0x0021, &bind);
    net.run(2_000);
    let seq = net.coord.zcl(short, COLOR, 0x00, 0x06, &[0x00, 0x03, 0x00, 0x21, 1, 0, 0x10, 0x0E, 0x10, 0x00, 0x00, 0x00, 0x00, 0x20, 1, 0, 0x10, 0x0E, 1]);
    net.run(50);
    assert_eq!(net.coord.last_zcl(short, COLOR, 0x07).unwrap(), vec![0x18, seq, 0x07, 0x00], "configured");
    let reports = |net: &Net, attr: u8| net.coord.zcl_rx.iter().filter(|e| e.1 == COLOR && e.2[2] == 0x0A && e.2[3] == attr).count();
    net.coord.zcl(short, COLOR, 0x11, 0x07, &[0x00, 0x40, 0x00, 0x40, 0, 0]);
    net.run(3_000);
    assert!(reports(&net, 0x03) >= 1, "x reported");
    net.coord.zcl(short, COLOR, 0x11, 0x00, &[80, 0, 0, 0]);
    net.run(3_000);
    assert!(reports(&net, 0x00) >= 1, "hue reported");
}

fn colour_status(net: &mut Net, short: u16, cmd: u8, payload: &[u8]) -> u8 {
    let seq = net.coord.zcl(short, COLOR, 0x01, cmd, payload);
    net.run(50);
    let r = net.coord.last_zcl(short, COLOR, 0x0B).unwrap();
    assert_eq!(r[..4], [0x18, seq, 0x0B, cmd]);
    r[4]
}

#[test]
fn colour_commands_with_a_reserved_mode_a_rate_of_zero_or_a_saturation_out_of_range_are_invalid_fields() {
    let (mut net, _, short) = colour_bulb();
    for (cmd, payload) in [
        (0x01u8, vec![2u8, 10]),
        (0x01, vec![1, 0]),
        (0x01, vec![3, 0]),
        (0x02, vec![0, 10, 0]),
        (0x02, vec![2, 10, 0]),
        (0x03, vec![255, 0, 0]),
        (0x04, vec![2, 10]),
        (0x04, vec![1, 0]),
        (0x05, vec![0, 10, 0]),
        (0x05, vec![2, 10, 0]),
        (0x4B, vec![2, 10, 0, 0, 0, 0, 0]),
        (0x4B, vec![1, 0, 0, 0, 0, 0, 0]),
        (0x4C, vec![2, 10, 0, 0, 0, 0, 0, 0, 0]),
    ] {
        assert_eq!(colour_status(&mut net, short, cmd, &payload), 0x85, "command {cmd:#04x} {payload:?}");
    }
    assert_eq!(colour_status(&mut net, short, 0x04, &[0, 0]), 0x00, "mode 0 stops, rate or not");
}

#[test]
fn move_color_runs_to_the_edge_of_the_space_in_both_directions_and_zero_rates_stop() {
    let (mut net, n, short) = colour_bulb();
    net.coord.zcl(short, COLOR, 0x11, 0x07, &[0x00, 0x40, 0x00, 0x20, 0, 0]);
    net.run(50);
    let before = net.nodes[n].commands.len();
    net.coord.zcl(short, COLOR, 0x11, 0x08, &(-0x0400i16).to_le_bytes().iter().chain(&0x0100i16.to_le_bytes()).copied().collect::<Vec<u8>>());
    net.run(50);
    assert_eq!(net.nodes[n].commands[before..], [Command::MoveToXy { x: 0, y: 0x2000 + 0x0100 * 16, transition_ms: 16_000 }]);
    let before = net.nodes[n].commands.len();
    net.coord.zcl(short, COLOR, 0x11, 0x08, &[0, 0, 0, 0]);
    net.run(50);
    assert_eq!(net.nodes[n].commands[before..], [Command::StopColorTransition]);
}

#[test]
fn colour_commands_for_a_light_that_is_off_jump_when_told_to_execute_anyway() {
    let (mut net, n, short) = colour_bulb();
    net.coord.zcl(short, 0x0006, 0x11, 0x00, &[]);
    net.run(50);
    let before = net.nodes[n].commands.len();
    net.coord.zcl(short, COLOR, 0x11, 0x06, &[80, 90, 10, 0, 0x01, 0x01]);
    net.coord.zcl(short, COLOR, 0x11, 0x07, &[0x00, 0x40, 0x00, 0x50, 10, 0, 0x01, 0x01]);
    net.coord.zcl(short, COLOR, 0x11, 0x47, &[0x01, 0x01]);
    net.coord.zcl(short, COLOR, 0x11, 0x06, &[81, 91, 10, 0, 0x01, 0x00]);
    net.run(50);
    assert_eq!(
        net.nodes[n].commands[before..],
        [
            Command::MoveToHueSaturation { hue: 80, sat: 90, direction: HueDirection::Shortest, transition_ms: 0 },
            Command::MoveToXy { x: 0x4000, y: 0x5000, transition_ms: 0 }
        ]
    );
}

#[test]
fn hue_saturation_and_temperature_move_and_step_down_as_well_as_up() {
    let (mut net, n, short) = colour_bulb();
    let hs = |hue: u8, sat: u8, direction: HueDirection, transition_ms: u32| Command::MoveToHueSaturation { hue, sat, direction, transition_ms };
    let mut send = |cmd: u8, payload: &[u8]| {
        net.coord.zcl(short, COLOR, 0x11, cmd, payload);
        net.run(50);
        last_command(&net, n)
    };
    assert_eq!(send(0x01, &[3, 10]), Command::MoveHue { up: false, rate: 10 << 8 });
    assert_eq!(send(0x02, &[3, 200, 5]), hs(224, 254, HueDirection::Down, 500));
    assert_eq!(send(0x04, &[3, 127]), hs(224, 0, HueDirection::Shortest, 2_000));
    assert_eq!(send(0x05, &[1, 50, 0]), hs(224, 50, HueDirection::Shortest, 0));
    assert_eq!(send(0x05, &[1, 250, 0]), hs(224, 254, HueDirection::Shortest, 0));
    assert_eq!(send(0x05, &[3, 100, 0]), hs(224, 154, HueDirection::Shortest, 0));
    assert_eq!(send(0x4C, &[3, 100, 0, 10, 0, 0, 0, 0, 0]), Command::MoveToColorTemperature { mireds: 233, transition_ms: 1_000 });
    assert_eq!(send(0x4C, &[3, 200, 0, 10, 0, 0, 0, 0, 0]), Command::MoveToColorTemperature { mireds: 153, transition_ms: 1_000 });
}

#[test]
fn hue_and_saturation_of_254_are_taken_and_255_refused() {
    let (mut net, n, short) = colour_bulb();
    assert_eq!(colour_status(&mut net, short, 0x03, &[254, 0, 0]), 0x00);
    assert_eq!(colour_status(&mut net, short, 0x06, &[254, 254, 0, 0]), 0x00);
    assert_eq!(last_command(&net, n), Command::MoveToHueSaturation { hue: 254, sat: 254, direction: HueDirection::Shortest, transition_ms: 0 });
    assert_eq!(colour_status(&mut net, short, 0x06, &[255, 1, 0, 0]), 0x85);
    assert_eq!(colour_status(&mut net, short, 0x06, &[1, 255, 0, 0]), 0x85);
}

#[test]
fn the_level_takes_the_colour_temperature_along_only_in_colour_temperature_mode() {
    let (mut net, n, short) = colour_bulb();
    net.coord.zcl(short, LEVEL, 0x00, 0x02, &[0x0F, 0x00, 0x18, 0x02]);
    net.run(50);
    net.coord.zcl(short, COLOR, 0x11, 0x06, &[0, 254, 0, 0]);
    net.run(50);
    assert_eq!(net.nodes[n].color_mode, bulb_core::colour::ColorMode::HueSaturation);
    let before = net.nodes[n].commands.len();
    net.coord.zcl(short, LEVEL, 0x11, 0x00, &[100, 0, 0, 0, 0]);
    net.run(50);
    assert_eq!(net.nodes[n].commands[before..], [Command::MoveToLevel { level: 100, transition_ms: 0 }], "ZCL 5.2.2.1.1: coupled only in ColorMode 0x02");
}

#[test]
fn colour_commands_in_one_poll_see_the_colour_the_ones_before_set() {
    let (mut net, n, short) = colour_bulb();
    net.coord.zcl(short, LEVEL, 0x00, 0x02, &[0x0F, 0x00, 0x18, 0x02]);
    net.run(50);
    let mut in_one_poll = |frames: &[(u16, u8, &[u8])]| {
        let before = net.nodes[n].commands.len();
        for &(cluster, cmd, payload) in frames {
            net.coord.zcl(short, cluster, 0x11, cmd, payload);
        }
        net.run(200);
        net.nodes[n].commands[before..].to_vec()
    };
    let level = |level: u8| Command::MoveToLevel { level, transition_ms: 0 };
    let xy = |x: u16, y: u16| Command::MoveToXy { x, y, transition_ms: 0 };
    let hs = |hue: u8, direction: HueDirection| Command::MoveToHueSaturation { hue, sat: 254, direction, transition_ms: 0 };
    assert_eq!(in_one_poll(&[(COLOR, 0x07, &[0, 0x20, 0, 0x30, 0, 0]), (COLOR, 0x09, &[0, 1, 0, 2, 0, 0]), (LEVEL, 0x00, &[100, 0, 0])]),
               vec![xy(0x2000, 0x3000), xy(0x2100, 0x3200), level(100)]);
    assert_eq!(in_one_poll(&[(COLOR, 0x06, &[40, 254, 0, 0]), (COLOR, 0x02, &[1, 10, 0]), (LEVEL, 0x00, &[120, 0, 0])]),
               vec![hs(40, HueDirection::Shortest), hs(50, HueDirection::Up), level(120)]);
    assert_eq!(in_one_poll(&[(COLOR, 0x0A, &[0x2C, 0x01, 0, 0]), (LEVEL, 0x00, &[254, 0, 0])]),
               vec![Command::MoveToColorTemperature { mireds: 300, transition_ms: 0 }, Command::MoveToColorTemperature { mireds: 153, transition_ms: 0 }, level(254)]);
    assert_eq!(in_one_poll(&[(COLOR, 0x01, &[1, 10]), (LEVEL, 0x00, &[200, 0, 0])]), vec![Command::MoveHue { up: true, rate: 10 << 8 }, level(200)]);
}

#[test]
fn a_scene_colour_of_only_x_and_y_is_recalled_as_that_xy() {
    let (mut net, n, short) = colour_bulb();
    net.coord.zcl(short, GROUPS, 0x01, 0x00, &[0x34, 0x12, 0]);
    net.run(50);
    let mut add = vec![0x34, 0x12, 3, 0, 0, 0];
    add.extend_from_slice(&[0x06, 0x00, 1, 1]);
    add.extend_from_slice(&[0x08, 0x00, 1, 200]);
    add.extend_from_slice(&[0x00, 0x03, 4, 0x00, 0x20, 0x00, 0x30]);
    let seq = net.coord.zcl(short, SCENES, 0x01, 0x00, &add);
    net.run(50);
    assert_eq!(net.coord.last_zcl(short, SCENES, 0x00).unwrap(), vec![0x19, seq, 0x00, 0x00, 0x34, 0x12, 3]);
    let before = net.nodes[n].commands.len();
    net.coord.zcl(short, SCENES, 0x01, 0x05, &[0x34, 0x12, 3]);
    net.run(50);
    assert_eq!(
        net.nodes[n].commands[before..],
        [Command::MoveToXy { x: 0x2000, y: 0x3000, transition_ms: 0 }, Command::MoveToLevel { level: 200, transition_ms: 0 }],
        "ZCL 3.7.2.4.2.1: trailing attributes may be left out; zigbee2mqtt scene_add sends x and y alone"
    );
}

#[test]
fn colour_commands_during_a_move_hue_start_from_the_hue_it_has_reached() {
    let (mut net, n, short) = colour_bulb();
    let mut send = |frames: &[(u8, &[u8])], wait: u32| {
        let before = net.nodes[n].commands.len();
        for &(cmd, payload) in frames {
            net.coord.zcl(short, COLOR, 0x11, cmd, payload);
        }
        net.run(wait);
        net.nodes[n].commands[before..].to_vec()
    };
    let hs = |hue: u8, sat: u8, direction: HueDirection| Command::MoveToHueSaturation { hue, sat, direction, transition_ms: 0 };
    send(&[(0x06, &[40, 254, 0, 0]), (0x01, &[1, 10])], 5_000);
    assert_eq!(send(&[(0x03, &[100, 0, 0])], 50), vec![hs(90, 100, HueDirection::Shortest)], "40 and 10 a second for 5 s");
    send(&[(0x00, &[10, 0, 0, 0]), (0x01, &[3, 10])], 5_000);
    let sent = send(&[(0x01, &[0, 0]), (0x02, &[1, 4, 0])], 50);
    assert_eq!(sent, vec![Command::StopColorTransition, hs(220, 100, HueDirection::Up)], "down from 10 past 0 to 216, stopped there");
    send(&[(0x00, &[0, 0, 0, 0]), (0x01, &[3, 1])], 500);
    assert_eq!(send(&[(0x03, &[50, 0, 0])], 50), vec![hs(254, 50, HueDirection::Shortest)], "just below 0 is 254");
}

#[test]
fn a_hue_set_over_ble_during_a_move_hue_is_where_a_step_starts() {
    let (mut net, n, short) = colour_bulb();
    net.coord.zcl(short, COLOR, 0x11, 0x06, &[40, 254, 0, 0]);
    net.run(50);
    net.coord.zcl(short, COLOR, 0x11, 0x01, &[1, 10]);
    net.run(2_000);
    assert!(net.nodes[n].hue_moving);
    net.nodes[n].hue = 100;
    net.nodes[n].hue_moving = false;
    net.nodes[n].report();
    net.run(5_000);
    net.coord.zcl(short, COLOR, 0x11, 0x02, &[1, 10, 0]);
    net.run(50);
    assert_eq!(last_command(&net, n), Command::MoveToHueSaturation { hue: 110, sat: 254, direction: HueDirection::Up, transition_ms: 0 }, "the bulb's report says the hue no longer moves");
}

#[test]
fn a_scene_stored_during_a_move_hue_keeps_the_hue_reached() {
    let (mut net, n, short) = colour_bulb();
    net.coord.zcl(short, GROUPS, 0x01, 0x00, &[0x34, 0x12, 0]);
    net.run(50);
    net.coord.zcl(short, COLOR, 0x11, 0x06, &[40, 254, 0, 0]);
    net.run(50);
    net.coord.zcl(short, COLOR, 0x11, 0x01, &[1, 10]);
    net.run(5_000);
    net.coord.zcl(short, SCENES, 0x01, 0x04, &[0x34, 0x12, 7]);
    net.run(50);
    net.coord.zcl(short, COLOR, 0x11, 0x01, &[0, 0]);
    net.run(50);
    net.coord.zcl(short, COLOR, 0x11, 0x06, &[200, 254, 0, 0]);
    net.run(50);
    net.coord.zcl(short, SCENES, 0x01, 0x05, &[0x34, 0x12, 7]);
    net.run(50);
    let hs = net.nodes[n].commands.iter().rev().find_map(|c| match c {
        Command::MoveToHueSaturation { hue, .. } => Some(*hue),
        _ => None,
    });
    assert!(matches!(hs, Some(89..=91)), "ZCL 8 3.7.2.4.3: Store Scene keeps the current hue, 40 + 10/s for 5 s; got {hs:?}");
}

#[test]
fn a_hue_set_over_ble_with_only_an_identify_queued_still_ends_the_move_hue() {
    let (mut net, n, short) = colour_bulb();
    net.coord.zcl(short, COLOR, 0x11, 0x06, &[40, 254, 0, 0]);
    net.run(50);
    net.coord.zcl(short, COLOR, 0x11, 0x01, &[1, 10]);
    net.run(2_000);
    assert!(net.nodes[n].hue_moving);
    net.coord.zcl(short, LEVEL, 0x11, 0x00, &[100, 0, 0]);
    net.coord.zcl(short, IDENTIFY, 0x11, 0x00, &[2, 0]);
    let mut seen = vec![];
    for _ in 0..200 {
        net.air.borrow_mut().now += 1;
        net.coord.poll();
        let node = &mut net.nodes[n];
        while let Some(c) = node.stack.poll() {
            seen.push(c);
            match c {
                Command::MoveToLevel { level, .. } => {
                    node.level = level;
                    node.report();
                    node.hue = 100;
                    node.hue_moving = false;
                    node.report();
                }
                Command::Identify { .. } => {}
                _ => node.report(),
            }
        }
    }
    assert_eq!(seen, vec![Command::MoveToLevel { level: 100, transition_ms: 0 }, Command::Identify { times: 2 }]);
    net.run(5_000);
    net.coord.zcl(short, COLOR, 0x11, 0x02, &[1, 10, 0]);
    net.run(50);
    assert_eq!(
        last_command(&net, n),
        Command::MoveToHueSaturation { hue: 110, sat: 254, direction: HueDirection::Up, transition_ms: 0 },
        "bulb-core sends no report after an Identify, so the report that said the hue stopped is the last one"
    );
}

#[test]
fn a_move_to_colour_0_0_that_bulb_core_ignores_leaves_the_move_hue_running() {
    let (mut net, n, short) = colour_bulb();
    net.coord.zcl(short, COLOR, 0x11, 0x06, &[40, 254, 0, 0]);
    net.run(50);
    net.coord.zcl(short, COLOR, 0x11, 0x01, &[1, 10]);
    net.run(1_000);
    net.coord.zcl(short, COLOR, 0x11, 0x07, &[0, 0, 0, 0, 0, 0]);
    net.run(50);
    assert!(net.nodes[n].hue_moving);
    net.run(4_000);
    assert_eq!(read(&mut net, short, &[0x0000]), vec![0x00, 0x00, 0x00, 0x20, 90], "CurrentHue, 40 + 10/s for 5 s");
}

fn read_now(net: &mut Net, n: usize, short: u16, ids: &[u16]) -> Vec<u8> {
    let p: Vec<u8> = ids.iter().flat_map(|i| i.to_le_bytes()).collect();
    let seq = net.coord.zcl(short, COLOR, 0x00, 0x00, &p);
    net.air.borrow_mut().now += 1;
    assert_eq!(net.nodes[n].stack.poll(), None);
    net.run(50);
    let rsp = net.coord.last_zcl(short, COLOR, 0x01).unwrap();
    assert_eq!(&rsp[..3], &[0x18, seq, 0x01]);
    rsp[3..].to_vec()
}

#[test]
fn a_move_to_colour_0_0_still_queued_leaves_the_colour_and_its_mode_as_they_were() {
    let (mut net, n, short) = colour_bulb();
    net.coord.zcl(short, COLOR, 0x11, 0x07, &[0x00, 0x30, 0x00, 0x20, 0, 0]);
    net.run(50);
    net.coord.zcl(short, COLOR, 0x11, 0x06, &[40, 254, 0, 0]);
    net.run(50);
    let before = read(&mut net, short, &[0x0003, 0x0004, 0x0008]);
    net.coord.zcl(short, COLOR, 0x11, 0x07, &[0, 0, 0, 0, 0, 0]);
    net.air.borrow_mut().now += 1;
    assert_eq!(net.nodes[n].stack.poll(), Some(Command::MoveToXy { x: 0, y: 0, transition_ms: 0 }));
    assert_eq!(read_now(&mut net, n, short, &[0x0003, 0x0004, 0x0008]), before);
}

#[test]
fn a_move_to_colour_the_bulb_has_not_reported_yet_ends_the_move_hue() {
    let (mut net, n, short) = colour_bulb();
    net.coord.zcl(short, COLOR, 0x11, 0x06, &[40, 254, 0, 0]);
    net.run(50);
    net.coord.zcl(short, COLOR, 0x11, 0x01, &[1, 10]);
    net.run(1_000);
    net.coord.zcl(short, COLOR, 0x11, 0x07, &[0x00, 0x30, 0x00, 0x20, 0, 0]);
    net.air.borrow_mut().now += 1;
    assert_eq!(net.nodes[n].stack.poll(), Some(Command::MoveToXy { x: 0x3000, y: 0x2000, transition_ms: 0 }));
    net.air.borrow_mut().now += 3_000;
    let hue = read_now(&mut net, n, short, &[0x0000]);
    assert!(matches!(hue[..], [0x00, 0x00, 0x00, 0x20, 49..=51]), "ZCL 8 5.2.2.3: a new colour stops the hue at 40 + 10/s for 1 s; got {hue:?}");
}
