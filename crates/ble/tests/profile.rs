//! The attribute table and the AwoX mesh profile, from the app's side.

mod sim;

use ble::crypto::{crc8_6c, tl_aes_encrypt};
use ble::LinkState;
use sim::{Rig, ADDRESS, COLOUR, DIMMABLE, TUNABLE};

const STATUS: u16 = 0x12;
const COMMAND: u16 = 0x15;
const PAIR: u16 = 0x1e;

const OP_POWER: u8 = 0xd0;
const OP_STATUS: u8 = 0xda;
const OP_RESET: u8 = 0xe3;
const OP_TEMPERATURE: u8 = 0xf0;
const OP_BRIGHTNESS: u8 = 0xf1;

fn hex(s: &str) -> Vec<u8> {
    (0..s.len() / 2).map(|i| u8::from_str_radix(&s[2 * i..2 * i + 2], 16).unwrap()).collect()
}

fn connected(identity: ble::Identity) -> Rig {
    let mut rig = Rig::new(identity);
    rig.connect();
    rig
}

fn logged_in(identity: ble::Identity) -> Rig {
    let mut rig = connected(identity);
    assert!(rig.login(b"unpaired", &Rig::secure_password()));
    rig.enable_status();
    rig.run(400);
    rig.statuses();
    rig
}

/// A command in the stock firmware's block format (emulator/awox_mesh.py).
fn block_packet(key: &[u8; 16], kind: u8, payload: &[u8]) -> Vec<u8> {
    let mut plain = [0u8; 16];
    plain[1] = payload.len() as u8;
    plain[2..2 + payload.len()].copy_from_slice(payload);
    plain[0] = crc8_6c(&plain[1..2 + payload.len()]);
    let mut packet = vec![kind];
    packet.extend_from_slice(&tl_aes_encrypt(key, &plain));
    packet
}

#[test]
fn the_test_client_builds_what_the_python_clients_build() {
    // vectors from flasher/awox_ota.py and emulator/awox_mesh.py for the
    // address a4:c1:38:11:22:33, random 01..08, device random 11..18
    let mut rig = Rig::new(DIMMABLE);
    rig.key = Some(hex("3c048be01d5756b9cfb9c7e0798f7871").try_into().unwrap());
    assert_eq!(rig.command_packet(0xffff, OP_POWER, &[1]), hex("010100b70d84582bbda555b855c3e6ca18ec3e5a"));
    assert_eq!(rig.command_packet(0x0000, OP_BRIGHTNESS, &[64]), hex("020100e9a9c6d135a44020ab206f1991a71c30d1"));
    let key = rig.key.unwrap();
    assert_eq!(block_packet(&key, 2, &[OP_POWER, 0x60, 0x01, 0])[..17], hex("029787efc2f11a6cb2c79b704bf2e0609c"));
    assert_eq!(
        block_packet(&key, 0, &[0xff, 0xff, 0x01, 0x08, 0x00, 0x04, 128, 5, 0, 0, 0, 0]),
        hex("006b3bc6d48567c0c48ee57faff36de5d4")
    );
    assert_eq!(
        tl_aes_encrypt(&core::array::from_fn(|i| i as u8), &core::array::from_fn(|i| 16 + i as u8)).to_vec(),
        hex("61b04d2d3166e91a0e6c16cae8bbc0f6")
    );
}

#[test]
fn a_login_packet_from_the_python_client_is_accepted() {
    let mut rig = connected(DIMMABLE);
    // awox_ota.login_packet("unpaired", secure_password(mac), 01..08)
    rig.write(PAIR, &hex("0c0102030405060708df7376b1f81f686e"));
    assert_eq!(rig.read(PAIR)[0], 0x0d);
    // ... and with "1234"
    rig.write(PAIR, &hex("0c010203040506070801a3567c6b987afd"));
    assert_eq!(rig.read(PAIR)[0], 0x0d);
}

