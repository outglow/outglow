use memcap as _;
mod sim;

use ble::diag::{self, parse, zigbee_event, DiagCommand, FrameLog, Info, LogEntry, State};
use bulb_core::command::{Command, PowerOnBehavior};
use sim::{Rig, DIMMABLE, TUNABLE};
use zigbee::stack::{ev, LightSettings};

const INFO: u16 = 0x22;
const STATE: u16 = 0x24;
const LOG: u16 = 0x26;
const CMD: u16 = 0x28;
const FRAMES: u16 = 0x2a;
const CONFIG: u16 = 0x2c;
const COUNTERS: u16 = 0x2e;

fn uuid(short: u8) -> Vec<u8> {
    vec![0x6f, 0x5e, 0x4d, 0x3c, 0x2b, 0x1a, 0x2f, 0x8e, 0x3d, 0x4c, 0x4b, 0x5a, short, 0x00, 0x10, 0xe6]
}

fn connected(identity: ble::Identity) -> Rig {
    let mut rig = Rig::new(identity);
    rig.connect();
    rig
}

fn read_long(rig: &mut Rig, handle: u16) -> Vec<u8> {
    let mut v = rig.read(handle);
    let mut last = v.len();
    while last == 22 {
        let mut q = vec![0x0c];
        q.extend_from_slice(&handle.to_le_bytes());
        q.extend_from_slice(&(v.len() as u16).to_le_bytes());
        let rsp = rig.att(&q);
        assert_eq!(rsp[0], 0x0d, "Read Blob of {handle:#x}: {rsp:02x?}");
        last = rsp.len() - 1;
        v.extend_from_slice(&rsp[1..]);
    }
    v
}

fn write_cmd(rig: &mut Rig, value: &[u8]) -> u8 {
    rig.write(CMD, value);
    read_long(rig, STATE)[39]
}

#[test]
fn the_service_is_found_by_its_uuid_after_the_stock_handles() {
    let mut rig = connected(DIMMABLE);
    let mut q = vec![0x06, 0x01, 0x00, 0xff, 0xff, 0x00, 0x28];
    q.extend(uuid(0));
    assert_eq!(rig.att(&q), [0x07, 0x20, 0x00, 0x2e, 0x00]);
    let mut found = vec![];
    let mut start = 0x20u16;
    loop {
        let mut q = vec![0x08];
        q.extend_from_slice(&start.to_le_bytes());
        q.extend_from_slice(&[0x2e, 0x00, 0x03, 0x28]);
        let rsp = rig.att(&q);
        if rsp[0] != 0x09 {
            assert_eq!(rsp, [0x01, 0x08, start as u8, 0x00, 0x0a]);
            break;
        }
        for e in rsp[2..].chunks(rsp[1] as usize) {
            let h = u16::from_le_bytes([e[0], e[1]]);
            assert_eq!(e[5..].to_vec(), uuid(e[17]));
            found.push((h, e[2], u16::from_le_bytes([e[3], e[4]]), e[17]));
            start = h + 1;
        }
    }
    assert_eq!(
        found,
        [(0x21, 0x02, INFO, 1), (0x23, 0x02, STATE, 2), (0x25, 0x02, LOG, 3), (0x27, 0x08, CMD, 4), (0x29, 0x02, FRAMES, 5), (0x2b, 0x02, CONFIG, 6), (0x2d, 0x02, COUNTERS, 7)]
    );
}

#[test]
fn the_command_characteristic_is_written_not_read() {
    let mut rig = connected(DIMMABLE);
    assert_eq!(rig.att(&[0x0a, CMD as u8, 0x00]), [0x01, 0x0a, CMD as u8, 0x00, 0x02], "Read Not Permitted");
    assert_eq!(rig.att(&[0x0c, CMD as u8, 0x00, 0x00, 0x00]), [0x01, 0x0c, CMD as u8, 0x00, 0x02]);
    let mut q = vec![0x08, 0x01, 0x00, 0xff, 0xff];
    q.extend(uuid(4));
    assert_eq!(rig.att(&q), [0x01, 0x08, CMD as u8, 0x00, 0x02], "by type: the error names the handle");
    assert_eq!(rig.att(&[0x12, STATE as u8, 0x00, 0x01]), [0x01, 0x12, STATE as u8, 0x00, 0x03]);
}

