use bulb_core::command::Command;
use crate::sim::*;

const IDENTIFY: u16 = 0x0003;
const GROUPS: u16 = 0x0004;
const SCENES: u16 = 0x0005;
const ON_OFF: u16 = 0x0006;
const LEVEL: u16 = 0x0008;
const COLOR: u16 = 0x0300;

fn in_group() -> (Net, usize, u16) {
    let mut net = Net::new();
    let n = net.join_node();
    let short = net.nodes[n].short();
    net.coord.zcl(short, GROUPS, 0x01, 0x00, &[0x34, 0x12, 0]);
    net.run(50);
    (net, n, short)
}

fn level(net: &mut Net, short: u16, level: u8) {
    net.coord.zcl(short, LEVEL, 0x11, 0x04, &[level, 0, 0, 0, 0]);
    net.run(50);
}

#[test]
fn a_stored_scene_is_recalled() {
    let (mut net, n, short) = in_group();
    level(&mut net, short, 200);
    let seq = net.coord.zcl(short, SCENES, 0x01, 0x04, &[0x34, 0x12, 7]);
    net.run(50);
    assert_eq!(net.coord.last_zcl(short, SCENES, 0x04).unwrap(), vec![0x19, seq, 0x04, 0x00, 0x34, 0x12, 7]);
    level(&mut net, short, 20);
    net.coord.zcl(short, ON_OFF, 0x11, 0x00, &[]);
    net.run(50);
    assert!(!net.nodes[n].on);
    let seq = net.coord.zcl(short, SCENES, 0x01, 0x05, &[0x34, 0x12, 7]);
    net.run(50);
    assert_eq!(net.nodes[n].commands.last(), Some(&Command::MoveToLevel { level: 200, transition_ms: 0 }));
    assert!(net.nodes[n].on);
    assert_eq!(net.coord.last_zcl(short, SCENES, 0x0B).unwrap(), vec![0x18, seq, 0x0B, 0x05, 0x00]);
    let seq = net.coord.zcl(short, SCENES, 0x00, 0x00, &[0, 0, 1, 0, 2, 0, 3, 0]);
    net.run(50);
    assert_eq!(
        net.coord.last_zcl(short, SCENES, 0x01).unwrap(),
        vec![0x18, seq, 0x01, 0, 0, 0, 0x20, 1, 1, 0, 0, 0x20, 7, 2, 0, 0, 0x21, 0x34, 0x12, 3, 0, 0, 0x10, 1]
    );
    level(&mut net, short, 90);
    let seq = net.coord.zcl(short, SCENES, 0x00, 0x00, &[3, 0]);
    net.run(50);
    assert_eq!(net.coord.last_zcl(short, SCENES, 0x01).unwrap(), vec![0x18, seq, 0x01, 3, 0, 0, 0x10, 0]);
    let seq = net.coord.zcl(short, SCENES, 0x01, 0x05, &[0x34, 0x12, 9]);
    net.run(50);
    assert_eq!(net.coord.last_zcl(short, SCENES, 0x0B).unwrap(), vec![0x18, seq, 0x0B, 0x05, 0x8B]);
}

#[test]
fn scenes_are_added_viewed_listed_and_removed() {
    let (mut net, n, short) = in_group();
    let mut add = vec![0x34, 0x12, 3, 2, 0, 0];
    add.extend_from_slice(&[0x06, 0x00, 1, 0x00, 0x08, 0x00, 1, 55]);
    let seq = net.coord.zcl(short, SCENES, 0x01, 0x00, &add);
    net.run(50);
    assert_eq!(net.coord.last_zcl(short, SCENES, 0x00).unwrap(), vec![0x19, seq, 0x00, 0x00, 0x34, 0x12, 3]);
    let seq = net.coord.zcl(short, SCENES, 0x01, 0x01, &[0x34, 0x12, 3]);
    net.run(50);
    assert_eq!(
        net.coord.last_zcl(short, SCENES, 0x01).unwrap(),
        vec![0x19, seq, 0x01, 0x00, 0x34, 0x12, 3, 2, 0, 0, 0x06, 0x00, 1, 0, 0x08, 0x00, 1, 55]
    );
    let before = net.nodes[n].commands.len();
    net.coord.zcl(short, SCENES, 0x11, 0x05, &[0x34, 0x12, 3]);
    net.run(50);
    assert_eq!(&net.nodes[n].commands[before..], &[Command::Off, Command::SetLevelWhileOff(55)]);
    let seq = net.coord.zcl(short, SCENES, 0x01, 0x06, &[0x34, 0x12]);
    net.run(50);
    assert_eq!(net.coord.last_zcl(short, SCENES, 0x06).unwrap(), vec![0x19, seq, 0x06, 0x00, 7, 0x34, 0x12, 1, 3]);
    let seq = net.coord.zcl(short, SCENES, 0x01, 0x04, &[0x99, 0x99, 1]);
    net.run(50);
    assert_eq!(net.coord.last_zcl(short, SCENES, 0x04).unwrap(), vec![0x19, seq, 0x04, 0x85, 0x99, 0x99, 1]);
    let seq = net.coord.zcl(short, SCENES, 0x01, 0x02, &[0x34, 0x12, 3]);
    net.run(50);
    assert_eq!(net.coord.last_zcl(short, SCENES, 0x02).unwrap(), vec![0x19, seq, 0x02, 0x00, 0x34, 0x12, 3]);
    let seq = net.coord.zcl(short, SCENES, 0x01, 0x01, &[0x34, 0x12, 3]);
    net.run(50);
    assert_eq!(net.coord.last_zcl(short, SCENES, 0x01).unwrap(), vec![0x19, seq, 0x01, 0x8B, 0x34, 0x12, 3]);
}

#[test]
fn scenes_survive_a_power_cycle_and_go_with_their_group() {
    let (mut net, n, short) = in_group();
    level(&mut net, short, 123);
    net.coord.zcl(short, SCENES, 0x11, 0x04, &[0x34, 0x12, 1]);
    net.run(100);
    net.power_cycle(n);
    net.run(1_000);
    level(&mut net, short, 10);
    net.coord.zcl_group(0x1234, SCENES, 0x05, &[0x34, 0x12, 1]);
    net.run(300);
    assert_eq!(net.nodes[n].level, 123);
    net.coord.zcl(short, GROUPS, 0x11, 0x03, &[0x34, 0x12]);
    net.run(50);
    net.coord.zcl(short, GROUPS, 0x11, 0x00, &[0x34, 0x12, 0]);
    net.run(50);
    let seq = net.coord.zcl(short, SCENES, 0x01, 0x06, &[0x34, 0x12]);
    net.run(50);
    assert_eq!(net.coord.last_zcl(short, SCENES, 0x06).unwrap(), vec![0x19, seq, 0x06, 0x00, 8, 0x34, 0x12, 0]);
}

fn tunable() -> (Net, usize, u16) {
    let mut net = Net::new();
    net.identity = IDENTITY_TW;
    let n = net.join_node();
    let short = net.nodes[n].short();
    (net, n, short)
}

#[test]
fn the_tunable_white_bulb_is_a_color_temperature_light() {
    let (mut net, _n, short) = tunable();
    let s = short.to_le_bytes();
    let seq = net.coord.zdo(short, 0x0004, &[s[0], s[1], 1]);
    net.run(50);
    assert_eq!(
        net.coord.last_zdo(short, 0x8004).unwrap(),
        vec![
            seq, 0x00, s[0], s[1], 28, 1, 0x04, 0x01, 0x0C, 0x01, 1, 9, 0x00, 0x00, 0x03, 0x00, 0x04, 0x00, 0x05, 0x00,
            0x06, 0x00, 0x08, 0x00, 0x00, 0x03, 0x05, 0x0B, 0x00, 0x10, 1, 0x19, 0x00
        ]
    );
    let seq = net.coord.zcl(short, COLOR, 0x00, 0x00, &[0x07, 0x00, 0x08, 0x00, 0x0A, 0x40, 0x0B, 0x40, 0x0C, 0x40]);
    net.run(50);
    assert_eq!(
        net.coord.last_zcl(short, COLOR, 0x01).unwrap(),
        vec![
            0x18, seq, 0x01, 0x07, 0x00, 0, 0x21, 0x4D, 0x01, 0x08, 0x00, 0, 0x30, 2, 0x0A, 0x40, 0, 0x19, 0x10, 0x00,
            0x0B, 0x40, 0, 0x21, 153, 0, 0x0C, 0x40, 0, 0x21, 0xC6, 0x01
        ]
    );
}