#[test]
fn services_and_characteristics_sit_at_the_stock_handles() {
    let mut rig = connected(DIMMABLE);
    // primary services
    let rsp = rig.att(&[0x10, 0x01, 0x00, 0xff, 0xff, 0x00, 0x28]);
    assert_eq!(rsp, [0x11, 6, 0x01, 0x00, 0x06, 0x00, 0x00, 0x18, 0x07, 0x00, 0x0f, 0x00, 0x0a, 0x18]);
    let rsp = rig.att(&[0x10, 0x10, 0x00, 0xff, 0xff, 0x00, 0x28]);
    assert_eq!(rsp[..6], [0x11, 20, 0x10, 0x00, 0x1f, 0x00]);
    assert_eq!(rsp[6..], hex("10190d0c0b0a09080706050403020100"));
    let rsp = rig.att(&[0x10, 0x20, 0x00, 0xff, 0xff, 0x00, 0x28]);
    assert_eq!(rsp, [0x01, 0x10, 0x20, 0x00, 0x0a], "no service after the mesh service");

    // characteristics of the mesh service: (declaration handle, properties, value handle, short id)
    let mut found = vec![];
    let mut start = 0x10u16;
    loop {
        let mut req = vec![0x08];
        req.extend_from_slice(&start.to_le_bytes());
        req.extend_from_slice(&[0xff, 0xff, 0x03, 0x28]);
        let rsp = rig.att(&req);
        if rsp[0] != 0x09 {
            assert_eq!(rsp[..2], [0x01, 0x08]);
            assert_eq!(rsp[4], 0x0a, "the search ends with Attribute Not Found");
            break;
        }
        for e in rsp[2..].chunks(rsp[1] as usize) {
            let handle = u16::from_le_bytes([e[0], e[1]]);
            found.push((handle, e[2], u16::from_le_bytes([e[3], e[4]]), e[5]));
            assert_eq!(e[6..], hex("190d0c0b0a09080706050403020100"));
            start = handle + 1;
        }
    }
    assert_eq!(
        found,
        [(0x11, 0x1a, 0x12, 0x11), (0x14, 0x0e, 0x15, 0x12), (0x17, 0x0a, 0x18, 0x13), (0x1a, 0x06, 0x1b, 0x15), (0x1d, 0x0a, 0x1e, 0x14)]
    );

    // descriptors
    assert_eq!(rig.att(&[0x04, 0x13, 0x00, 0x13, 0x00]), [0x05, 0x01, 0x13, 0x00, 0x02, 0x29]);
    assert_eq!(rig.read(0x16), b"cmd");
    assert_eq!(rig.read(0x1c), b"OTA2");
    // the whole table, one format per response
    let rsp = rig.att(&[0x04, 0x01, 0x00, 0xff, 0xff]);
    assert_eq!(rsp[..2], [0x05, 0x01]);
    assert_eq!(rsp.len(), 2 + 5 * 4, "five 16-bit entries fit the default MTU");
    let rsp = rig.att(&[0x04, 0x12, 0x00, 0xff, 0xff]);
    assert_eq!(rsp[..4], [0x05, 0x02, 0x12, 0x00], "a 128-bit entry goes alone");
    assert_eq!(rsp.len(), 2 + 18);
    assert_eq!(rig.att(&[0x04, 0x20, 0x00, 0xff, 0xff]), [0x01, 0x04, 0x20, 0x00, 0x0a]);
    // 16-bit entries stop where the first 128-bit one would follow
    let rsp = rig.att(&[0x04, 0x0f, 0x00, 0xff, 0xff]);
    assert_eq!(rsp, [0x05, 0x01, 0x0f, 0x00, 0x27, 0x2a, 0x10, 0x00, 0x00, 0x28, 0x11, 0x00, 0x03, 0x28]);
}

#[test]
fn device_information_reads_like_the_stock() {
    let mut rig = connected(TUNABLE);
    assert_eq!(rig.read(0x03), b"unpaired");
    assert_eq!(rig.read(0x09), b"3.0.2_1610");
    assert_eq!(rig.read(0x0b), b"AwoX");
    assert_eq!(rig.read(0x0d), b"ESMLFzm_w6_TW");
    assert_eq!(rig.read(0x0f), b"4.63");
    // by type: the device name
    assert_eq!(rig.att(&[0x08, 0x01, 0x00, 0xff, 0xff, 0x00, 0x2a]), [&[0x09, 10, 0x03, 0x00][..], b"unpaired"].concat());
    // primary service by its UUID
    assert_eq!(rig.att(&[0x06, 0x01, 0x00, 0xff, 0xff, 0x00, 0x28, 0x0a, 0x18]), [0x07, 0x07, 0x00, 0x0f, 0x00]);
}