#[test]
fn the_values_read_as_the_firmware_set_them() {
    let mut rig = connected(TUNABLE);
    {
        let mut b = rig.ble.borrow_mut();
        b.set_diag_info(&Info {
            tunable: true,
            watchdog: true,
            sense: false,
            release: 4,
            ieee: 0xa4c1_3811_2233_4455,
            model: "ESMLFzm_w6_TW",
            firmware: "3.0.2_1610",
        });
        b.set_diag_state(&State {
            uptime_s: 0x0102_0304,
            reset_cause: 1,
            watchdog_resets: 2,
            reset_step: 3,
            starts: 0x0506,
            joined: true,
            short: 0x1234,
            pan: 0xabcd,
            channel: 15,
            neighbors: 4,
            routers: 3,
            end_devices: 1,
            lqi: 200,
            rssi: -50,
            mac_rx: 1000,
            mac_tx: 900,
            mac_retries: 30,
            mac_failures: 2,
            ..State::default()
        });
        b.set_diag_config(&LightSettings {
            start_up_on_off: 0xFF,
            start_up_level: 80,
            level_options: 3,
            color_options: Some(1),
            start_up_color_temperature: 300,
            ct_min: 153,
            ct_max: 454,
            global_scene_control: true,
            on_time: 7,
            off_wait_time: 8,
            remaining_tenths: 9,
            min_level: 1,
            max_level: 254,
        });
        b.set_diag_log((0..20).map(|i| LogEntry { time: 100 - i, code: 37, arg: i as u8 }));
        let mut f = FrameLog::EMPTY;
        for i in 0..10u16 {
            f.push(i as u32, 0x0000, 0x0006, i as u8, 0x11, 180, -40);
        }
        b.set_diag_frames(&f);
        b.set_diag_counters(&[1, 0x00, 0x00, 0x21, 0x05, 0x00]);
    }
    let info = read_long(&mut rig, INFO);
    assert_eq!(info[..12], [1, 1, 1, 4, 0x55, 0x44, 0x33, 0x22, 0x11, 0x38, 0xc1, 0xa4]);
    assert_eq!(info[12], 13);
    assert_eq!(&info[13..26], b"ESMLFzm_w6_TW");
    assert_eq!(info[26], 10);
    assert_eq!(&info[27..], b"3.0.2_1610");

    let s = read_long(&mut rig, STATE);
    assert_eq!(s.len(), diag::STATE_LEN);
    assert_eq!(s[..12], [1, 4, 3, 2, 1, 1, 0, 2, 0, 3, 6, 5]);
    assert_eq!(s[12..23], [1, 0x34, 0x12, 0xcd, 0xab, 15, 4, 3, 1, 200, (-50i8) as u8]);
    assert_eq!(s[23..35], [0xe8, 0x03, 0, 0, 0x84, 0x03, 0, 0, 30, 0, 2, 0]);
    let light = rig.bulb.state();
    assert_eq!(s[35..37], [light.on as u8, light.level]);
    assert_eq!(u16::from_le_bytes([s[37], s[38]]), light.color_temp);
    assert_eq!(s[39], 0x00, "no command yet, not logged in");

    assert_eq!(read_long(&mut rig, CONFIG), [1, 0xff, 80, 3, 1, 0x2c, 0x01, 153, 0, 0xc6, 0x01, 1, 7, 0, 8, 0, 9, 0, 1, 254]);

    let log = read_long(&mut rig, LOG);
    assert_eq!(log.len(), 1 + diag::LOG_ENTRIES * 6, "as many as fit");
    assert_eq!(log[0] as usize, diag::LOG_ENTRIES);
    assert_eq!(log[1..7], [100, 0, 0, 0, 37, 0]);
    assert_eq!(log[91..97], [85, 0, 0, 0, 37, 15]);

    let frames = read_long(&mut rig, FRAMES);
    assert_eq!(frames.len(), diag::FRAMES_MAX);
    assert_eq!(frames[0] as usize, 8 * 12);
    assert_eq!(frames[1..13], [9, 0, 0, 0, 0, 0, 0x06, 0x00, 9, 0x11, 180, (-40i8) as u8], "newest first");
    assert_eq!(frames[85], 2, "the oldest kept");

    assert_eq!(read_long(&mut rig, COUNTERS), [1, 0x00, 0x00, 0x21, 0x05, 0x00]);
}