#[test]
fn color_temperature_commands() {
    let (mut net, n, short) = tunable();
    let seq = net.coord.zcl(short, COLOR, 0x01, 0x0A, &[250, 0, 10, 0]);
    net.run(50);
    assert_eq!(net.nodes[n].commands.last(), Some(&Command::MoveToColorTemperature { mireds: 250, transition_ms: 1000 }));
    assert_eq!(net.coord.last_zcl(short, COLOR, 0x0B).unwrap(), vec![0x18, seq, 0x0B, 0x0A, 0x00]);
    net.coord.zcl(short, COLOR, 0x11, 0x0A, &[50, 0, 0, 0]);
    net.run(50);
    assert_eq!(net.nodes[n].color_temp, 153);
    net.coord.zcl(short, COLOR, 0x11, 0x4C, &[1, 100, 0, 20, 0, 0, 0, 200, 0]);
    net.run(2_050);
    assert_eq!(net.nodes[n].commands.last(), Some(&Command::MoveToColorTemperature { mireds: 200, transition_ms: 2000 }));
    net.coord.zcl(short, COLOR, 0x11, 0x4B, &[1, 50, 0, 0, 0, 0, 0]);
    net.run(50);
    assert_eq!(net.nodes[n].commands.last(), Some(&Command::MoveToColorTemperature { mireds: 454, transition_ms: 5080 }));
    net.coord.zcl(short, COLOR, 0x11, 0x47, &[]);
    net.run(50);
    assert_eq!(net.nodes[n].commands.last(), Some(&Command::StopColorTransition));
    net.coord.zcl(short, COLOR, 0x11, 0x0A, &[250, 0, 50, 0]);
    net.coord.zcl(short, COLOR, 0x11, 0x4B, &[0, 0, 0, 0, 0, 0, 0]);
    net.run(50);
    assert_eq!(net.nodes[n].commands.last(), Some(&Command::StopColorTransition));
    assert_eq!(read(&mut net, short, COLOR, 0x0002), vec![0x00, 0x21, 0, 0], "RemainingTime");
    net.coord.zcl(short, ON_OFF, 0x11, 0x00, &[]);
    net.run(50);
    let seq = net.coord.zcl(short, COLOR, 0x01, 0x0A, &[100, 1, 20, 0]);
    net.run(50);
    assert_eq!(net.nodes[n].commands.last(), Some(&Command::MoveToColorTemperature { mireds: 356, transition_ms: 0 }));
    assert_eq!(net.coord.last_zcl(short, COLOR, 0x0B).unwrap(), vec![0x18, seq, 0x0B, 0x0A, 0x00]);
    net.coord.zcl(short, COLOR, 0x10, 0x02, &[0x0F, 0x00, 0x18, 0x00]);
    net.run(50);
    let before = net.nodes[n].commands.len();
    let seq = net.coord.zcl(short, COLOR, 0x01, 0x0A, &[200, 0, 0, 0]);
    net.run(50);
    assert_eq!(net.nodes[n].commands.len(), before);
    assert_eq!(net.coord.last_zcl(short, COLOR, 0x0B).unwrap(), vec![0x18, seq, 0x0B, 0x0A, 0x00]);
    let seq = net.coord.zcl(short, COLOR, 0x00, 0x02, &[0x10, 0x40, 0x21, 0x2C, 0x01]);
    net.run(50);
    assert_eq!(net.coord.last_zcl(short, COLOR, 0x04).unwrap(), vec![0x18, seq, 0x04, 0x00]);
    assert!(net.nodes[n].commands.contains(&Command::SetStartupColorTemperature(300)));
}

#[test]
fn color_temperature_is_reported_and_stored_in_scenes() {
    let (mut net, n, short) = tunable();
    let ext = net.air.borrow().ports[net.nodes[n].id].ext;
    let mut bind = ext.to_le_bytes().to_vec();
    bind.push(1);
    bind.extend_from_slice(&COLOR.to_le_bytes());
    bind.push(3);
    bind.extend_from_slice(&COORD_EXT.to_le_bytes());
    bind.push(1);
    net.coord.zdo(short, 0x0021, &bind);
    net.run(2_000);
    let seq = net.coord.zcl(short, COLOR, 0x00, 0x06, &[0x00, 0x07, 0x00, 0x21, 0, 0, 60, 0, 10, 0]);
    net.run(50);
    assert_eq!(net.coord.last_zcl(short, COLOR, 0x07).unwrap(), vec![0x18, seq, 0x07, 0x00]);
    let reports = |net: &Net| net.coord.zcl_rx.iter().filter(|e| e.1 == COLOR && e.2[2] == 0x0A).count();
    let before = reports(&net);
    net.coord.zcl(short, COLOR, 0x11, 0x0A, &[0x50, 0x01, 0, 0]);
    net.run(2_000);
    assert_eq!(reports(&net), before);
    net.coord.zcl(short, COLOR, 0x11, 0x0A, &[200, 0, 0, 0]);
    net.run(2_000);
    assert_eq!(reports(&net), before + 1);
    assert_eq!(&net.coord.last_zcl(short, COLOR, 0x0A).unwrap()[2..], &[0x0A, 0x07, 0x00, 0x21, 200, 0]);
    net.coord.zcl(short, SCENES, 0x11, 0x04, &[0, 0, 5]);
    net.run(50);
    net.coord.zcl(short, COLOR, 0x11, 0x0A, &[0x90, 0x01, 0, 0]);
    net.run(50);
    net.coord.zcl(short, SCENES, 0x11, 0x05, &[0, 0, 5]);
    net.run(50);
    assert_eq!(net.nodes[n].color_temp, 200);
    let seq = net.coord.zcl(short, SCENES, 0x01, 0x01, &[0, 0, 5]);
    net.run(50);
    let view = net.coord.last_zcl(short, SCENES, 0x01).unwrap();
    assert_eq!(&view[..3], &[0x19, seq, 0x01]);
    assert_eq!(&view[view.len() - 16..], &[0x00, 0x03, 13, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 200, 0]);
}

#[test]
fn the_dimmable_bulb_has_no_color_control() {
    let mut net = Net::new();
    let n = net.join_node();
    let short = net.nodes[n].short();
    let seq = net.coord.zcl(short, COLOR, 0x01, 0x0A, &[250, 0, 0, 0]);
    net.run(50);
    assert_eq!(net.coord.last_zcl(short, COLOR, 0x0B).unwrap(), vec![0x18, seq, 0x0B, 0x0A, 0xC3]);
    assert!(net.nodes[n].commands.iter().all(|c| !matches!(c, Command::MoveToColorTemperature { .. })));
}

#[test]
fn identify_starts_and_stops_the_blinking() {
    let mut net = Net::new();
    let n = net.join_node();
    let short = net.nodes[n].short();
    net.coord.zcl(short, IDENTIFY, 0x11, 0x00, &[5, 0]);
    net.run(50);
    assert_eq!(net.nodes[n].commands.last(), Some(&Command::Identify { times: 5 }));
    net.run(2_000);
    let seq = net.coord.zcl(short, IDENTIFY, 0x01, 0x01, &[]);
    net.run(50);
    assert_eq!(net.coord.last_zcl(short, IDENTIFY, 0x00).unwrap(), vec![0x19, seq, 0x00, 2, 0]);
    net.coord.zcl(short, IDENTIFY, 0x11, 0x00, &[0, 0]);
    net.run(50);
    assert_eq!(net.nodes[n].commands.last(), Some(&Command::Identify { times: 0 }));
}

#[test]
fn commands_for_a_light_that_is_off_follow_the_options() {
    let (mut net, n, short) = tunable();
    level(&mut net, short, 100);
    net.coord.zcl(short, ON_OFF, 0x11, 0x00, &[]);
    net.run(50);
    let count = |net: &Net| net.nodes[n].commands.len();
    let before = count(&net);
    net.coord.zcl(short, LEVEL, 0x11, 0x00, &[50, 0, 0]);
    net.coord.zcl(short, LEVEL, 0x11, 0x00, &[50, 0, 0, 0, 0]);
    net.run(100);
    assert_eq!(count(&net), before);
    net.coord.zcl(short, LEVEL, 0x11, 0x00, &[50, 0, 0, 0x01, 0x01]);
    net.run(50);
    assert_eq!(net.nodes[n].commands.last(), Some(&Command::SetLevelWhileOff(50)));
    assert!(!net.nodes[n].on, "the light stays off");
    let seq = net.coord.zcl(short, LEVEL, 0x00, 0x00, &[0x00, 0x00]);
    net.run(50);
    assert_eq!(net.coord.last_zcl(short, LEVEL, 0x01).unwrap(), vec![0x18, seq, 0x01, 0x00, 0x00, 0x00, 0x20, 50]);
    let seq = net.coord.zcl(short, LEVEL, 0x00, 0x02, &[0x0F, 0x00, 0x18, 0x04]);
    net.run(50);
    assert_eq!(net.coord.last_zcl(short, LEVEL, 0x04).unwrap(), vec![0x18, seq, 0x04, 0x87, 0x0F, 0x00]);
    let seq = net.coord.zcl(short, LEVEL, 0x00, 0x02, &[0x0F, 0x00, 0x18, 0x01]);
    net.run(50);
    assert_eq!(net.coord.last_zcl(short, LEVEL, 0x04).unwrap(), vec![0x18, seq, 0x04, 0x00]);
    net.coord.zcl(short, LEVEL, 0x11, 0x00, &[70, 0, 0]);
    net.run(50);
    assert_eq!(net.nodes[n].commands.last(), Some(&Command::SetLevelWhileOff(70)));
    net.coord.zcl(short, LEVEL, 0x11, 0x02, &[0, 20, 0, 0, 0, 0]);
    net.run(50);
    assert_eq!(net.nodes[n].commands.last(), Some(&Command::SetLevelWhileOff(90)));
    let before = count(&net);
    net.coord.zcl(short, LEVEL, 0x11, 0x00, &[30, 0, 0, 0x01, 0x00]);
    net.run(50);
    assert_eq!(count(&net), before);
    net.coord.zcl(short, COLOR, 0x10, 0x02, &[0x0F, 0x00, 0x18, 0x00]);
    net.coord.zcl(short, COLOR, 0x11, 0x0A, &[200, 0, 0, 0, 0, 0]);
    net.run(50);
    assert_eq!(count(&net), before);
    net.coord.zcl(short, COLOR, 0x11, 0x0A, &[200, 0, 30, 0, 0x01, 0x01]);
    net.run(50);
    assert_eq!(net.nodes[n].commands.last(), Some(&Command::MoveToColorTemperature { mireds: 200, transition_ms: 0 }));
    let seq = net.coord.zcl(short, COLOR, 0x00, 0x02, &[0x0F, 0x00, 0x18, 0x01]);
    net.run(50);
    assert_eq!(net.coord.last_zcl(short, COLOR, 0x04).unwrap(), vec![0x18, seq, 0x04, 0x00]);
    net.coord.zcl(short, COLOR, 0x11, 0x0A, &[0x2C, 0x01, 0, 0]);
    net.run(50);
    assert_eq!(net.nodes[n].commands.last(), Some(&Command::MoveToColorTemperature { mireds: 300, transition_ms: 0 }));
    net.run(11_000);
    net.power_cycle(n);
    net.run(1_000);
    for cluster in [LEVEL, COLOR] {
        let seq = net.coord.zcl(short, cluster, 0x00, 0x00, &[0x0F, 0x00]);
        net.run(50);
        assert_eq!(net.coord.last_zcl(short, cluster, 0x01).unwrap(), vec![0x18, seq, 0x01, 0x0F, 0x00, 0x00, 0x18, 0x01]);
    }
}