#[test]
fn requests_that_cannot_be_served_get_the_right_error() {
    let mut rig = connected(DIMMABLE);
    assert_eq!(rig.att(&[0x0a, 0x40, 0x00]), [0x01, 0x0a, 0x40, 0x00, 0x01], "read of a handle that does not exist");
    assert_eq!(rig.att(&[0x0a, 0x00, 0x00]), [0x01, 0x0a, 0x00, 0x00, 0x01]);
    assert_eq!(rig.att(&[0x12, 0x0b, 0x00, 0x41]), [0x01, 0x12, 0x0b, 0x00, 0x03], "write to a read-only value");
    assert_eq!(rig.att(&[0x08, 0x05, 0x00, 0x01, 0x00, 0x03, 0x28]), [0x01, 0x08, 0x05, 0x00, 0x01], "range that ends before it starts");
    assert_eq!(rig.att(&[0x10, 0x01, 0x00, 0xff, 0xff, 0x03, 0x28]), [0x01, 0x10, 0x01, 0x00, 0x10], "grouping by anything but services");
    assert_eq!(rig.att(&[0x16, 0x15, 0x00, 0x00, 0x00, 0x01]), [0x01, 0x16, 0x00, 0x00, 0x06], "prepared writes");
    assert_eq!(rig.att(&[0x12, 0x13, 0x00, 0x01, 0x00, 0x00]), [0x01, 0x12, 0x13, 0x00, 0x0d], "a client configuration of three bytes");
    assert_eq!(rig.att(&[0x0c, 0x0b, 0x00, 0x09, 0x00]), [0x01, 0x0c, 0x0b, 0x00, 0x07], "blob offset past the end");
    assert_eq!(rig.att(&[0x0c, 0x0b, 0x00, 0x02, 0x00]), [0x0d, b'o', b'X']);
    // a command is never answered, even when it cannot be served
    rig.central().send_att(&[0x52, 0x0b, 0x00, 0x41]);
    rig.run(300);
    assert!(rig.central().att.is_empty());
}

#[test]
fn requests_of_the_wrong_length_are_invalid() {
    let mut rig = connected(DIMMABLE);
    // Vol 3 Part F 3.3: Invalid PDU, handle 0x0000, for a request shorter or longer than its format
    for (request, what) in [
        (&[0x04, 0x01, 0x00, 0xff, 0xff, 0x00][..], "Find Information, 6 octets"),
        (&[0x04, 0x01, 0x00, 0xff][..], "Find Information, 4 octets"),
        (&[0x0a, 0x03, 0x00, 0x00][..], "Read, 4 octets"),
        (&[0x0a, 0x03][..], "Read, 2 octets"),
        (&[0x0c, 0x03, 0x00, 0x00, 0x00, 0x00][..], "Read Blob, 6 octets"),
        (&[0x02, 0x17, 0x00, 0x00][..], "Exchange MTU, 4 octets"),
    ] {
        assert_eq!(rig.att(request), [0x01, request[0], 0x00, 0x00, 0x04], "{what}");
    }
    // Read By Type and Read By Group Type with a type of 3, 4, 15 or 17 octets
    for n in [3, 4, 15, 17] {
        for opcode in [0x08, 0x10] {
            let mut request = vec![opcode, 0x01, 0x00, 0xff, 0xff, 0x00, 0x28];
            request.resize(5 + n, 0);
            assert_eq!(rig.att(&request), [0x01, opcode, 0x00, 0x00, 0x04], "{opcode:#04x} with a type of {n} octets");
        }
    }
    // the requests of the right length right after
    assert_eq!(rig.att(&[0x0a, 0x0b, 0x00]), b"\x0bAwoX");
    assert_eq!(rig.att(&[0x02, 0x17, 0x00]), [0x03, 64, 0]);
}

#[test]
fn a_larger_mtu_gives_longer_responses_in_fragments() {
    let mut rig = connected(DIMMABLE);
    assert_eq!(rig.att(&[0x02, 247, 0]), [0x03, 64, 0]);
    // three 16-bit characteristic declarations of 7 bytes fit 23; with 64 all four of Device Information
    let rsp = rig.att(&[0x08, 0x07, 0x00, 0x0f, 0x00, 0x03, 0x28]);
    assert_eq!(rsp.len(), 2 + 4 * 7);
    // 15 entries of four bytes: more than two link layer packets
    let rsp = rig.att(&[0x04, 0x01, 0x00, 0xff, 0xff]);
    assert_eq!(rsp.len(), 2 + 15 * 4);
    // 16-bit and 128-bit UUIDs never share a response, however much room there is
    assert_eq!(rig.att(&[0x04, 0x0f, 0x00, 0xff, 0xff]).len(), 2 + 3 * 4);
    // a client that asks for less than the minimum still gets 23
    assert_eq!(rig.att(&[0x02, 10, 0]), [0x03, 64, 0]);
    assert_eq!(rig.att(&[0x04, 0x01, 0x00, 0xff, 0xff]).len(), 2 + 5 * 4);
}