#[test]
fn a_dimmable_bulb_shows_no_colour() {
    let mut rig = connected(DIMMABLE);
    let s = rig.read(STATE);
    assert_eq!(s.len(), 22, "the first part of 40");
    let s = read_long(&mut rig, STATE);
    assert_eq!(s[37..39], [0xff, 0xff]);
    rig.ble.borrow_mut().set_diag_config(&LightSettings {
        start_up_on_off: 1,
        start_up_level: 0xff,
        level_options: 0,
        color_options: None,
        start_up_color_temperature: 0xffff,
        ct_min: 0,
        ct_max: 0,
        global_scene_control: true,
        on_time: 0,
        off_wait_time: 0,
        remaining_tenths: 0,
        min_level: 1,
        max_level: 254,
    });
    let c = rig.read(CONFIG);
    assert_eq!(c[4..11], [0xff, 0xff, 0xff, 0, 0, 0, 0]);
}

#[test]
fn only_the_blink_works_without_a_login() {
    let mut rig = connected(DIMMABLE);
    assert_eq!(write_cmd(&mut rig, &[0x01]), 0x10);
    assert_eq!(rig.ble.borrow_mut().take_diag_command(), Some(DiagCommand::Identify));
    assert_eq!(write_cmd(&mut rig, &[0x10, 0x00]), 0x21);
    assert_eq!(rig.ble.borrow_mut().take_diag_command(), None);
    assert_eq!(write_cmd(&mut rig, &[0x31, b'R', b'S', b'T']), 0x31);
    assert_eq!(rig.ble.borrow_mut().take_diag_command(), None);
    rig.write(CMD, &[]);
    assert_eq!(read_long(&mut rig, STATE)[39], 0x31);

    assert!(rig.login(b"unpaired", b"1234"));
    assert_eq!(read_long(&mut rig, STATE)[39], 0x80 | 0x31, "logged in");
    assert_eq!(write_cmd(&mut rig, &[0x10, 0x00]), 0x80 | 0x40);
    assert_eq!(rig.ble.borrow_mut().take_diag_command(), Some(DiagCommand::Light(Command::Off)));
    assert_eq!(write_cmd(&mut rig, &[0x10, 0x07]), 0x80 | 0x52, "invalid parameters");
    assert_eq!(rig.ble.borrow_mut().take_diag_command(), None);
    for _ in 0..3 {
        write_cmd(&mut rig, &[0x30]);
    }
    assert_eq!(read_long(&mut rig, STATE)[39], 0x80 | 0x00);
    assert_eq!(rig.ble.borrow_mut().take_diag_command(), Some(DiagCommand::Restart));

    rig.central().terminate();
    rig.run(500);
    rig.connect();
    assert_eq!(read_long(&mut rig, STATE)[39] & 0x80, 0);
}

#[test]
fn commands_are_parsed_as_the_c_firmware_takes_them() {
    let tw = &TUNABLE;
    let dimm = &DIMMABLE;
    let light = |c| Some(DiagCommand::Light(c));
    assert_eq!(parse(&[0x01], dimm), Some(DiagCommand::Identify));
    assert_eq!(parse(&[], dimm), None);
    assert_eq!(parse(&[0x10, 1], dimm), light(Command::On));
    assert_eq!(parse(&[0x10, 2], dimm), light(Command::Toggle));
    assert_eq!(parse(&[0x10], dimm), None);
    assert_eq!(parse(&[0x11, 60, 5, 0], dimm), light(Command::MoveToLevel { level: 60, transition_ms: 500 }));
    assert_eq!(parse(&[0x11, 255, 0, 0], dimm), light(Command::MoveToLevel { level: 254, transition_ms: 0 }));
    assert_eq!(parse(&[0x11, 60, 5], dimm), None);
    assert_eq!(parse(&[0x12, 0x90, 0x01, 10, 0], tw), light(Command::MoveToColorTemperature { mireds: 400, transition_ms: 1000 }));
    assert_eq!(parse(&[0x12, 100, 0, 0, 0], tw), None);
    assert_eq!(parse(&[0x12, 0xc6, 0x01, 0, 0], tw), light(Command::MoveToColorTemperature { mireds: 454, transition_ms: 0 }));
    assert_eq!(parse(&[0x12, 0x90, 0x01, 0, 0], dimm), None);
    assert_eq!(parse(&[0x20, 0xff], dimm), light(Command::SetPowerOnBehavior(PowerOnBehavior::Previous)));
    assert_eq!(parse(&[0x20, 3], dimm), None);
    assert_eq!(parse(&[0x21, 0], dimm), light(Command::SetStartupLevel(0)));
    assert_eq!(parse(&[0x22, 0xff], tw), Some(DiagCommand::LevelOptions(3)));
    assert_eq!(parse(&[0x22, 0xff], dimm), Some(DiagCommand::LevelOptions(1)));
    assert_eq!(parse(&[0x23, 0xff], tw), Some(DiagCommand::ColorOptions(1)));
    assert_eq!(parse(&[0x23, 1], dimm), None);
    assert_eq!(parse(&[0x24, 0xff, 0xff], tw), light(Command::SetStartupColorTemperature(0xffff)));
    assert_eq!(parse(&[0x24, 0x2c, 0x01], tw), light(Command::SetStartupColorTemperature(300)));
    assert_eq!(parse(&[0x24, 0x64, 0x00], tw), None);
    assert_eq!(parse(&[0x24, 0xff, 0xff], dimm), None);
    assert_eq!(parse(&[0x30], dimm), Some(DiagCommand::Restart));
    assert_eq!(parse(&[0x31, b'R', b'S', b'T'], dimm), Some(DiagCommand::FactoryReset));
    assert_eq!(parse(&[0x31, b'R', b'S'], dimm), None);
    assert_eq!(parse(&[0x31, b'r', b'S', b'T'], dimm), None);
    assert_eq!(parse(&[0x99], dimm), None);
    for short in [&[0x11u8][..], &[0x11, 60], &[0x12, 0x90], &[0x12, 0x90, 0x01, 10], &[0x21], &[0x22], &[0x23], &[0x24, 0xff], &[0x20]] {
        assert_eq!(parse(short, tw), None, "{short:02x?}");
    }
}