fn read(net: &mut Net, short: u16, cluster: u16, id: u16) -> Vec<u8> {
    let i = id.to_le_bytes();
    let seq = net.coord.zcl(short, cluster, 0x00, 0x00, &i);
    net.run(50);
    let rsp = net.coord.last_zcl(short, cluster, 0x01).unwrap();
    assert_eq!(&rsp[..5], &[0x18, seq, 0x01, i[0], i[1]]);
    rsp[5..].to_vec()
}

#[test]
fn off_with_effect_keeps_the_look_for_on_with_recall_global_scene() {
    let (mut net, n, short) = tunable();
    level(&mut net, short, 100);
    net.coord.zcl(short, COLOR, 0x11, 0x0A, &[250, 0, 0, 0]);
    net.run(50);
    assert_eq!(read(&mut net, short, ON_OFF, 0x4000), vec![0x00, 0x10, 1], "GlobalSceneControl");
    net.coord.zcl(short, ON_OFF, 0x11, 0x40, &[0x00, 0x00]);
    net.run(50);
    assert_eq!(net.nodes[n].commands.last(), Some(&Command::FadeOff { transition_ms: 800 }));
    assert!(!net.nodes[n].on);
    assert_eq!(read(&mut net, short, ON_OFF, 0x4000), vec![0x00, 0x10, 0]);
    net.coord.zcl(short, ON_OFF, 0x11, 0x40, &[0x00, 0x00]);
    net.run(50);
    let before = net.nodes[n].commands.len();
    net.coord.zcl(short, ON_OFF, 0x11, 0x41, &[]);
    net.run(50);
    assert_eq!(
        &net.nodes[n].commands[before..],
        &[Command::MoveToColorTemperature { mireds: 250, transition_ms: 0 }, Command::MoveToLevel { level: 100, transition_ms: 0 }]
    );
    assert_eq!(read(&mut net, short, ON_OFF, 0x4000), vec![0x00, 0x10, 1]);
    net.coord.zcl(short, ON_OFF, 0x11, 0x40, &[0x00, 0x00]);
    net.run(50);
    assert_eq!(read(&mut net, short, ON_OFF, 0x4000), vec![0x00, 0x10, 0]);
    level(&mut net, short, 20);
    assert_eq!(read(&mut net, short, ON_OFF, 0x4000), vec![0x00, 0x10, 1], "set by Move to Level (with On/Off)");
    net.coord.zcl(short, COLOR, 0x11, 0x0A, &[0x90, 0x01, 0, 0]);
    net.run(50);
    net.coord.zcl(short, ON_OFF, 0x11, 0x40, &[0x00, 0x00]);
    net.run(50);
    assert_eq!(read(&mut net, short, ON_OFF, 0x4000), vec![0x00, 0x10, 0]);
    let before = net.nodes[n].commands.len();
    net.coord.zcl(short, ON_OFF, 0x11, 0x41, &[]);
    net.run(50);
    assert_eq!(
        &net.nodes[n].commands[before..],
        &[Command::MoveToColorTemperature { mireds: 400, transition_ms: 0 }, Command::MoveToLevel { level: 20, transition_ms: 0 }]
    );
    assert_eq!(read(&mut net, short, ON_OFF, 0x4000), vec![0x00, 0x10, 1]);
    let before = net.nodes[n].commands.len();
    net.coord.zcl(short, ON_OFF, 0x11, 0x41, &[]);
    net.run(50);
    assert_eq!(net.nodes[n].commands.len(), before);
}

#[test]
fn on_with_timed_off_turns_the_light_off_again_and_guards_the_off_state() {
    let mut net = Net::new();
    let n = net.join_node();
    let short = net.nodes[n].short();
    net.coord.zcl(short, ON_OFF, 0x11, 0x00, &[]);
    net.run(50);
    let before = net.nodes[n].commands.len();
    net.coord.zcl(short, ON_OFF, 0x11, 0x42, &[0x01, 30, 0, 50, 0]);
    net.run(50);
    assert_eq!(net.nodes[n].commands.len(), before);
    net.coord.zcl(short, ON_OFF, 0x11, 0x42, &[0x00, 30, 0, 50, 0]);
    net.run(50);
    assert_eq!(net.nodes[n].commands.last(), Some(&Command::On));
    net.run(950);
    let left = u16::from_le_bytes(read(&mut net, short, ON_OFF, 0x4001)[2..4].try_into().unwrap());
    assert!((19..=21).contains(&left), "OnTime counts down in tenths of a second: {left}");
    net.coord.zcl(short, ON_OFF, 0x11, 0x42, &[0x00, 5, 0, 50, 0]);
    net.run(50);
    let left = u16::from_le_bytes(read(&mut net, short, ON_OFF, 0x4001)[2..4].try_into().unwrap());
    assert!((18..=20).contains(&left), "{left}");
    net.run(2_500);
    assert_eq!(net.nodes[n].commands.last(), Some(&Command::Off), "time is up");
    assert!(!net.nodes[n].on);
    assert_eq!(read(&mut net, short, ON_OFF, 0x4002), vec![0x00, 0x21, 0, 0], "it ran out by itself: no guard time");
    net.coord.zcl(short, ON_OFF, 0x11, 0x42, &[0x00, 100, 0, 50, 0]);
    net.run(500);
    net.coord.zcl(short, ON_OFF, 0x11, 0x00, &[]);
    net.run(1_000);
    let before = net.nodes[n].commands.len();
    net.coord.zcl(short, ON_OFF, 0x11, 0x42, &[0x00, 100, 0, 50, 0]);
    net.run(100);
    assert_eq!(net.nodes[n].commands.len(), before, "a motion sensor does not turn it back on");
    net.run(5_000);
    net.coord.zcl(short, ON_OFF, 0x11, 0x42, &[0x00, 10, 0, 0, 0]);
    net.run(100);
    assert_eq!(net.nodes[n].commands.last(), Some(&Command::On));
}

#[test]
fn scenes_take_transition_times_in_tenths_and_can_be_copied() {
    let (mut net, n, short) = in_group();
    net.coord.zcl(short, GROUPS, 0x01, 0x00, &[0x78, 0x56, 0]);
    net.run(50);
    let seq = net.coord.zcl(short, SCENES, 0x01, 0x40, &[0x34, 0x12, 3, 25, 0, 0, 0x06, 0x00, 1, 1, 0x08, 0x00, 1, 60]);
    net.run(50);
    assert_eq!(net.coord.last_zcl(short, SCENES, 0x40).unwrap(), vec![0x19, seq, 0x40, 0x00, 0x34, 0x12, 3]);
    let seq = net.coord.zcl(short, SCENES, 0x01, 0x01, &[0x34, 0x12, 3]);
    net.run(50);
    assert_eq!(&net.coord.last_zcl(short, SCENES, 0x01).unwrap()[..10], &[0x19, seq, 0x01, 0x00, 0x34, 0x12, 3, 2, 0, 0]);
    let seq = net.coord.zcl(short, SCENES, 0x01, 0x41, &[0x34, 0x12, 3]);
    net.run(50);
    assert_eq!(
        net.coord.last_zcl(short, SCENES, 0x41).unwrap(),
        vec![0x19, seq, 0x41, 0x00, 0x34, 0x12, 3, 25, 0, 0, 0x06, 0x00, 1, 1, 0x08, 0x00, 1, 60]
    );
    net.coord.zcl(short, SCENES, 0x11, 0x05, &[0x34, 0x12, 3]);
    net.run(50);
    assert_eq!(net.nodes[n].commands.last(), Some(&Command::MoveToLevel { level: 60, transition_ms: 2_500 }));
    let seq = net.coord.zcl(short, SCENES, 0x01, 0x42, &[0x00, 0x34, 0x12, 3, 0x78, 0x56, 9]);
    net.run(50);
    assert_eq!(net.coord.last_zcl(short, SCENES, 0x42).unwrap(), vec![0x19, seq, 0x42, 0x00, 0x34, 0x12, 3]);
    net.coord.zcl(short, SCENES, 0x11, 0x05, &[0x78, 0x56, 9, 0xFF, 0xFF]);
    net.run(50);
    assert_eq!(net.nodes[n].commands.last(), Some(&Command::MoveToLevel { level: 60, transition_ms: 2_500 }));
    net.coord.zcl(short, SCENES, 0x11, 0x00, &[0x34, 0x12, 4, 0, 0, 0, 0x08, 0x00, 1, 70]);
    let seq = net.coord.zcl(short, SCENES, 0x01, 0x42, &[0x01, 0x34, 0x12, 0, 0x78, 0x56, 0]);
    net.run(50);
    assert_eq!(net.coord.last_zcl(short, SCENES, 0x42).unwrap(), vec![0x19, seq, 0x42, 0x00, 0x34, 0x12, 0]);
    let seq = net.coord.zcl(short, SCENES, 0x01, 0x06, &[0x78, 0x56]);
    net.run(50);
    let members = net.coord.last_zcl(short, SCENES, 0x06).unwrap();
    assert_eq!(&members[..7], &[0x19, seq, 0x06, 0x00, 3, 0x78, 0x56]);
    let mut ids = members[8..].to_vec();
    ids.sort();
    assert_eq!((members[7], ids), (3, vec![3, 4, 9]));
    let seq = net.coord.zcl(short, SCENES, 0x01, 0x42, &[0x00, 0x34, 0x12, 77, 0x78, 0x56, 1]);
    net.run(50);
    assert_eq!(net.coord.last_zcl(short, SCENES, 0x42).unwrap(), vec![0x19, seq, 0x42, 0x8B, 0x34, 0x12, 77]);
    let seq = net.coord.zcl(short, SCENES, 0x01, 0x42, &[0x00, 0x34, 0x12, 3, 0x99, 0x99, 1]);
    net.run(50);
    assert_eq!(net.coord.last_zcl(short, SCENES, 0x42).unwrap(), vec![0x19, seq, 0x42, 0x85, 0x34, 0x12, 3]);
    net.run(11_000);
    net.power_cycle(n);
    net.run(1_000);
    let seq = net.coord.zcl(short, SCENES, 0x01, 0x41, &[0x78, 0x56, 9]);
    net.run(50);
    assert_eq!(&net.coord.last_zcl(short, SCENES, 0x41).unwrap()[..9], &[0x19, seq, 0x41, 0x00, 0x78, 0x56, 9, 25, 0]);
}