#[test]
fn login_takes_the_factory_passwords_and_no_other() {
    let mut rig = connected(DIMMABLE);
    assert!(!rig.login(b"unpaired", b"wrong-password"));
    assert!(rig.login(b"unpaired", b"1234"), "the password of old apps");
    assert!(rig.login(b"unpaired", &Rig::secure_password()), "sha256 of the address and AwoX's secret");
    assert!(!rig.login(b"somewhere", b"1234"), "another network's name");
}

#[test]
fn light_commands_work_after_login_and_changes_are_notified() {
    let mut rig = connected(DIMMABLE);
    assert!(rig.login(b"unpaired", &Rig::secure_password()));
    rig.enable_status();
    rig.run(400);
    // 0xdc: address, 0, mode, brightness 1..=127, temperature
    let first = rig.statuses();
    assert_eq!(first.len(), 1, "one notification when status is enabled");
    assert_eq!(first[0].0, 0xdc);
    assert_eq!(first[0].1[..5], [0x33, 0, 1, 127, 64]);

    rig.command(OP_POWER, &[0]);
    assert!(!rig.is_on());
    assert_eq!(rig.statuses().last().unwrap().1[2], 0, "off is notified");
    rig.command(OP_POWER, &[1]);
    assert!(rig.is_on());
    rig.command(OP_BRIGHTNESS, &[64]);
    assert_eq!(rig.level(), 128);
    let s = rig.statuses();
    assert_eq!(s.last().unwrap().1[2..4], [1, 64]);
    rig.command(OP_BRIGHTNESS, &[127]);
    assert_eq!(rig.level(), 254);
    rig.statuses();

    rig.command(OP_STATUS, &[0x10]);
    let s = rig.statuses();
    assert_eq!(s.len(), 1);
    // 0xdb: mode, brightness, temperature right after the vendor id
    assert_eq!((s[0].0, &s[0].1[..3]), (0xdb, &[1, 127, 64][..]));

    // a change from elsewhere (Zigbee) is notified too
    rig.bulb.handle(bulb_core::Command::Off);
    rig.run(400);
    assert_eq!(rig.statuses().last().unwrap().1[2], 0);
}

#[test]
fn brightness_while_off_does_not_turn_the_light_on() {
    let mut rig = logged_in(DIMMABLE);
    rig.command(OP_POWER, &[0]);
    rig.command(OP_BRIGHTNESS, &[20]);
    assert!(!rig.is_on());
    rig.command(OP_POWER, &[1]);
    assert_eq!((rig.is_on(), rig.level()), (true, 40));
}

#[test]
fn commands_need_a_login_the_right_key_and_a_new_number() {
    let mut rig = connected(DIMMABLE);
    // not logged in: the client's key is anything
    rig.key = Some([7; 16]);
    rig.command(OP_POWER, &[0]);
    assert!(rig.is_on(), "no command before the login");
    assert!(rig.login(b"unpaired", b"1234"));
    let good = rig.key;
    rig.key = Some([7; 16]);
    rig.command(OP_POWER, &[0]);
    assert!(rig.is_on(), "a command under another key is ignored");
    rig.key = good;
    // one bit of the checksum wrong
    let mut damaged = rig.command_packet(0xffff, OP_POWER, &[0]);
    damaged[3] ^= 0x01;
    rig.write(COMMAND, &damaged);
    rig.run(300);
    assert!(rig.is_on(), "a command with a wrong checksum is ignored");
    let packet = rig.command_packet(0xffff, OP_POWER, &[0]);
    rig.write(COMMAND, &packet);
    rig.run(300);
    assert!(!rig.is_on());
    rig.command(OP_POWER, &[1]);
    // the same packet again (same sequence number) is a repeat
    let again = rig.command_packet(0xffff, OP_POWER, &[0]);
    rig.write(COMMAND, &again);
    rig.write(COMMAND, &again);
    rig.run(300);
    assert!(!rig.is_on());
    rig.bulb.handle(bulb_core::Command::On);
    rig.write(COMMAND, &again);
    rig.run(300);
    assert!(rig.is_on(), "a repeated packet does nothing");
}

#[test]
fn commands_are_addressed() {
    let mut rig = logged_in(DIMMABLE);
    // the bulb's address is the low byte of its address until the app assigns one
    for (dst, taken) in [(0x0000u16, true), (0x0033, true), (0x0034, false), (0x8001, false), (0xffff, true)] {
        rig.bulb.handle(bulb_core::Command::On);
        let packet = rig.command_packet(dst, OP_POWER, &[0]);
        rig.write(COMMAND, &packet);
        rig.run(300);
        assert_eq!(!rig.is_on(), taken, "destination {dst:#06x}");
    }
    // 0xe0 assigns an address, kept in flash
    rig.command(0xe0, &[0x21, 0x00]);
    rig.bulb.handle(bulb_core::Command::On);
    let packet = rig.command_packet(0x0021, OP_POWER, &[0]);
    rig.write(COMMAND, &packet);
    rig.run(300);
    assert!(!rig.is_on());
    assert_eq!(rig.world.borrow().nv_writes, 1);
}

