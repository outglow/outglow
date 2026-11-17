use bulb_core::command::{Command, HueDirection};
use crate::sim::*;

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
    assert!(matches!(last_command(&net, n), Command::MoveToHueSaturation { direction: HueDirection::Up, transition_ms: 1000, .. }));
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