#[test]
fn commands_sent_to_a_group_or_to_everyone_get_no_cluster_response() {
    let (mut net, n, short) = in_group();
    let from_bulb = |net: &Net| net.coord.zcl_rx.iter().filter(|f| f.0 == short).count();
    let before = from_bulb(&net);
    net.coord.zcl(0xFFFD, GROUPS, 0x01, 0x00, &[0x78, 0x56, 0]);
    net.coord.zcl(0xFFFD, SCENES, 0x01, 0x04, &[0x34, 0x12, 5]);
    net.coord.zcl_group(0x1234, SCENES, 0x01, &[0x34, 0x12, 5]);
    net.run(500);
    assert_eq!(from_bulb(&net), before);
    let seq = net.coord.zcl(short, SCENES, 0x01, 0x06, &[0x34, 0x12]);
    net.run(50);
    assert_eq!(net.coord.last_zcl(short, SCENES, 0x06).unwrap(), vec![0x19, seq, 0x06, 0x00, 7, 0x34, 0x12, 1, 5]);
    let _ = n;
}

#[test]
fn commands_and_attribute_access_can_be_discovered() {
    let (mut net, _, short) = tunable();
    let seq = net.coord.zcl(short, ON_OFF, 0x00, 0x11, &[0x00, 20]);
    net.run(50);
    assert_eq!(net.coord.last_zcl(short, ON_OFF, 0x12).unwrap(), vec![0x18, seq, 0x12, 1, 0x00, 0x01, 0x02, 0x40, 0x41, 0x42]);
    let seq = net.coord.zcl(short, ON_OFF, 0x00, 0x11, &[0x02, 2]);
    net.run(50);
    assert_eq!(net.coord.last_zcl(short, ON_OFF, 0x12).unwrap(), vec![0x18, seq, 0x12, 0, 0x02, 0x40]);
    let seq = net.coord.zcl(short, GROUPS, 0x00, 0x13, &[0x00, 20]);
    net.run(50);
    assert_eq!(net.coord.last_zcl(short, GROUPS, 0x14).unwrap(), vec![0x18, seq, 0x14, 1, 0x00, 0x01, 0x02, 0x03]);
    let seq = net.coord.zcl(short, ON_OFF, 0x00, 0x13, &[0x00, 20]);
    net.run(50);
    assert_eq!(net.coord.last_zcl(short, ON_OFF, 0x14).unwrap(), vec![0x18, seq, 0x14, 1]);
    let seq = net.coord.zcl(short, LEVEL, 0x00, 0x15, &[0x00, 0x00, 20]);
    net.run(50);
    assert_eq!(
        net.coord.last_zcl(short, LEVEL, 0x16).unwrap(),
        vec![
            0x18, seq, 0x16, 1, 0x00, 0x00, 0x20, 0x05, 0x01, 0x00, 0x21, 0x01, 0x02, 0x00, 0x20, 0x01, 0x03, 0x00, 0x20, 0x01, 0x0F, 0x00,
            0x18, 0x03, 0x11, 0x00, 0x20, 0x03, 0x00, 0x40, 0x20, 0x03, 0xFD, 0xFF, 0x21, 0x01
        ]
    );
    for (cluster, payload) in
        [(0x0000u16, vec![]), (IDENTIFY, vec![0; 4]), (GROUPS, vec![0; 4]), (SCENES, vec![0; 8]), (ON_OFF, vec![0; 6]), (LEVEL, vec![1; 8]), (COLOR, vec![1; 12]), (0x1000, vec![0])]
    {
        let seq = net.coord.zcl(short, cluster, 0x00, 0x11, &[0x00, 40]);
        net.run(50);
        let listed = net.coord.last_zcl(short, cluster, 0x12).unwrap();
        assert_eq!(&listed[..4], &[0x18, seq, 0x12, 1]);
        for cmd in &listed[4..] {
            let seq = net.coord.zcl(short, cluster, 0x01, *cmd, &payload);
            net.run(50);
            let unsupported = net.coord.last_zcl(short, cluster, 0x0B).is_some_and(|r| r[1] == seq && r[4] == 0x81);
            assert!(!unsupported, "cluster {cluster:#06x} command {cmd:#04x}");
        }
        let seq = net.coord.zcl(short, cluster, 0x01, 0x7E, &payload);
        net.run(50);
        assert_eq!(net.coord.last_zcl(short, cluster, 0x0B).unwrap(), vec![0x18, seq, 0x0B, 0x7E, 0x81]);
    }
}

#[test]
fn on_level_remaining_time_and_reset_to_factory_defaults() {
    let (mut net, n, short) = tunable();
    level(&mut net, short, 200);
    let seq = net.coord.zcl(short, LEVEL, 0x00, 0x02, &[0x11, 0x00, 0x20, 40]);
    net.run(50);
    assert_eq!(net.coord.last_zcl(short, LEVEL, 0x04).unwrap(), vec![0x18, seq, 0x04, 0x00]);
    net.coord.zcl(short, ON_OFF, 0x11, 0x00, &[]);
    net.coord.zcl(short, ON_OFF, 0x11, 0x01, &[]);
    net.run(50);
    assert_eq!(net.nodes[n].commands.last(), Some(&Command::MoveToLevel { level: 40, transition_ms: 0 }));
    net.run(11_000);
    net.power_cycle(n);
    net.run(1_000);
    assert_eq!(read(&mut net, short, LEVEL, 0x0011), vec![0x00, 0x20, 40], "kept over a power cut");
    net.coord.zcl(short, LEVEL, 0x11, 0x04, &[200, 30, 0, 0, 0]);
    net.coord.zcl(short, COLOR, 0x11, 0x0A, &[250, 0, 50, 0]);
    net.run(950);
    let left = |v: Vec<u8>| u16::from_le_bytes([v[2], v[3]]);
    let l = left(read(&mut net, short, LEVEL, 0x0001));
    assert!((19..=21).contains(&l), "{l}");
    let c = left(read(&mut net, short, COLOR, 0x0002));
    assert!((38..=41).contains(&c), "{c}");
    net.coord.zcl(short, LEVEL, 0x11, 0x03, &[0, 0]);
    net.run(50);
    assert_eq!(left(read(&mut net, short, LEVEL, 0x0001)), 0, "stopped");
    net.run(5_000);
    assert_eq!(left(read(&mut net, short, COLOR, 0x0002)), 0);
    net.coord.zcl(short, GROUPS, 0x11, 0x00, &[0x34, 0x12, 0]);
    net.coord.zcl(short, LEVEL, 0x10, 0x02, &[0x0F, 0x00, 0x18, 0x01]);
    net.coord.zcl(short, ON_OFF, 0x10, 0x02, &[0x03, 0x40, 0x30, 0x00]);
    net.run(100);
    assert!(net.nodes[n].commands.contains(&Command::SetPowerOnBehavior(bulb_core::command::PowerOnBehavior::Off)));
    assert_eq!(read(&mut net, short, LEVEL, 0x000F), vec![0x00, 0x18, 0x01]);
    let seq = net.coord.zcl(short, 0x0000, 0x01, 0x00, &[]);
    net.run(100);
    assert_eq!(net.coord.last_zcl(short, 0x0000, 0x0B).unwrap(), vec![0x18, seq, 0x0B, 0x00, 0x00]);
    assert_eq!(read(&mut net, short, LEVEL, 0x0011), vec![0x00, 0x20, 0xFF]);
    assert_eq!(read(&mut net, short, LEVEL, 0x000F), vec![0x00, 0x18, 0x00]);
    assert_eq!(read(&mut net, short, ON_OFF, 0x4003), vec![0x00, 0x30, 0xFF]);
    assert!(net.nodes[n].commands.contains(&Command::SetPowerOnBehavior(bulb_core::command::PowerOnBehavior::Previous)));
    let seq = net.coord.zcl(short, GROUPS, 0x01, 0x02, &[0]);
    net.run(50);
    assert_eq!(net.coord.last_zcl(short, GROUPS, 0x02).unwrap(), vec![0x19, seq, 0x02, 7, 1, 0x34, 0x12]);
}

#[test]
fn off_with_effect_fades_as_the_variant_says() {
    let mut net = Net::new();
    let n = net.join_node();
    let short = net.nodes[n].short();
    for (effect, variant, want) in [
        (0u8, 1u8, Command::Off),
        (0, 2, Command::FadeOff { transition_ms: 12_800 }),
        (1, 0, Command::FadeOff { transition_ms: 1_500 }),
    ] {
        level(&mut net, short, 200);
        net.coord.zcl(short, ON_OFF, 0x11, 0x40, &[effect, variant]);
        net.run(50);
        assert_eq!(net.nodes[n].commands.last(), Some(&want), "effect {effect}, variant {variant}");
        assert!(!net.nodes[n].on);
    }
    let before = net.nodes[n].commands.len();
    net.coord.zcl(short, ON_OFF, 0x11, 0x40, &[0, 0]);
    net.run(50);
    assert_eq!(&net.nodes[n].commands[before..], &[Command::Off]);
}