#[test]
fn colour_temperature_on_the_tunable_bulb_only() {
    let mut rig = logged_in(TUNABLE);
    rig.command(OP_TEMPERATURE, &[0]);
    assert_eq!(rig.bulb.state().color_temp, 153, "0 is the coolest");
    assert_eq!(rig.statuses().last().unwrap().1[4], 0);
    rig.command(OP_TEMPERATURE, &[127]);
    assert_eq!(rig.bulb.state().color_temp, 454);
    assert_eq!(rig.statuses().last().unwrap().1[4], 127);
    rig.command(OP_TEMPERATURE, &[64]);
    assert_eq!(rig.bulb.state().color_temp, 153 + 301 * 64 / 127);
    assert_eq!(rig.statuses().last().unwrap().1[4], 64);

    let mut rig = logged_in(DIMMABLE);
    let before = rig.bulb.state();
    rig.command(OP_TEMPERATURE, &[0]);
    assert_eq!(rig.bulb.state(), before);
}

#[test]
fn the_stock_block_format_and_the_apps_zcl_tunnel_are_understood() {
    let mut rig = logged_in(TUNABLE);
    let key = rig.key.unwrap();
    let send = |rig: &mut Rig, kind: u8, payload: &[u8]| {
        rig.write(COMMAND, &block_packet(&key, kind, payload));
        rig.run(400);
    };
    // the mesh's own opcodes in a block
    send(&mut rig, 2, &[OP_POWER, 0x60, 0x01, 0]);
    assert!(!rig.is_on());
    send(&mut rig, 2, &[OP_POWER, 0x60, 0x01, 1]);
    assert!(rig.is_on());
    // ZCL: On/Off, Level (Move to Level with On/Off), Color Control (Move to Color Temperature)
    send(&mut rig, 0, &[0xff, 0xff, 0x01, 0x06, 0x00, 0]);
    assert!(!rig.is_on());
    send(&mut rig, 0, &[0x00, 0x00, 0x01, 0x06, 0x00, 1]);
    assert!(rig.is_on());
    send(&mut rig, 0, &[0x00, 0x33, 0x01, 0x08, 0x00, 0x04, 64, 0, 0]);
    assert_eq!(rig.level(), 64);
    send(&mut rig, 0, &[0xff, 0xff, 0x01, 0x00, 0x03, 0x0a, 0x2c, 0x01, 0, 0]);
    assert_eq!(rig.bulb.state().color_temp, 300);
    // another bulb's address, and a group that is not "everyone"
    send(&mut rig, 0, &[0x00, 0x34, 0x01, 0x06, 0x00, 0]);
    send(&mut rig, 1, &[0x80, 0x01, 0x01, 0x06, 0x00, 0]);
    assert!(rig.is_on());
    send(&mut rig, 1, &[0xff, 0xff, 0x01, 0x06, 0x00, 0]);
    assert!(!rig.is_on());
    // the app's status request: answered with the app's own status (`the_app_reads_its_own_status`)
    rig.statuses();
    send(&mut rig, 2, &[0x0a, 0, 0]);
    let (s, other) = rig.app_statuses();
    assert_eq!((s.len(), other), (1, 0));
    // a block under another key is not a command
    rig.write(COMMAND, &block_packet(&[9; 16], 0, &[0xff, 0xff, 0x01, 0x06, 0x00, 1]));
    rig.run(300);
    assert!(!rig.is_on());
    // nor is one whose inner checksum is wrong
    let mut plain = [0u8; 16];
    plain[1] = 6;
    plain[2..8].copy_from_slice(&[0xff, 0xff, 0x01, 0x06, 0x00, 1]);
    plain[0] = crc8_6c(&plain[1..8]) ^ 0x80;
    rig.write(COMMAND, &[&[0u8][..], &tl_aes_encrypt(&key, &plain)].concat());
    rig.run(300);
    assert!(!rig.is_on());
}