#[test]
fn the_stacks_events_take_the_c_firmwares_codes_where_they_mean_the_same() {
    assert_eq!(zigbee_event(ev::START, 1, 7, 11), (1, 1), "start, the reset cause");
    assert_eq!(zigbee_event(ev::UP, 0x1234, 0, 15), (5, 15), "joined, the channel");
    assert_eq!(zigbee_event(ev::LEFT, 0, 0, 15), (6, 0));
    assert_eq!(zigbee_event(ev::BLE_CONNECT, 3, 0, 15), (8, 0));
    assert_eq!(zigbee_event(ev::BLE_DISCONNECT, 3, 0, 15), (9, 0));
    assert_eq!(zigbee_event(ev::IDENTIFY, 0, 0, 15), (10, 0));
    assert_eq!(zigbee_event(ev::NEIGHBOR_ADDED, 0x1234, 180, 15), (37, 180), "the neighbor's link quality");
    assert_eq!(zigbee_event(ev::TX_FAIL, 0x1234, 0xe9, 15), (50, 0xe9), "the MAC status");
    assert_eq!(zigbee_event(ev::STATUS_RX, 0x0b, 0x1234, 15), (41, 0x0b), "the status code");
    assert_eq!(zigbee_event(ev::BLE_LOST, 2, 0, 15), (49, 2));
}

#[test]
fn long_names_are_cut_to_what_the_48_octets_hold() {
    let mut rig = connected(DIMMABLE);
    let info = |model, firmware, sense| Info { tunable: false, watchdog: false, sense, release: 1, ieee: 1, model, firmware };
    rig.ble.borrow_mut().set_diag_info(&info("M".repeat(40).leak(), "F".repeat(20).leak(), true));
    let i = read_long(&mut rig, INFO);
    assert_eq!(i.len(), 48);
    assert_eq!(i[2], 0x02, "sense without the watchdog");
    assert_eq!((i[12], i[46], i[47]), (33, 1, b'F'), "33 octets of model leave one for the firmware");
    rig.ble.borrow_mut().set_diag_info(&info("Model", "F".repeat(40).leak(), false));
    let i = read_long(&mut rig, INFO);
    assert_eq!(i.len(), 48);
    assert_eq!((i[2], i[12], i[18]), (0x00, 5, 29));
}

#[test]
fn a_bulb_outside_a_network_shows_no_address() {
    let mut rig = connected(DIMMABLE);
    rig.ble.borrow_mut().set_diag_state(&State { joined: false, short: 0x1234, pan: 0xabcd, ..State::default() });
    let s = read_long(&mut rig, STATE);
    assert_eq!(s[12..17], [0, 0xff, 0xff, 0xff, 0xff]);
}