#[test]
fn the_colour_temperature_can_follow_the_level() {
    let (mut net, n, short) = tunable();
    let seq = net.coord.zcl(short, LEVEL, 0x00, 0x02, &[0x0F, 0x00, 0x18, 0x02]);
    net.run(50);
    assert_eq!(net.coord.last_zcl(short, LEVEL, 0x04).unwrap(), vec![0x18, seq, 0x04, 0x00]);
    let last_two = |net: &Net| net.nodes[n].commands[net.nodes[n].commands.len() - 2..].to_vec();
    net.coord.zcl(short, LEVEL, 0x11, 0x04, &[254, 10, 0, 0, 0]);
    net.run(50);
    assert_eq!(
        last_two(&net),
        vec![Command::MoveToColorTemperature { mireds: 153, transition_ms: 1_000 }, Command::MoveToLevel { level: 254, transition_ms: 1_000 }]
    );
    net.coord.zcl(short, LEVEL, 0x11, 0x00, &[1, 0, 0, 0, 0]);
    net.run(50);
    assert_eq!(last_two(&net)[0], Command::MoveToColorTemperature { mireds: 454, transition_ms: 0 });
    net.coord.zcl(short, LEVEL, 0x11, 0x00, &[30, 0, 0, 0, 0]);
    net.run(50);
    assert_eq!(last_two(&net)[0], Command::MoveToColorTemperature { mireds: 420, transition_ms: 0 });
    net.coord.zcl(short, LEVEL, 0x10, 0x02, &[0x0F, 0x00, 0x18, 0x00]);
    net.coord.zcl(short, LEVEL, 0x11, 0x00, &[99, 0, 0, 0, 0]);
    net.run(50);
    assert_eq!(last_two(&net)[0], Command::MoveToLevel { level: 30, transition_ms: 0 });
    let mut net = Net::new();
    let n = net.join_node();
    let short = net.nodes[n].short();
    let seq = net.coord.zcl(short, LEVEL, 0x00, 0x02, &[0x0F, 0x00, 0x18, 0x02]);
    net.run(50);
    assert_eq!(net.coord.last_zcl(short, LEVEL, 0x04).unwrap(), vec![0x18, seq, 0x04, 0x87, 0x0F, 0x00]);
}

#[test]
fn the_endpoint_shows_its_limits_its_signal_and_that_it_is_a_touchlink_target() {
    let mut net = Net::new();
    let n = net.join_node();
    let short = net.nodes[n].short();
    assert_eq!(read(&mut net, short, LEVEL, 0x0002), vec![0x00, 0x20, 1]);
    assert_eq!(read(&mut net, short, LEVEL, 0x0003), vec![0x00, 0x20, 254]);
    assert_eq!(read(&mut net, short, 0x0000, 0x0012), vec![0x00, 0x10, 1]);
    assert_eq!(read(&mut net, short, 0x0B05, 0x011C), vec![0x00, 0x20, 255]);
    assert_eq!(read(&mut net, short, 0x0B05, 0x011D), vec![0x00, 0x28, (-15i8) as u8]);
    let (pc, pn) = (0, net.nodes[n].id);
    net.link(pc, pn, 100);
    assert_eq!(read(&mut net, short, 0x0B05, 0x011C), vec![0x00, 0x20, 100]);
    assert_eq!(read(&mut net, short, 0x0B05, 0x011D), vec![0x00, 0x28, (-67i8) as u8]);
    assert_eq!(read(&mut net, short, 0x1000, 0xFFFD), vec![0x00, 0x21, 3, 0]);
    let seq = net.coord.zcl(short, 0x1000, 0x01, 0x41, &[0]);
    net.run(50);
    assert_eq!(net.coord.last_zcl(short, 0x1000, 0x41).unwrap(), vec![0x19, seq, 0x41, 0, 0, 0]);
    let seq = net.coord.zcl(short, 0x1000, 0x01, 0x42, &[2]);
    net.run(50);
    assert_eq!(net.coord.last_zcl(short, 0x1000, 0x42).unwrap(), vec![0x19, seq, 0x42, 0, 2, 0]);
}

fn tenths(net: &mut Net, short: u16, id: u16) -> u16 {
    let v = read(net, short, ON_OFF, id);
    u16::from_le_bytes([v[2], v[3]])
}

#[test]
fn every_command_that_turns_the_light_on_sets_global_scene_control() {
    let (mut net, _, short) = in_group();
    let off_with_effect = |net: &mut Net| {
        net.coord.zcl(short, ON_OFF, 0x11, 0x40, &[0x00, 0x01]);
        net.run(50);
        assert_eq!(read(net, short, ON_OFF, 0x4000), vec![0x00, 0x10, 0]);
    };
    level(&mut net, short, 80);
    net.coord.zcl(short, SCENES, 0x11, 0x04, &[0x34, 0x12, 2]);
    net.run(50);
    off_with_effect(&mut net);
    net.coord.zcl(short, SCENES, 0x11, 0x05, &[0x34, 0x12, 2]);
    net.run(50);
    assert_eq!(read(&mut net, short, ON_OFF, 0x4000), vec![0x00, 0x10, 1], "Recall Scene");
    off_with_effect(&mut net);
    net.coord.zcl(short, LEVEL, 0x11, 0x06, &[0, 40, 0, 0]);
    net.run(50);
    assert_eq!(read(&mut net, short, ON_OFF, 0x4000), vec![0x00, 0x10, 1], "Step (with On/Off)");
    off_with_effect(&mut net);
    net.coord.zcl(short, LEVEL, 0x11, 0x05, &[0, 50]);
    net.run(50);
    assert_eq!(read(&mut net, short, ON_OFF, 0x4000), vec![0x00, 0x10, 1], "Move (with On/Off)");
    off_with_effect(&mut net);
    net.coord.zcl(short, ON_OFF, 0x11, 0x42, &[0x00, 100, 0, 0, 0]);
    net.run(50);
    assert_eq!(read(&mut net, short, ON_OFF, 0x4000), vec![0x00, 0x10, 1], "On with Timed Off");
    off_with_effect(&mut net);
    net.coord.zcl(short, LEVEL, 0x11, 0x00, &[50, 0, 0, 0x01, 0x01]);
    net.coord.zcl(short, SCENES, 0x11, 0x00, &[0x34, 0x12, 9, 0, 0, 0, 0x06, 0x00, 1, 0]);
    net.run(50);
    net.coord.zcl(short, SCENES, 0x11, 0x05, &[0x34, 0x12, 9]);
    net.coord.zcl(short, LEVEL, 0x11, 0x04, &[0, 0, 0]);
    net.run(50);
    assert_eq!(read(&mut net, short, ON_OFF, 0x4000), vec![0x00, 0x10, 0]);
}

#[test]
fn level_commands_and_scenes_that_switch_the_light_end_the_timed_on_period_or_the_guard() {
    let (mut net, n, short) = in_group();
    net.coord.zcl(short, SCENES, 0x11, 0x00, &[0x34, 0x12, 1, 0, 0, 0, 0x06, 0x00, 1, 0]);
    net.coord.zcl(short, SCENES, 0x11, 0x00, &[0x34, 0x12, 2, 0, 0, 0, 0x06, 0x00, 1, 1]);
    net.run(50);
    net.coord.zcl(short, ON_OFF, 0x11, 0x42, &[0x00, 0x2C, 0x01, 0, 0]);
    net.run(500);
    assert!(tenths(&mut net, short, 0x4001) > 280);
    net.coord.zcl(short, LEVEL, 0x11, 0x04, &[0, 5, 0]);
    net.run(50);
    assert!(net.nodes[n].on, "on until the level reaches the minimum");
    assert_eq!(tenths(&mut net, short, 0x4001), 0, "OnTime after Move to Level (with On/Off) 0");
    net.run(500);
    assert!(!net.nodes[n].on);
    net.coord.zcl(short, ON_OFF, 0x11, 0x01, &[]);
    net.run(40_000);
    assert!(net.nodes[n].on);
    for (cluster, cmd, payload) in [(LEVEL, 0x06, vec![1, 254, 0, 0]), (SCENES, 0x05, vec![0x34, 0x12, 1])] {
        net.coord.zcl(short, ON_OFF, 0x11, 0x42, &[0x00, 0x2C, 0x01, 0, 0]);
        net.run(500);
        net.coord.zcl(short, cluster, 0x11, cmd, &payload);
        net.run(50);
        assert!(!net.nodes[n].on, "cluster {cluster:#06x}");
        assert_eq!(tenths(&mut net, short, 0x4001), 0, "OnTime, cluster {cluster:#06x}");
    }
    for (cluster, cmd, payload) in [(LEVEL, 0x04, vec![100, 0, 0]), (LEVEL, 0x06, vec![0, 40, 0, 0]), (SCENES, 0x05, vec![0x34, 0x12, 2])] {
        net.coord.zcl(short, ON_OFF, 0x11, 0x42, &[0x00, 100, 0, 0x2C, 0x01]);
        net.run(500);
        net.coord.zcl(short, ON_OFF, 0x11, 0x00, &[]);
        net.run(1_000);
        let guard = tenths(&mut net, short, 0x4002);
        assert!((280..300).contains(&guard), "OffWaitTime counts down: {guard}");
        net.coord.zcl(short, cluster, 0x11, cmd, &payload);
        net.run(50);
        assert!(net.nodes[n].on);
        assert_eq!(tenths(&mut net, short, 0x4002), 0, "OffWaitTime, cluster {cluster:#06x} command {cmd:#04x}");
    }
    net.coord.zcl(short, ON_OFF, 0x11, 0x42, &[0x00, 100, 0, 50, 0]);
    net.run(500);
    net.coord.zcl(short, LEVEL, 0x11, 0x04, &[60, 0, 0]);
    net.run(50);
    assert_eq!(tenths(&mut net, short, 0x4002), 50);
}