#[test]
fn pairing_sets_a_new_network_that_survives_a_restart() {
    let mut rig = logged_in(DIMMABLE);
    let key = rig.key.unwrap();
    let pad = |s: &[u8]| {
        let mut b = [0u8; 16];
        b[..s.len()].copy_from_slice(s);
        b
    };
    for (op, value) in [(4u8, &b"EgloTest"[..]), (5, b"s3cret"), (6, b"0123456789abcdef")] {
        assert_ne!(rig.read(PAIR)[0], 0x07);
        rig.write(PAIR, &[&[op][..], &tl_aes_encrypt(&key, &pad(value))].concat());
    }
    assert_eq!(rig.read(PAIR)[0], 0x07, "all three parts arrived");
    assert_eq!(rig.read(0x03), b"EgloTest", "the device name is the network name");

    rig.central().terminate();
    rig.run(300);
    rig.central().advertisements.clear();
    rig.central().scan = true;
    rig.run(400);
    {
        let c = rig.central();
        let adv = c.advertisements.last().unwrap();
        assert_eq!(adv[15..17], [9, 0x09]);
        assert_eq!(&adv[17..25], b"EgloTest", "advertised under the new name");
        assert_eq!(&c.scan_responses.last().unwrap()[10..], b"EgloTest");
    }
    rig.connect();
    assert!(!rig.login(b"unpaired", &Rig::secure_password()), "the factory password is gone");
    assert!(!rig.login(b"unpaired", b"1234"));
    assert!(rig.login(b"EgloTest", b"s3cret"));
    rig.command(OP_POWER, &[0]);
    assert!(!rig.is_on());

    let mut rig = rig.restart();
    rig.connect();
    assert!(rig.login(b"EgloTest", b"s3cret"), "the network is in flash");
    assert!(!rig.login(b"unpaired", b"1234"));
}

#[test]
fn a_mesh_reset_returns_to_the_factory_network_and_disconnects() {
    let mut rig = logged_in(DIMMABLE);
    let key = rig.key.unwrap();
    for (op, value) in [(4u8, *b"EgloTest\0\0\0\0\0\0\0\0"), (5, *b"s3cret\0\0\0\0\0\0\0\0\0\0"), (6, *b"0123456789abcdef")] {
        rig.write(PAIR, &[&[op][..], &tl_aes_encrypt(&key, &value)].concat());
    }
    let packet = rig.command_packet(0xffff, OP_RESET, &[0]);
    rig.write(COMMAND, &packet);
    rig.run(1000);
    assert!(rig.central().ended, "the bulb ended the connection itself");
    assert_eq!(rig.state(), LinkState::Advertising);
    rig.central().connected = false;
    rig.connect();
    assert_eq!(rig.read(0x03), b"unpaired");
    assert!(rig.login(b"unpaired", &Rig::secure_password()));
}

#[test]
fn nothing_is_notified_until_the_app_asks() {
    let mut rig = connected(DIMMABLE);
    assert!(rig.login(b"unpaired", b"1234"));
    rig.command(OP_POWER, &[0]);
    rig.command(OP_STATUS, &[0x10]);
    assert!(!rig.is_on());
    assert!(rig.central().notifications.is_empty());
    rig.enable_status();
    rig.run(400);
    assert_eq!(rig.statuses().len(), 1);
}

#[test]
fn notifications_can_be_enabled_the_standard_way_alone() {
    let mut rig = connected(DIMMABLE);
    assert!(rig.login(b"unpaired", b"1234"));
    // a generic client: only the client characteristic configuration
    rig.write(0x13, &[1, 0]);
    rig.run(400);
    assert_eq!(rig.statuses().len(), 1);
    // the app's switch on top changes nothing
    rig.write(0x12, &[1]);
    rig.run(400);
    assert!(rig.statuses().is_empty());
    rig.command(OP_POWER, &[0]);
    assert_eq!(rig.statuses().last().unwrap().1[2], 0);
    // both off again
    rig.write(0x13, &[0, 0]);
    rig.write(0x12, &[0]);
    rig.command(OP_POWER, &[1]);
    assert!(rig.central().notifications.is_empty());
    // the app's way alone (as on the stock)
    rig.write(0x12, &[1]);
    rig.run(400);
    assert_eq!(rig.statuses().len(), 1);
}

#[test]
fn an_l2cap_header_split_over_two_packets_is_put_together() {
    let mut rig = connected(DIMMABLE);
    // Read Request for handle 0x0b: 03 00 04 00 | 0a 0b 00, cut inside the header
    rig.central().send_fragments(&[&[0x03, 0x00, 0x04][..], &[0x00, 0x0a, 0x0b, 0x00]]);
    rig.run(300);
    assert_eq!(rig.central().att.pop_front().unwrap(), b"\x0bAwoX");
}

#[test]
fn a_frame_that_does_not_match_its_length_is_discarded() {
    let mut rig = connected(DIMMABLE);
    // Read Request for handle 0x0b with six octets more than the Length field (Vol 3 Part A 7.2.2)
    rig.central().send_fragments(&[&[0x03, 0x00, 0x04, 0x00, 0x0a][..], &[0x0b, 0x00, 0x0a, 0x03, 0x00, 0, 0, 0, 0]]);
    rig.run(300);
    assert!(rig.central().att.is_empty(), "surplus octets in the continuation");
    rig.central().send_fragments(&[&[0x03, 0x00, 0x04, 0x00, 0x0a, 0x0b, 0x00, 0x00][..]]);
    rig.run(300);
    assert!(rig.central().att.is_empty(), "surplus octets in the start fragment");
    // too few octets, then a new frame: only the new one counts
    rig.central().send_fragments(&[&[0x03, 0x00, 0x04, 0x00, 0x0a][..]]);
    assert_eq!(rig.att(&[0x0a, 0x0b, 0x00]), b"\x0bAwoX");
    rig.run(300);
    assert!(rig.central().att.is_empty());
}

#[test]
fn a_signalling_packet_over_the_signalling_mtu_is_rejected() {
    let mut rig = connected(DIMMABLE);
    // 124 octets: code 0x1c, identifier 0x61, 120 octets of data (Vol 3 Part A 4)
    let mut command = vec![0x1c, 0x61, 120, 0x00];
    command.extend(0..120u8);
    rig.central().send_l2cap(5, &command);
    rig.run(300);
    // Command Reject, "signalling MTU exceeded", MTUsig 64
    assert_eq!(rig.central().signaling, [vec![0x01, 0x61, 0x04, 0x00, 0x01, 0x00, 64, 0]]);
    // one that fits is understood (or not) as a whole
    rig.central().signaling.clear();
    let mut command = vec![0x1c, 0x62, 60, 0x00];
    command.extend(0..60u8);
    rig.central().send_l2cap(5, &command);
    rig.run(300);
    assert_eq!(rig.central().signaling, [vec![0x01, 0x62, 0x02, 0x00, 0x00, 0x00]]);
    // an attribute request of that size is too long for our MTU: dropped, and nothing else lost
    let mut request = vec![0x12, 0x15, 0x00];
    request.extend([0u8; 100]);
    rig.central().send_att(&request);
    rig.run(300);
    assert!(rig.central().att.is_empty());
    assert_eq!(rig.att(&[0x0a, 0x0b, 0x00]), b"\x0bAwoX");
}

#[test]
fn pairing_needs_a_login() {
    let mut rig = connected(DIMMABLE);
    // encrypted with the key of no session
    for (op, value) in [(4u8, *b"EgloTest\0\0\0\0\0\0\0\0"), (5, *b"s3cret\0\0\0\0\0\0\0\0\0\0"), (6, *b"0123456789abcdef")] {
        rig.write(PAIR, &[&[op][..], &tl_aes_encrypt(&[0; 16], &value)].concat());
    }
    assert_ne!(rig.read(PAIR)[0], 0x07);
    assert_eq!(rig.read(0x03), b"unpaired");
    assert_eq!(rig.world.borrow().nv_writes, 0);
}

#[test]
fn a_new_connection_starts_logged_out() {
    let mut rig = logged_in(DIMMABLE);
    rig.central().terminate();
    rig.run(300);
    rig.connect();
    // the old session key no longer works
    rig.command(OP_POWER, &[0]);
    assert!(rig.is_on());
    rig.run(600);
    assert!(rig.central().notifications.is_empty(), "no status without a login");
    assert_eq!(rig.read(STATUS).len(), 20);
}

/// The status the AwoX HomeControl app reads, as the stock firmware sends it
/// (`emulator/rs/scenarios/src/sims/app_colour.rs` recorded it from the stock
/// images of the dimmable, tunable and colour lamps; docs/stock-colour.md).
fn app_status(product: u16, flags: u8, level: u8, mireds: u16, hue: u8, sat: u8) -> [u8; 19] {
    let mut d = [0u8; 19];
    d[2] = 0x01;
    d[5..7].copy_from_slice(&product.to_le_bytes());
    d[7..11].copy_from_slice(&ADDRESS[..4]);
    d[11] = 0x01;
    d[12] = flags;
    d[13] = level;
    d[14..16].copy_from_slice(&mireds.to_le_bytes());
    (d[16], d[17], d[18]) = (hue, sat, 0x20);
    d
}