#[test]
fn with_on_off_the_lowest_level_is_off_and_a_step_cut_short_takes_less_time() {
    let mut net = Net::new();
    let n = net.join_node();
    let short = net.nodes[n].short();
    level(&mut net, short, 100);
    net.coord.zcl(short, LEVEL, 0x11, 0x04, &[1, 10, 0]);
    net.run(50);
    assert_eq!(net.nodes[n].commands.last(), Some(&Command::MoveToLevel { level: 0, transition_ms: 1_000 }));
    assert_eq!(read(&mut net, short, ON_OFF, 0x0000), vec![0x00, 0x10, 1], "ZCL 8 3.10.2.3.6: on until the level reaches the minimum");
    net.run(1_000);
    assert!(!net.nodes[n].on);
    assert_eq!(read(&mut net, short, ON_OFF, 0x0000), vec![0x00, 0x10, 0]);
    net.coord.zcl(short, LEVEL, 0x11, 0x04, &[2, 0, 0]);
    net.run(50);
    assert_eq!(net.nodes[n].commands.last(), Some(&Command::MoveToLevel { level: 2, transition_ms: 0 }));
    level(&mut net, short, 100);
    net.coord.zcl(short, LEVEL, 0x11, 0x00, &[1, 0, 0]);
    net.run(50);
    assert_eq!(net.nodes[n].commands.last(), Some(&Command::MoveToLevel { level: 1, transition_ms: 0 }));
    assert!(net.nodes[n].on);
    level(&mut net, short, 100);
    net.coord.zcl(short, LEVEL, 0x11, 0x06, &[1, 99, 0, 0]);
    net.run(50);
    assert_eq!(net.nodes[n].commands.last(), Some(&Command::MoveToLevel { level: 0, transition_ms: 0 }));
    level(&mut net, short, 204);
    net.coord.zcl(short, LEVEL, 0x11, 0x02, &[0, 100, 40, 0]);
    net.run(50);
    assert_eq!(net.nodes[n].commands.last(), Some(&Command::MoveToLevel { level: 254, transition_ms: 2_000 }));
    level(&mut net, short, 51);
    net.coord.zcl(short, LEVEL, 0x11, 0x02, &[1, 200, 40, 0]);
    net.run(1_050);
    assert_eq!(net.nodes[n].commands.last(), Some(&Command::MoveToLevel { level: 1, transition_ms: 1_000 }));
    net.coord.zcl(short, LEVEL, 0x11, 0x02, &[0, 30, 40, 0]);
    net.run(50);
    assert_eq!(net.nodes[n].commands.last(), Some(&Command::MoveToLevel { level: 31, transition_ms: 4_000 }));
}

#[test]
fn a_scene_changes_only_the_clusters_it_has_field_sets_for() {
    let (mut net, n, short) = tunable();
    net.coord.zcl(short, GROUPS, 0x11, 0x00, &[0x34, 0x12, 0]);
    net.coord.zcl(short, COLOR, 0x11, 0x0A, &[250, 0, 0, 0]);
    net.run(50);
    net.coord.zcl(short, ON_OFF, 0x11, 0x00, &[]);
    net.run(50);
    let mut color = vec![0x34, 0x12, 3, 0, 0, 0, 0x00, 0x03, 13];
    color.extend_from_slice(&[0; 11]);
    color.extend_from_slice(&[0x90, 0x01]);
    net.coord.zcl(short, SCENES, 0x11, 0x00, &[0x34, 0x12, 1, 0, 0, 0, 0x08, 0x00, 1, 60]);
    net.coord.zcl(short, SCENES, 0x11, 0x00, &[0x34, 0x12, 2, 0, 0, 0, 0x06, 0x00, 1, 1]);
    net.coord.zcl(short, SCENES, 0x11, 0x00, &color);
    net.run(50);
    net.coord.zcl(short, SCENES, 0x11, 0x00, &[0x34, 0x12, 4, 0, 0, 0]);
    net.run(50);
    let view = |net: &mut Net, id: u8| {
        let seq = net.coord.zcl(short, SCENES, 0x01, 0x01, &[0x34, 0x12, id]);
        net.run(50);
        let v = net.coord.last_zcl(short, SCENES, 0x01).unwrap();
        assert_eq!(&v[..10], &[0x19, seq, 0x01, 0x00, 0x34, 0x12, id, 0, 0, 0]);
        v[10..].to_vec()
    };
    assert_eq!(view(&mut net, 1), vec![0x08, 0x00, 1, 60]);
    assert_eq!(view(&mut net, 2), vec![0x06, 0x00, 1, 1]);
    assert_eq!(view(&mut net, 3), vec![0x00, 0x03, 13, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0x90, 0x01]);
    assert_eq!(view(&mut net, 4), Vec::<u8>::new());
    level(&mut net, short, 200);
    let before = net.nodes[n].commands.len();
    net.coord.zcl(short, SCENES, 0x11, 0x05, &[0x34, 0x12, 1]);
    net.run(50);
    assert_eq!(&net.nodes[n].commands[before..], &[Command::MoveToLevel { level: 60, transition_ms: 0 }]);
    net.coord.zcl(short, ON_OFF, 0x11, 0x00, &[]);
    level(&mut net, short, 200);
    net.coord.zcl(short, ON_OFF, 0x11, 0x00, &[]);
    net.run(50);
    let before = net.nodes[n].commands.len();
    net.coord.zcl(short, SCENES, 0x11, 0x05, &[0x34, 0x12, 1]);
    net.run(50);
    assert_eq!(&net.nodes[n].commands[before..], &[Command::SetLevelWhileOff(60)]);
    assert!(!net.nodes[n].on);
    level(&mut net, short, 90);
    net.coord.zcl(short, ON_OFF, 0x11, 0x00, &[]);
    net.run(50);
    let before = net.nodes[n].commands.len();
    net.coord.zcl(short, SCENES, 0x11, 0x05, &[0x34, 0x12, 2]);
    net.run(50);
    assert_eq!(&net.nodes[n].commands[before..], &[Command::On]);
    assert_eq!((net.nodes[n].on, net.nodes[n].level, net.nodes[n].color_temp), (true, 90, 250));
    let before = net.nodes[n].commands.len();
    net.coord.zcl(short, SCENES, 0x11, 0x05, &[0x34, 0x12, 3, 20, 0]);
    net.run(50);
    assert_eq!(&net.nodes[n].commands[before..], &[Command::MoveToColorTemperature { mireds: 400, transition_ms: 2_000 }]);
    let before = net.nodes[n].commands.len();
    let seq = net.coord.zcl(short, SCENES, 0x01, 0x05, &[0x34, 0x12, 4]);
    net.run(50);
    assert_eq!(net.nodes[n].commands.len(), before);
    assert_eq!(net.coord.last_zcl(short, SCENES, 0x0B).unwrap(), vec![0x18, seq, 0x0B, 0x05, 0x00]);
    net.run(2_000);
    net.coord.zcl(short, SCENES, 0x11, 0x04, &[0x34, 0x12, 4]);
    net.run(50);
    assert_eq!(view(&mut net, 4), vec![0x06, 0x00, 1, 1, 0x08, 0x00, 1, 90, 0x00, 0x03, 13, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0x90, 0x01]);
    net.coord.zcl(short, SCENES, 0x11, 0x42, &[0x00, 0x34, 0x12, 1, 0x34, 0x12, 7]);
    net.run(11_000);
    net.power_cycle(n);
    net.run(1_000);
    assert_eq!(view(&mut net, 7), vec![0x08, 0x00, 1, 60]);
    assert_eq!(view(&mut net, 2), vec![0x06, 0x00, 1, 1]);
    assert_eq!(view(&mut net, 4).len(), 24);
}

#[test]
fn who_is_identifying_in_a_group_or_has_scenes_is_answered_however_it_is_asked() {
    let (mut net, _, short) = in_group();
    let answers = |net: &Net, cluster: u16, cmd: u8| net.coord.zcl_rx.iter().filter(|f| f.0 == short && f.1 == cluster && f.2[0] == 0x19 && f.2[2] == cmd).count();
    net.coord.zcl(0xFFFF, IDENTIFY, 0x01, 0x01, &[]);
    net.run(500);
    assert_eq!(answers(&net, IDENTIFY, 0x00), 0);
    net.coord.zcl(short, IDENTIFY, 0x11, 0x00, &[180, 0]);
    net.run(1_100);
    let seq = net.coord.zcl(0xFFFF, IDENTIFY, 0x01, 0x01, &[]);
    net.run(500);
    assert_eq!(answers(&net, IDENTIFY, 0x00), 1);
    assert_eq!(net.coord.last_zcl(short, IDENTIFY, 0x00).unwrap(), vec![0x19, seq, 0x00, 178, 0]);
    let seq = net.coord.zcl(0xFFFD, GROUPS, 0x01, 0x02, &[0]);
    net.run(500);
    assert_eq!(net.coord.last_zcl(short, GROUPS, 0x02).unwrap(), vec![0x19, seq, 0x02, 7, 1, 0x34, 0x12]);
    let seq = net.coord.zcl(0xFFFD, GROUPS, 0x01, 0x02, &[2, 0x99, 0x99, 0x34, 0x12]);
    net.run(500);
    assert_eq!(net.coord.last_zcl(short, GROUPS, 0x02).unwrap(), vec![0x19, seq, 0x02, 7, 1, 0x34, 0x12]);
    let before = answers(&net, GROUPS, 0x02);
    net.coord.zcl(0xFFFD, GROUPS, 0x01, 0x02, &[1, 0x99, 0x99]);
    net.run(500);
    assert_eq!(answers(&net, GROUPS, 0x02), before);
    let seq = net.coord.zcl(short, GROUPS, 0x01, 0x02, &[1, 0x99, 0x99]);
    net.run(50);
    assert_eq!(net.coord.last_zcl(short, GROUPS, 0x02).unwrap(), vec![0x19, seq, 0x02, 7, 0]);
    net.coord.zcl_group(0x1234, SCENES, 0x06, &[0x34, 0x12]);
    net.coord.zcl(0xFFFD, SCENES, 0x01, 0x06, &[0x99, 0x99]);
    net.run(500);
    assert_eq!(answers(&net, SCENES, 0x06), 0);
    net.coord.zcl(short, SCENES, 0x11, 0x04, &[0x34, 0x12, 5]);
    net.run(50);
    net.coord.zcl_group(0x1234, SCENES, 0x06, &[0x34, 0x12]);
    net.run(500);
    assert_eq!(answers(&net, SCENES, 0x06), 1);
    assert_eq!(&net.coord.last_zcl(short, SCENES, 0x06).unwrap()[3..], &[0x00, 7, 0x34, 0x12, 1, 5]);
    let seq = net.coord.zcl(short, SCENES, 0x01, 0x06, &[0, 0]);
    net.run(50);
    assert_eq!(net.coord.last_zcl(short, SCENES, 0x06).unwrap(), vec![0x19, seq, 0x06, 0x00, 7, 0, 0, 0]);
}