#[test]
fn the_app_reads_its_own_status() {
    for (identity, mireds) in [(DIMMABLE, 0xFFFF), (TUNABLE, 333)] {
        let mut rig = logged_in(identity);
        let key = rig.key.unwrap();
        rig.statuses();
        rig.write(COMMAND, &block_packet(&key, 2, &[0x0a, 0, 0]));
        rig.run(400);
        let (s, other) = rig.app_statuses();
        assert_eq!(s, vec![app_status(identity.product, 0x01, 254, mireds, 0xFF, 0xFF)], "{}", identity.model);
        assert_eq!(other, 0);
        // every command is answered with it, and nothing of the mesh's own comes to this central
        rig.write(COMMAND, &block_packet(&key, 0, &[0x00, 0x00, 0x01, 0x06, 0x00, 0]));
        rig.run(400);
        let (s, other) = rig.app_statuses();
        assert_eq!((s.last().map(|d| d[12]), other), (Some(0x00), 0), "off: bit 0 clear");
        assert_eq!(s.last().unwrap()[13], 254, "the level it will come on with");
    }
}

#[test]
fn the_apps_colour_its_animation_and_its_status() {
    let mut rig = logged_in(COLOUR);
    let key = rig.key.unwrap();
    let send = |rig: &mut Rig, kind: u8, payload: &[u8], ms: u64| {
        rig.write(COMMAND, &block_packet(&key, kind, payload));
        rig.run(ms);
    };
    let tunnel = |cluster: u16, zcl: &[u8]| [&[0x00, 0x00, 0x01, cluster as u8, (cluster >> 8) as u8][..], zcl].concat();
    send(&mut rig, 2, &[0x0a, 0, 0], 400);
    assert_eq!(rig.app_statuses().0, vec![app_status(0x2096, 0x01, 254, 333, 0xFF, 0xFF)], "white, as a new bulb");
    // colour as the app sends it: Move to Hue and Saturation (h * 254 / 360, s * 254), 0.2 s
    send(&mut rig, 0, &tunnel(0x0300, &[0x06, 84, 254, 2, 0]), 600);
    let (s, other) = rig.app_statuses();
    assert_eq!((s.last().copied(), other), (Some(app_status(0x2096, 0x03, 254, 0xFFFF, 84, 254)), 0));
    // the colour brightness is the level
    send(&mut rig, 0, &tunnel(0x0008, &[0x04, 128, 2, 0, 0, 0, 0]), 600);
    assert_eq!(rig.app_statuses().0.last().unwrap()[13], 128);
    // the animation: Enhanced Move Hue up at 0x0C00 a second; said when it starts, not as it turns
    send(&mut rig, 0, &tunnel(0x0300, &[0x41, 1, 0x00, 0x0C]), 400);
    let (s, _) = rig.app_statuses();
    assert_eq!(s.last().unwrap()[12], 0x07, "on, colour, turning");
    rig.run(5000);
    assert_eq!(rig.app_statuses().0.len(), 0, "a turning hue is not news");
    send(&mut rig, 2, &[0x0a, 0, 0], 400);
    let (s, _) = rig.app_statuses();
    let hue = s[0][16];
    assert!((140..=150).contains(&hue), "asked, it says where the hue is: {hue}");
    // the app stops it with Stop Move Step
    send(&mut rig, 0, &tunnel(0x0300, &[0x47, 0, 0, 0]), 400);
    let (s, _) = rig.app_statuses();
    assert_eq!(s.last().unwrap()[12], 0x03, "stopped");
    rig.run(2000);
    send(&mut rig, 2, &[0x0a, 0, 0], 400);
    assert_eq!(rig.app_statuses().0[0][16], s.last().unwrap()[16], "and stays where it stopped");
    // xy shows no hue and saturation; white shows its mireds
    send(&mut rig, 0, &tunnel(0x0300, &[0x07, 0xd7, 0xa3, 0x7b, 0x54, 2, 0]), 600);
    assert_eq!(rig.app_statuses().0.last().unwrap()[12..18], [0x01, 128, 0xFF, 0xFF, 0xFF, 0xFF]);
    send(&mut rig, 0, &tunnel(0x0300, &[0x0a, 0x2f, 0x01, 2, 0]), 600);
    assert_eq!(rig.app_statuses().0.last().unwrap()[12..18], [0x01, 128, 0x2f, 0x01, 0xFF, 0xFF]);
    // the candle mode (Level command 0x10) is not a level; Toggle is
    send(&mut rig, 0, &tunnel(0x0008, &[0x10, 1, 1]), 400);
    assert_eq!(rig.level(), 128);
    send(&mut rig, 0, &tunnel(0x0006, &[0x02]), 400);
    assert!(!rig.is_on());
    assert_eq!(rig.app_statuses().1, 0, "no mesh notification to the app at all");
}