#[test]
fn what_the_bulb_does_not_have_is_unsup_command_and_a_group_twice_is_no_error() {
    let (mut net, n, short) = in_group();
    level(&mut net, short, 100);
    let seq = net.coord.zcl(short, ON_OFF, 0x00, 0x7F, &[]);
    net.run(50);
    assert_eq!(net.coord.last_zcl(short, ON_OFF, 0x0B).unwrap(), vec![0x18, seq, 0x0B, 0x7F, 0x81]);
    for frame_control in [0x05, 0x04] {
        let before = net.nodes[n].commands.len();
        let code_low = net.coord.zcl(short, ON_OFF, frame_control, 0x1A, &[0x77, 0x00]);
        net.run(50);
        assert_eq!(net.nodes[n].commands.len(), before);
        let answer = net.coord.zcl_rx.last().unwrap();
        assert_eq!((answer.0, answer.1), (short, ON_OFF));
        assert_eq!(answer.2, vec![0x1C, code_low, 0x1A, 0x77, 0x0B, 0x00, 0x81]);
    }
    let seq = net.coord.zcl(short, GROUPS, 0x01, 0x00, &[0x34, 0x12, 0]);
    net.run(50);
    assert_eq!(net.coord.last_zcl(short, GROUPS, 0x00).unwrap(), vec![0x19, seq, 0x00, 0x00, 0x34, 0x12]);
    let seq = net.coord.zcl(short, GROUPS, 0x01, 0x02, &[0]);
    net.run(50);
    assert_eq!(net.coord.last_zcl(short, GROUPS, 0x02).unwrap(), vec![0x19, seq, 0x02, 7, 1, 0x34, 0x12]);
}

#[test]
fn group_identifiers_out_of_range_are_invalid_values_and_unknown_groups_are_told_apart() {
    let (mut net, n, short) = in_group();
    for (group, status) in [(0x0000u16, 0x87), (0xFFF8, 0x87), (0x5555, 0x8B), (0x1234, 0x00)] {
        let g = group.to_le_bytes();
        let seq = net.coord.zcl(short, GROUPS, 0x01, 0x01, &g);
        net.run(50);
        assert_eq!(net.coord.last_zcl(short, GROUPS, 0x01).unwrap(), vec![0x19, seq, 0x01, status, g[0], g[1], 0]);
    }
    for (group, status) in [(0x0000u16, 0x87), (0xFFF8, 0x87), (0x5555, 0x8B), (0x1234, 0x00), (0x1234, 0x8B)] {
        let g = group.to_le_bytes();
        let seq = net.coord.zcl(short, GROUPS, 0x01, 0x03, &g);
        net.run(50);
        assert_eq!(net.coord.last_zcl(short, GROUPS, 0x03).unwrap(), vec![0x19, seq, 0x03, status, g[0], g[1]]);
    }
    net.coord.zcl(short, GROUPS, 0x11, 0x00, &[0x34, 0x12, 0]);
    net.run(50);
    for (cmd, payload) in [
        (0x00u8, vec![3, 0, 0, 0]),
        (0x01, vec![3]),
        (0x02, vec![3]),
        (0x03, vec![]),
        (0x04, vec![3]),
        (0x06, vec![]),
        (0x40, vec![3, 0, 0, 0]),
        (0x41, vec![3]),
    ] {
        for (group, status) in [(0xFFF8u16, 0x87), (0x5555, 0x85)] {
            let mut p = group.to_le_bytes().to_vec();
            p.extend_from_slice(&payload);
            let seq = net.coord.zcl(short, SCENES, 0x01, cmd, &p);
            net.run(50);
            assert_eq!(&net.coord.last_zcl(short, SCENES, cmd).unwrap()[..4], &[0x19, seq, cmd, status], "command {cmd:#04x}, group {group:#06x}");
        }
    }
    for (from, to, status) in [(0xFFF8u16, 0x1234u16, 0x87), (0x1234, 0xFFFF, 0x87), (0x5555, 0xFFF8, 0x87), (0x5555, 0x1234, 0x85), (0x1234, 0x5555, 0x85)] {
        let (f, t) = (from.to_le_bytes(), to.to_le_bytes());
        let seq = net.coord.zcl(short, SCENES, 0x01, 0x42, &[0x00, f[0], f[1], 1, t[0], t[1], 1]);
        net.run(50);
        assert_eq!(net.coord.last_zcl(short, SCENES, 0x42).unwrap(), vec![0x19, seq, 0x42, status, f[0], f[1], 1], "{from:#06x} to {to:#06x}");
    }
    for (group, status) in [(0xFFF8u16, 0x87), (0x5555, 0x85), (0x1234, 0x8B)] {
        let g = group.to_le_bytes();
        let seq = net.coord.zcl(short, SCENES, 0x01, 0x05, &[g[0], g[1], 3]);
        net.run(50);
        assert_eq!(net.coord.last_zcl(short, SCENES, 0x0B).unwrap(), vec![0x18, seq, 0x0B, 0x05, status]);
    }
    let seq = net.coord.zcl(short, GROUPS, 0x01, 0x05, &[0x00, 0x00, 0]);
    net.run(50);
    assert_eq!(net.coord.last_zcl(short, GROUPS, 0x0B).unwrap(), vec![0x18, seq, 0x0B, 0x05, 0x00]);
    net.coord.zcl(short, IDENTIFY, 0x11, 0x00, &[60, 0]);
    let seq = net.coord.zcl(short, GROUPS, 0x01, 0x05, &[0x00, 0x00, 0]);
    net.run(50);
    assert_eq!(net.coord.last_zcl(short, GROUPS, 0x0B).unwrap(), vec![0x18, seq, 0x0B, 0x05, 0x87]);
    let seq = net.coord.zcl(short, GROUPS, 0x01, 0x05, &[0x78, 0x56, 0]);
    net.run(50);
    assert_eq!(net.coord.last_zcl(short, GROUPS, 0x0B).unwrap(), vec![0x18, seq, 0x0B, 0x05, 0x00]);
    let seq = net.coord.zcl(short, GROUPS, 0x01, 0x02, &[0]);
    net.run(50);
    assert_eq!(net.coord.last_zcl(short, GROUPS, 0x02).unwrap(), vec![0x19, seq, 0x02, 6, 2, 0x34, 0x12, 0x78, 0x56]);
    let _ = n;
}

#[test]
fn a_cluster_command_cut_short_gets_a_default_response_not_half_a_response() {
    let (mut net, _, short) = in_group();
    for (cluster, cmd, payload) in [(SCENES, 0x01u8, vec![0x34]), (SCENES, 0x01, vec![0x34, 0x12]), (GROUPS, 0x02, vec![2, 0x34, 0x12])] {
        let seq = net.coord.zcl(short, cluster, 0x01, cmd, &payload);
        net.run(50);
        let answer = net.coord.zcl_rx.last().unwrap();
        assert_eq!((answer.0, answer.1, answer.2.clone()), (short, cluster, vec![0x18, seq, 0x0B, cmd, 0x80]));
    }
}

#[test]
fn the_basic_cluster_says_which_zcl_this_is_and_has_the_attributes_a_light_must_have() {
    let (mut net, _, short) = tunable();
    assert_eq!(read(&mut net, short, 0x0000, 0x0000), vec![0x00, 0x20, 8]);
    assert_eq!(read(&mut net, short, 0x0000, 0x0006), vec![0x00, 0x42, 0]);
    assert_eq!(read(&mut net, short, 0x0000, 0x0008), vec![0x00, 0x30, 0x00]);
    assert_eq!(read(&mut net, short, 0x0000, 0x0009), vec![0x00, 0x30, 0x05]);
    assert_eq!(read(&mut net, short, 0x0000, 0x000A), vec![0x00, 0x41, 0]);
    assert_eq!(read(&mut net, short, 0x0000, 0x000B), vec![0x00, 0x42, 0]);
    let seq = net.coord.zcl(short, 0x0000, 0x00, 0x02, &[0x08, 0x00, 0x30, 0x01]);
    net.run(50);
    assert_eq!(net.coord.last_zcl(short, 0x0000, 0x04).unwrap(), vec![0x18, seq, 0x04, 0x88, 0x08, 0x00]);
    let seq = net.coord.zcl(short, 0x0000, 0x00, 0x0C, &[0x05, 0x00, 7]);
    net.run(50);
    assert_eq!(
        net.coord.last_zcl(short, 0x0000, 0x0D).unwrap(),
        vec![0x18, seq, 0x0D, 0, 0x05, 0x00, 0x42, 0x06, 0x00, 0x42, 0x07, 0x00, 0x30, 0x08, 0x00, 0x30, 0x09, 0x00, 0x30, 0x0A, 0x00, 0x41, 0x0B, 0x00, 0x42]
    );
    for (cluster, revision) in
        [(0x0000u16, 3u8), (IDENTIFY, 2), (GROUPS, 3), (SCENES, 3), (ON_OFF, 2), (LEVEL, 2), (COLOR, 3), (0x0B05, 3), (0x1000, 3)]
    {
        assert_eq!(read(&mut net, short, cluster, 0xFFFD), vec![0x00, 0x21, revision, 0], "cluster {cluster:#06x}");
    }
}

#[test]
fn a_reporting_interval_with_the_maximum_below_the_minimum_is_refused() {
    let mut net = Net::new();
    let n = net.join_node();
    let short = net.nodes[n].short();
    let config = |net: &mut Net| {
        let seq = net.coord.zcl(short, LEVEL, 0x00, 0x08, &[0x00, 0x00, 0x00]);
        net.run(50);
        let v = net.coord.last_zcl(short, LEVEL, 0x09).unwrap();
        assert_eq!(&v[..8], &[0x18, seq, 0x09, 0x00, 0x00, 0x00, 0x00, 0x20]);
        v[8..].to_vec()
    };
    let seq = net.coord.zcl(short, LEVEL, 0x00, 0x06, &[0x00, 0x00, 0x00, 0x20, 5, 0, 120, 0, 3]);
    net.run(50);
    assert_eq!(net.coord.last_zcl(short, LEVEL, 0x07).unwrap(), vec![0x18, seq, 0x07, 0x00]);
    assert_eq!(config(&mut net), vec![5, 0, 120, 0, 3]);
    let seq = net.coord.zcl(short, LEVEL, 0x00, 0x06, &[0x00, 0x00, 0x00, 0x20, 60, 0, 10, 0, 1]);
    net.run(50);
    assert_eq!(net.coord.last_zcl(short, LEVEL, 0x07).unwrap(), vec![0x18, seq, 0x07, 0x87, 0x00, 0x00, 0x00]);
    assert_eq!(config(&mut net), vec![5, 0, 120, 0, 3]);
    for (min, max) in [(60u16, 60u16), (60, 0), (60, 0xFFFF)] {
        let (a, b) = (min.to_le_bytes(), max.to_le_bytes());
        let seq = net.coord.zcl(short, LEVEL, 0x00, 0x06, &[0x00, 0x00, 0x00, 0x20, a[0], a[1], b[0], b[1], 1]);
        net.run(50);
        assert_eq!(net.coord.last_zcl(short, LEVEL, 0x07).unwrap(), vec![0x18, seq, 0x07, 0x00]);
        assert_eq!(config(&mut net), vec![a[0], a[1], b[0], b[1], 1]);
    }
    let seq = net.coord.zcl(short, LEVEL, 0x00, 0x06, &[0x00, 0x00, 0x00, 0x20, 0xFF, 0xFF, 0, 0, 9]);
    net.run(50);
    assert_eq!(net.coord.last_zcl(short, LEVEL, 0x07).unwrap(), vec![0x18, seq, 0x07, 0x00]);
    assert_eq!(config(&mut net), vec![1, 0, 0x10, 0x0E, 1]);
}

#[test]
fn write_attributes_undivided_writes_all_records_or_none() {
    let mut net = Net::new();
    let n = net.join_node();
    let short = net.nodes[n].short();
    let records = [0x11, 0x00, 0x20, 40, 0x00, 0x0F, 0x20, 1];
    let seq = net.coord.zcl(short, LEVEL, 0x00, 0x03, &records);
    net.run(50);
    assert_eq!(net.coord.last_zcl(short, LEVEL, 0x04).unwrap(), vec![0x18, seq, 0x04, 0x86, 0x00, 0x0F]);
    assert_eq!(read(&mut net, short, LEVEL, 0x0011), vec![0x00, 0x20, 0xFF], "OnLevel untouched");
    for (records, status) in [
        (vec![0x00, 0x0F, 0x20, 1, 0x11, 0x00, 0x20, 40], vec![0x86, 0x00, 0x0F]),
        (vec![0x11, 0x00, 0x20, 40, 0x0F, 0x00, 0x18, 0x80], vec![0x87, 0x0F, 0x00]),
        (vec![0x11, 0x00, 0x20, 40, 0x00, 0x00, 0x20, 9], vec![0x88, 0x00, 0x00]),
    ] {
        let seq = net.coord.zcl(short, LEVEL, 0x00, 0x03, &records);
        net.run(50);
        assert_eq!(&net.coord.last_zcl(short, LEVEL, 0x04).unwrap()[..3], &[0x18, seq, 0x04]);
        assert_eq!(&net.coord.last_zcl(short, LEVEL, 0x04).unwrap()[3..], &status[..]);
        assert_eq!(read(&mut net, short, LEVEL, 0x0011), vec![0x00, 0x20, 0xFF]);
    }
    let seq = net.coord.zcl(short, LEVEL, 0x00, 0x03, &[0x11, 0x00, 0x20, 40, 0x0F, 0x00, 0x18, 0x01, 0x00, 0x40, 0x20, 0]);
    net.run(50);
    assert_eq!(net.coord.last_zcl(short, LEVEL, 0x04).unwrap(), vec![0x18, seq, 0x04, 0x00]);
    assert_eq!(read(&mut net, short, LEVEL, 0x0011), vec![0x00, 0x20, 40]);
    assert_eq!(read(&mut net, short, LEVEL, 0x000F), vec![0x00, 0x18, 0x01]);
    assert!(net.nodes[n].commands.contains(&Command::SetStartupLevel(0)));
    let seq = net.coord.zcl(short, LEVEL, 0x00, 0x02, &[0x11, 0x00, 0x20, 77, 0x00, 0x0F, 0x20, 1]);
    net.run(50);
    assert_eq!(net.coord.last_zcl(short, LEVEL, 0x04).unwrap(), vec![0x18, seq, 0x04, 0x86, 0x00, 0x0F]);
    assert_eq!(read(&mut net, short, LEVEL, 0x0011), vec![0x00, 0x20, 77]);
}

#[test]
fn a_step_of_the_colour_temperature_during_a_fade_counts_from_where_the_fade_is() {
    let (mut net, n, short) = tunable();
    net.coord.zcl(short, COLOR, 0x11, 0x0A, &[153, 0, 0, 0]);
    net.run(50);
    net.coord.zcl(short, COLOR, 0x11, 0x0A, &[0xC5, 0x01, 100, 0]);
    net.run(5_000);
    net.coord.zcl(short, COLOR, 0x11, 0x4C, &[1, 10, 0, 0, 0, 0, 0, 0, 0]);
    net.run(50);
    assert_eq!(net.nodes[n].commands.last(), Some(&Command::MoveToColorTemperature { mireds: 313, transition_ms: 0 }), "half way from 153 to 453");
    net.coord.zcl(short, COLOR, 0x11, 0x0A, &[153, 0, 100, 0]);
    net.run(5_000);
    let before = net.nodes[n].commands.len();
    net.coord.zcl(short, COLOR, 0x11, 0x47, &[]);
    net.coord.zcl(short, COLOR, 0x11, 0x4C, &[1, 10, 0, 0, 0, 0, 0, 0, 0]);
    net.run(50);
    assert_eq!(
        net.nodes[n].commands[before..],
        [Command::StopColorTransition, Command::MoveToColorTemperature { mireds: 243, transition_ms: 0 }],
        "stopped half way from 313 to 153"
    );
}

#[test]
fn a_scene_colour_of_only_x_and_y_is_recalled_as_a_colour_temperature_on_a_tunable_white() {
    let (mut net, n, short) = tunable();
    let add = |net: &mut Net, id: u8, colour: &[u8]| {
        let mut p = vec![0, 0, id, 0, 0, 0, 0x00, 0x03, colour.len() as u8];
        p.extend_from_slice(colour);
        let seq = net.coord.zcl(short, SCENES, 0x01, 0x00, &p);
        net.run(50);
        assert_eq!(net.coord.last_zcl(short, SCENES, 0x00).unwrap(), vec![0x19, seq, 0x00, 0x00, 0, 0, id]);
    };
    let recall = |net: &mut Net, id: u8| {
        net.coord.zcl(short, SCENES, 0x11, 0x05, &[0, 0, id]);
        net.run(50);
        *net.nodes[n].commands.last().unwrap()
    };
    let ct = |mireds: u16| Command::MoveToColorTemperature { mireds, transition_ms: 0 };
    add(&mut net, 1, &[0x64, 0x61, 0x72, 0x60]);
    add(&mut net, 2, &[0, 0, 0, 0]);
    add(&mut net, 3, &[0, 0x20, 0, 0x30]);
    let mut longer = vec![0xB9, 0x75, 0x1D, 0x69];
    longer.extend_from_slice(&[0; 9]);
    add(&mut net, 4, &longer);
    assert_eq!(recall(&mut net, 1), ct(250), "4000 K as zigbee2mqtt sends it");
    assert_eq!(recall(&mut net, 2), ct(454), "red of the curve: the warmest");
    assert_eq!(recall(&mut net, 3), ct(153), "blue of it: the coldest");
    assert_eq!(recall(&mut net, 4), ct(370), "2700 K with a temperature of 0");
    let seq = net.coord.zcl(short, SCENES, 0x01, 0x01, &[0, 0, 1]);
    net.run(50);
    let view = net.coord.last_zcl(short, SCENES, 0x01).unwrap();
    assert_eq!(&view[..3], &[0x19, seq, 0x01]);
    assert_eq!(&view[view.len() - 16..], &[0x00, 0x03, 13, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 250, 0]);
}

#[test]
fn a_scene_stored_during_a_fade_keeps_the_level_and_colour_temperature_reached() {
    let mut net = Net::new();
    net.identity = IDENTITY_TW;
    let n = net.join_node();
    let short = net.nodes[n].short();
    net.coord.zcl(short, GROUPS, 0x01, 0x00, &[0x34, 0x12, 0]);
    level(&mut net, short, 254);
    net.coord.zcl(short, COLOR, 0x11, 0x0A, &[200, 0, 0, 0]);
    net.run(50);
    net.coord.zcl(short, LEVEL, 0x11, 0x00, &[54, 100, 0]);
    net.coord.zcl(short, COLOR, 0x11, 0x0A, &[0xC8, 0x01, 100, 0]);
    net.run(5_000);
    net.coord.zcl(short, SCENES, 0x01, 0x04, &[0x34, 0x12, 7]);
    net.run(50);
    net.coord.zcl(short, ON_OFF, 0x11, 0x40, &[0, 0]);
    net.run(50);
    let before = net.nodes[n].commands.len();
    net.coord.zcl(short, SCENES, 0x01, 0x05, &[0x34, 0x12, 7]);
    net.run(50);
    let recalled = &net.nodes[n].commands[before..];
    assert!(
        matches!(recalled, [Command::MoveToColorTemperature { mireds: 325..=329, .. }, Command::MoveToLevel { level: 150..=155, .. }]),
        "ZCL 8 3.7.2.4.4: Store Scene keeps the current values, half way from 254 to 54 and from 200 to 454 mireds; got {recalled:?}"
    );
    level(&mut net, short, 254);
    let before = net.nodes[n].commands.len();
    net.coord.zcl(short, LEVEL, 0x11, 0x00, &[54, 100, 0]);
    net.run(5_000);
    net.coord.zcl(short, ON_OFF, 0x11, 0x40, &[0, 1]);
    net.run(50);
    net.coord.zcl(short, ON_OFF, 0x11, 0x41, &[]);
    net.run(50);
    assert!(
        matches!(net.nodes[n].commands.last(), Some(Command::MoveToLevel { level: 150..=155, .. })),
        "ZCL 8 3.8.2.3.4.3: Off With Effect keeps the current settings in the global scene; got {:?}",
        &net.nodes[n].commands[before..]
    );
}
