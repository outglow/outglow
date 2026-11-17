
use ble::crypto::{crc8_6c, tl_aes_encrypt};
use ble::LinkState;
use crate::sim::{Rig, ADDRESS, COLOUR, DIMMABLE, TUNABLE};

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
    rig.write(PAIR, &hex("0c0102030405060708df7376b1f81f686e"));
    assert_eq!(rig.read(PAIR)[0], 0x0d);
    rig.write(PAIR, &hex("0c010203040506070801a3567c6b987afd"));
    assert_eq!(rig.read(PAIR)[0], 0x0d);
}

#[test]
fn services_and_characteristics_sit_at_the_stock_handles() {
    let mut rig = connected(DIMMABLE);
    let rsp = rig.att(&[0x10, 0x01, 0x00, 0xff, 0xff, 0x00, 0x28]);
    assert_eq!(rsp, [0x11, 6, 0x01, 0x00, 0x06, 0x00, 0x00, 0x18, 0x07, 0x00, 0x0f, 0x00, 0x0a, 0x18]);
    let rsp = rig.att(&[0x10, 0x10, 0x00, 0xff, 0xff, 0x00, 0x28]);
    assert_eq!(rsp[..6], [0x11, 20, 0x10, 0x00, 0x1f, 0x00]);
    assert_eq!(rsp[6..], hex("10190d0c0b0a09080706050403020100"));
    let rsp = rig.att(&[0x10, 0x20, 0x00, 0xff, 0xff, 0x00, 0x28]);
    assert_eq!(rsp[..6], [0x11, 20, 0x20, 0x00, 0x2e, 0x00], "the diagnostic service after the mesh service");
    assert_eq!(rsp[6..], hex("6f5e4d3c2b1a2f8e3d4c4b5a000010e6"));
    let rsp = rig.att(&[0x10, 0x2f, 0x00, 0xff, 0xff, 0x00, 0x28]);
    assert_eq!(rsp, [0x01, 0x10, 0x2f, 0x00, 0x0a], "nothing after it");

    let mut found = vec![];
    let mut start = 0x10u16;
    loop {
        let mut req = vec![0x08];
        req.extend_from_slice(&start.to_le_bytes());
        req.extend_from_slice(&[0x1f, 0x00, 0x03, 0x28]);
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

    assert_eq!(rig.att(&[0x04, 0x13, 0x00, 0x13, 0x00]), [0x05, 0x01, 0x13, 0x00, 0x02, 0x29]);
    assert_eq!(rig.read(0x16), b"cmd");
    assert_eq!(rig.read(0x1c), b"OTA2");
    let rsp = rig.att(&[0x04, 0x01, 0x00, 0xff, 0xff]);
    assert_eq!(rsp[..2], [0x05, 0x01]);
    assert_eq!(rsp.len(), 2 + 5 * 4, "five 16-bit entries fit the default MTU");
    let rsp = rig.att(&[0x04, 0x12, 0x00, 0xff, 0xff]);
    assert_eq!(rsp[..4], [0x05, 0x02, 0x12, 0x00], "a 128-bit entry goes alone");
    assert_eq!(rsp.len(), 2 + 18);
    assert_eq!(rig.att(&[0x04, 0x2f, 0x00, 0xff, 0xff]), [0x01, 0x04, 0x2f, 0x00, 0x0a]);
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
    assert_eq!(rig.att(&[0x08, 0x01, 0x00, 0xff, 0xff, 0x00, 0x2a]), [&[0x09, 10, 0x03, 0x00][..], b"unpaired"].concat());
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
    rig.central().send_att(&[0x52, 0x0b, 0x00, 0x41]);
    rig.run(300);
    assert!(rig.central().att.is_empty());
}

#[test]
fn requests_of_the_wrong_length_are_invalid() {
    let mut rig = connected(DIMMABLE);
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
    for n in [3, 4, 15, 17] {
        for opcode in [0x08, 0x10] {
            let mut request = vec![opcode, 0x01, 0x00, 0xff, 0xff, 0x00, 0x28];
            request.resize(5 + n, 0);
            assert_eq!(rig.att(&request), [0x01, opcode, 0x00, 0x00, 0x04], "{opcode:#04x} with a type of {n} octets");
        }
    }
    assert_eq!(rig.att(&[0x0a, 0x0b, 0x00]), b"\x0bAwoX");
    assert_eq!(rig.att(&[0x02, 0x17, 0x00]), [0x03, 64, 0]);
}

#[test]
fn a_larger_mtu_gives_longer_responses_in_fragments() {
    let mut rig = connected(DIMMABLE);
    assert_eq!(rig.att(&[0x02, 247, 0]), [0x03, 64, 0]);
    let rsp = rig.att(&[0x08, 0x07, 0x00, 0x0f, 0x00, 0x03, 0x28]);
    assert_eq!(rsp.len(), 2 + 4 * 7);
    let rsp = rig.att(&[0x04, 0x01, 0x00, 0xff, 0xff]);
    assert_eq!(rsp.len(), 2 + 15 * 4);
    assert_eq!(rig.att(&[0x04, 0x0f, 0x00, 0xff, 0xff]).len(), 2 + 3 * 4);
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
    assert_eq!((s[0].0, &s[0].1[..3]), (0xdb, &[1, 127, 64][..]));

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
    rig.key = Some([7; 16]);
    rig.command(OP_POWER, &[0]);
    assert!(rig.is_on(), "no command before the login");
    assert!(rig.login(b"unpaired", b"1234"));
    let good = rig.key;
    rig.key = Some([7; 16]);
    rig.command(OP_POWER, &[0]);
    assert!(rig.is_on(), "a command under another key is ignored");
    rig.key = good;
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
    for (dst, taken) in [(0x0000u16, true), (0x0033, true), (0x0034, false), (0x8001, false), (0xffff, true)] {
        rig.bulb.handle(bulb_core::Command::On);
        let packet = rig.command_packet(dst, OP_POWER, &[0]);
        rig.write(COMMAND, &packet);
        rig.run(300);
        assert_eq!(!rig.is_on(), taken, "destination {dst:#06x}");
    }
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
    send(&mut rig, 2, &[OP_POWER, 0x60, 0x01, 0]);
    assert!(!rig.is_on());
    send(&mut rig, 2, &[OP_POWER, 0x60, 0x01, 1]);
    assert!(rig.is_on());
    send(&mut rig, 0, &[0xff, 0xff, 0x01, 0x06, 0x00, 0]);
    assert!(!rig.is_on());
    send(&mut rig, 0, &[0x00, 0x00, 0x01, 0x06, 0x00, 1]);
    assert!(rig.is_on());
    send(&mut rig, 0, &[0x00, 0x33, 0x01, 0x08, 0x00, 0x04, 64, 0, 0]);
    assert_eq!(rig.level(), 64);
    send(&mut rig, 0, &[0xff, 0xff, 0x01, 0x00, 0x03, 0x0a, 0x2c, 0x01, 0, 0]);
    assert_eq!(rig.bulb.state().color_temp, 300);
    send(&mut rig, 0, &[0x00, 0x34, 0x01, 0x06, 0x00, 0]);
    send(&mut rig, 1, &[0x80, 0x01, 0x01, 0x06, 0x00, 0]);
    assert!(rig.is_on());
    send(&mut rig, 1, &[0xff, 0xff, 0x01, 0x06, 0x00, 0]);
    assert!(!rig.is_on());
    rig.statuses();
    send(&mut rig, 2, &[0x0a, 0, 0]);
    let (s, other) = rig.app_statuses();
    assert_eq!((s.len(), other), (1, 0));
    rig.write(COMMAND, &block_packet(&[9; 16], 0, &[0xff, 0xff, 0x01, 0x06, 0x00, 1]));
    rig.run(300);
    assert!(!rig.is_on());
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
    assert_ne!(rig.read(PAIR)[0], 0x07);
    rig.write(PAIR, &[&[4u8][..], &tl_aes_encrypt(&key, &pad(b"EgloTest"))].concat());
    assert_ne!(rig.read(PAIR)[0], 0x07, "the name alone is not a network");
    rig.write(PAIR, &[&[5u8][..], &tl_aes_encrypt(&key, &pad(b"s3cret"))].concat());
    assert_eq!(rig.read(PAIR)[0], 0x07, "name and password arrived");
    rig.write(PAIR, &[&[6u8][..], &tl_aes_encrypt(&key, &pad(b"0123456789abcdef"))].concat());
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
        assert_eq!(c.scan_responses.last().unwrap()[9], 0xff);
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
    rig.write(0x13, &[1, 0]);
    rig.run(400);
    assert_eq!(rig.statuses().len(), 1);
    rig.write(0x12, &[1]);
    rig.run(400);
    assert!(rig.statuses().is_empty());
    rig.command(OP_POWER, &[0]);
    assert_eq!(rig.statuses().last().unwrap().1[2], 0);
    rig.write(0x13, &[0, 0]);
    rig.write(0x12, &[0]);
    rig.command(OP_POWER, &[1]);
    assert!(rig.central().notifications.is_empty());
    rig.write(0x12, &[1]);
    rig.run(400);
    assert_eq!(rig.statuses().len(), 1);
}

#[test]
fn the_client_configuration_keeps_only_the_bits_the_characteristic_has() {
    let mut rig = connected(DIMMABLE);
    assert!(rig.login(b"unpaired", b"1234"));
    for (written, kept) in [([2u8, 0u8], [0u8, 0u8]), ([3, 0], [1, 0]), ([0xFF, 0xFF], [1, 0]), ([1, 0], [1, 0]), ([0, 0], [0, 0])] {
        rig.write(0x13, &written);
        assert_eq!(rig.read(0x13), kept, "{written:02x?}");
    }
    rig.run(400);
    rig.statuses();
    rig.write(0x13, &[2, 0]);
    rig.command(OP_POWER, &[0]);
    rig.run(400);
    assert!(rig.central().notifications.is_empty());
    rig.write(0x13, &[3, 0]);
    rig.run(400);
    assert_eq!(rig.statuses().len(), 1);
}

#[test]
fn an_l2cap_header_split_over_two_packets_is_put_together() {
    let mut rig = connected(DIMMABLE);
    rig.central().send_fragments(&[&[0x03, 0x00, 0x04][..], &[0x00, 0x0a, 0x0b, 0x00]]);
    rig.run(300);
    assert_eq!(rig.central().att.pop_front().unwrap(), b"\x0bAwoX");
}

#[test]
fn a_frame_that_does_not_match_its_length_is_discarded() {
    let mut rig = connected(DIMMABLE);
    rig.central().send_fragments(&[&[0x03, 0x00, 0x04, 0x00, 0x0a][..], &[0x0b, 0x00, 0x0a, 0x03, 0x00, 0, 0, 0, 0]]);
    rig.run(300);
    assert!(rig.central().att.is_empty(), "surplus octets in the continuation");
    rig.central().send_fragments(&[&[0x03, 0x00, 0x04, 0x00, 0x0a, 0x0b, 0x00, 0x00][..]]);
    rig.run(300);
    assert!(rig.central().att.is_empty(), "surplus octets in the start fragment");
    rig.central().send_fragments(&[&[0x03, 0x00, 0x04, 0x00, 0x0a][..]]);
    assert_eq!(rig.att(&[0x0a, 0x0b, 0x00]), b"\x0bAwoX");
    rig.run(300);
    assert!(rig.central().att.is_empty());
}

#[test]
fn a_signalling_packet_over_the_signalling_mtu_is_rejected() {
    let mut rig = connected(DIMMABLE);
    let mut command = vec![0x1c, 0x61, 120, 0x00];
    command.extend(0..120u8);
    rig.central().send_l2cap(5, &command);
    rig.run(300);
    assert_eq!(rig.central().signaling, [vec![0x01, 0x61, 0x04, 0x00, 0x01, 0x00, 64, 0]]);
    rig.central().signaling.clear();
    let mut command = vec![0x1c, 0x62, 60, 0x00];
    command.extend(0..60u8);
    rig.central().send_l2cap(5, &command);
    rig.run(300);
    assert_eq!(rig.central().signaling, [vec![0x01, 0x62, 0x02, 0x00, 0x00, 0x00]]);
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
    rig.command(OP_POWER, &[0]);
    assert!(rig.is_on());
    rig.run(600);
    assert!(rig.central().notifications.is_empty(), "no status without a login");
    assert_eq!(rig.read(STATUS).len(), 20);
}

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
    send(&mut rig, 0, &tunnel(0x0300, &[0x06, 84, 254, 2, 0]), 600);
    let (s, other) = rig.app_statuses();
    assert_eq!((s.last().copied(), other), (Some(app_status(0x2096, 0x03, 254, 0xFFFF, 84, 254)), 0));
    send(&mut rig, 0, &tunnel(0x0008, &[0x04, 128, 2, 0, 0, 0, 0]), 600);
    assert_eq!(rig.app_statuses().0.last().unwrap()[13], 128);
    send(&mut rig, 0, &tunnel(0x0300, &[0x41, 1, 0x00, 0x0C]), 400);
    let (s, _) = rig.app_statuses();
    assert_eq!(s.last().unwrap()[12], 0x07, "on, colour, turning");
    rig.run(5000);
    assert_eq!(rig.app_statuses().0.len(), 0, "a turning hue is not news");
    send(&mut rig, 2, &[0x0a, 0, 0], 400);
    let (s, _) = rig.app_statuses();
    let hue = s[0][16];
    assert!((140..=150).contains(&hue), "asked, it says where the hue is: {hue}");
    send(&mut rig, 0, &tunnel(0x0300, &[0x47, 0, 0, 0]), 400);
    let (s, _) = rig.app_statuses();
    assert_eq!(s.last().unwrap()[12], 0x03, "stopped");
    rig.run(2000);
    send(&mut rig, 2, &[0x0a, 0, 0], 400);
    assert_eq!(rig.app_statuses().0[0][16], s.last().unwrap()[16], "and stays where it stopped");
    send(&mut rig, 0, &tunnel(0x0300, &[0x07, 0xd7, 0xa3, 0x7b, 0x54, 2, 0]), 600);
    assert_eq!(rig.app_statuses().0.last().unwrap()[12..18], [0x01, 128, 0xFF, 0xFF, 0xFF, 0xFF]);
    send(&mut rig, 0, &tunnel(0x0300, &[0x0a, 0x2f, 0x01, 2, 0]), 600);
    assert_eq!(rig.app_statuses().0.last().unwrap()[12..18], [0x01, 128, 0x2f, 0x01, 0xFF, 0xFF]);
    send(&mut rig, 0, &tunnel(0x0008, &[0x10, 1, 1]), 400);
    assert_eq!(rig.level(), 128);
    send(&mut rig, 0, &tunnel(0x0006, &[0x02]), 400);
    assert!(!rig.is_on());
    assert_eq!(rig.app_statuses().1, 0, "no mesh notification to the app at all");
}

#[test]
fn the_apps_zigbee_installation_pairs_and_is_told_it_is_done() {
    let mut rig = logged_in(DIMMABLE);
    let key = rig.key.unwrap();
    rig.enable_status();
    let pad = |s: &[u8]| {
        let mut b = [0u8; 16];
        b[..s.len()].copy_from_slice(s);
        b
    };
    rig.write(PAIR, &[&[4u8][..], &tl_aes_encrypt(&key, &pad(b"ICCsTest"))].concat());
    rig.write(PAIR, &[&[5u8][..], &tl_aes_encrypt(&key, &pad(b"pw"))].concat());
    assert_eq!(rig.read(PAIR)[0], 0x07);
    let long = |payload: &[u8]| -> Vec<u8> {
        let mut whole = vec![payload.len() as u8];
        whole.extend_from_slice(payload);
        let mut plain = [0u8; 16];
        plain[0] = crc8_6c(&whole);
        let head = whole.len().min(15);
        plain[1..1 + head].copy_from_slice(&whole[..head]);
        let mut v = vec![2u8];
        v.extend_from_slice(&tl_aes_encrypt(&key, &plain));
        v.extend_from_slice(&whole[head..]);
        v
    };
    assert_eq!(rig.att(&[0x02, 247, 0]), [0x03, 64, 0]);
    rig.central().notifications.clear();
    let network = [0x03, 0x14, 0x31, 0x32, 0x36, 0x35, 0x34, 0x33, 0x32, 0x31, 0xc1, 0xa4, 0, 0, 0, 0, 0];
    rig.write(COMMAND, &long(&network));
    rig.run(200);
    let mut nwk_key = vec![0x04];
    nwk_key.extend_from_slice(&[0x5a; 16]);
    nwk_key.push(0);
    rig.write(COMMAND, &long(&nwk_key));
    rig.run(200);
    let all: Vec<Vec<u8>> = rig.central().notifications.clone();
    let done: Vec<_> = all.iter().filter(|n| n.len() >= 4 && n[3] == 0x83).collect();
    assert_eq!(done.len(), 2, "a 0x83 notification for each: {all:?}");
    assert!(done.iter().all(|n| n.len() >= 3 + 17), "17 octets and more after the handle");

    rig.central().notifications.clear();
    rig.write(COMMAND, &long(&[0x0a, 0x00, 0x00]));
    rig.run(600);
    let (statuses, _) = rig.app_statuses();
    assert!(!statuses.is_empty(), "a status in answer to the app's request");
    assert!(statuses.iter().all(|d| d[18] == 0x60), "provision state 0x60: {statuses:?}");
    rig.central().terminate();
    let mut rig = rig.restart();
    rig.central().scan = true;
    rig.run(400);
    let rsp = rig.central().scan_responses.last().unwrap().clone();
    assert_eq!(rsp[8..10], [0x1e, 0xff]);
    assert_eq!(rsp[10 + 20], 0x60, "provision state in the scan response");
}

#[test]
fn the_apps_status_comes_when_the_stock_firmware_s_does() {
    let mut rig = logged_in(COLOUR);
    let key = rig.key.unwrap();
    rig.write(COMMAND, &block_packet(&key, 2, &[0x0a, 0, 0]));
    rig.run(400);
    rig.app_statuses();
    rig.write(COMMAND, &block_packet(&key, 0, &[0x00, 0x00, 0x01, 0x08, 0x00, 0x04, 128, 2, 0, 0, 0, 0]));
    rig.run(60);
    assert_eq!(rig.app_statuses().0.len(), 0, "not yet");
    rig.run(60);
    let (s, _) = rig.app_statuses();
    assert_eq!(s.len(), 1);
    assert!((150..250).contains(&s[0][13]), "on its way: {}", s[0][13]);
    rig.run(600);
    let (s, other) = rig.app_statuses();
    assert_eq!((s.iter().map(|d| d[13]).collect::<Vec<_>>(), other), (vec![128], 0), "and where it ends");
    rig.run(3000);
    assert_eq!(rig.app_statuses().0.len(), 0, "then quiet");
}

#[test]
fn the_apps_candle_mode() {
    let mut rig = logged_in(COLOUR);
    let key = rig.key.unwrap();
    let tunnel = |zcl: &[u8]| block_packet(&key, 0, &[&[0x00, 0x00, 0x01, 0x08, 0x00][..], zcl].concat());
    rig.write(COMMAND, &tunnel(&[0x04, 128, 0, 0, 0, 0, 0]));
    rig.run(400);
    rig.app_statuses();
    rig.write(COMMAND, &tunnel(&[0x10, 1, 1]));
    rig.run(400);
    let (s, _) = rig.app_statuses();
    assert_eq!(s.last().unwrap()[12], 0x11, "on, candle");
    assert_eq!(s.last().unwrap()[13], 128, "the level is the light's own");
    assert!(rig.bulb.state().candle);
    rig.run(10_000);
    assert_eq!(rig.app_statuses().0.len(), 0, "a flicker is not news");
    rig.write(COMMAND, &tunnel(&[0x10, 0, 1]));
    rig.run(400);
    assert_eq!(rig.app_statuses().0.last().unwrap()[12], 0x01);
    assert!(!rig.bulb.state().candle);
}

fn base_uuid(short: u16) -> Vec<u8> {
    let mut u = vec![0xfb, 0x34, 0x9b, 0x5f, 0x80, 0x00, 0x00, 0x80, 0x00, 0x10, 0x00, 0x00];
    u.extend_from_slice(&short.to_le_bytes());
    u.extend_from_slice(&[0, 0]);
    u
}

#[test]
fn a_16_bit_type_asked_for_in_its_128_bit_form_is_found() {
    let mut rig = connected(DIMMABLE);
    let short = rig.att(&[0x10, 0x01, 0x00, 0xff, 0xff, 0x00, 0x28]);
    let long = rig.att(&[&[0x10, 0x01, 0x00, 0xff, 0xff][..], &base_uuid(0x2800)].concat());
    assert_eq!(long, short, "Read By Group Type of the primary service in the Bluetooth Base UUID");
    let rsp = rig.att(&[&[0x08, 0x01, 0x00, 0xff, 0xff][..], &base_uuid(0x2a00)].concat());
    assert_eq!(rsp, [&[0x09, 10, 0x03, 0x00][..], b"unpaired"].concat());
    let mut other_base = base_uuid(0x2a00);
    other_base[0] ^= 1;
    assert_eq!(rig.att(&[&[0x08, 0x01, 0x00, 0xff, 0xff][..], &other_base].concat()), [0x01, 0x08, 0x01, 0x00, 0x0a]);
    let mut other_short = base_uuid(0x2a00);
    other_short[14] = 1;
    assert_eq!(rig.att(&[&[0x08, 0x01, 0x00, 0xff, 0xff][..], &other_short].concat()), [0x01, 0x08, 0x01, 0x00, 0x0a]);
    let mut grouped = base_uuid(0x2800);
    grouped[0] ^= 1;
    assert_eq!(rig.att(&[&[0x10, 0x01, 0x00, 0xff, 0xff][..], &grouped].concat()), [0x01, 0x10, 0x01, 0x00, 0x10]);
    let long_status = [0x11, 0x19, 0x0d, 0x0c, 0x0b, 0x0a, 0x09, 0x08, 0x07, 0x06, 0x05, 0x04, 0x03, 0x02, 0x01, 0x00];
    assert_eq!(
        rig.att(&[&[0x10, 0x01, 0x00, 0xff, 0xff][..], &long_status].concat()),
        [0x01, 0x10, 0x01, 0x00, 0x10],
        "a 128-bit type is never the 16-bit primary service"
    );
    assert_eq!(rig.att(&[0x06, 0x01, 0x00, 0xff, 0xff, 0x00, 0x28, 0x11, 0x19]), [0x01, 0x06, 0x01, 0x00, 0x0a], "the first two octets of a 128-bit service are not its value");
}

#[test]
fn find_by_type_value_without_a_match_is_attribute_not_found() {
    let mut rig = connected(DIMMABLE);
    assert_eq!(rig.att(&[0x06, 0x01, 0x00, 0xff, 0xff, 0x00, 0x28, 0x0f, 0x18]), [0x01, 0x06, 0x01, 0x00, 0x0a]);
    assert_eq!(rig.att(&[0x06, 0x07, 0x00, 0xff, 0xff, 0x00, 0x28, 0x00, 0x18]), [0x01, 0x06, 0x07, 0x00, 0x0a], "the service before the range");
    assert_eq!(rig.att(&[0x06, 0x01, 0x00, 0xff, 0xff, 0x00, 0x28, 0x00, 0x18]), [0x07, 0x01, 0x00, 0x06, 0x00]);
    assert_eq!(rig.att(&[0x06, 0x03, 0x00, 0x03, 0x00, 0x00, 0x2a, b'u', b'n', b'p', b'a', b'i', b'r', b'e', b'd']), [0x07, 0x03, 0x00, 0x03, 0x00], "a value that is not a service is its own group");
}

#[test]
fn a_confirmation_needs_no_answer() {
    let mut rig = connected(DIMMABLE);
    rig.central().send_att(&[0x1e]);
    rig.run(300);
    assert!(rig.central().att.is_empty());
    assert_eq!(rig.att(&[0x0a, 0x0b, 0x00]), b"\x0bAwoX");
}

#[test]
fn a_continuation_without_its_start_is_dropped() {
    let mut rig = connected(DIMMABLE);
    rig.central().send_pdu(1, &[0x03, 0x00, 0x04, 0x00, 0x0a, 0x0b, 0x00]);
    rig.run(300);
    assert!(rig.central().att.is_empty());
    rig.central().send_fragments(&[&[0x03, 0x00, 0x04, 0x00, 0x0a][..], &[0x0b, 0x00]]);
    rig.run(300);
    assert_eq!(rig.central().att.pop_front().unwrap(), b"\x0bAwoX");
    rig.central().send_pdu(1, &[0x0b, 0x00]);
    rig.run(300);
    assert!(rig.central().att.is_empty(), "the frame was closed by its last fragment");
}

#[test]
fn signalling_shorter_than_its_header_is_dropped() {
    let mut rig = connected(DIMMABLE);
    rig.central().send_l2cap(5, &[0x1c, 0x61, 0x00]);
    rig.run(300);
    assert!(rig.central().signaling.is_empty());
    rig.central().send_l2cap(5, &[0x1c, 0x61, 0x00, 0x00]);
    rig.run(300);
    assert_eq!(rig.central().signaling, [vec![0x01, 0x61, 0x02, 0x00, 0x00, 0x00]]);
}

#[test]
fn the_command_characteristic_reads_back_the_last_command() {
    let mut rig = logged_in(DIMMABLE);
    assert_eq!(rig.read(COMMAND), [0; 20]);
    rig.command(OP_POWER, &[0]);
    let mut want = [0u8; 20];
    want[..5].copy_from_slice(&[0xff, 0xff, OP_POWER, 0x60, 0x01]);
    assert_eq!(rig.read(COMMAND), want, "the decrypted payload of a mesh command");
    let key = rig.key.unwrap();
    rig.write(COMMAND, &block_packet(&key, 2, &[OP_POWER, 0x60, 0x01, 1]));
    rig.run(300);
    let mut want = [0u8; 20];
    want[1..6].copy_from_slice(&[4, OP_POWER, 0x60, 0x01, 1]);
    want[0] = crc8_6c(&want[1..6]);
    assert_eq!(rig.read(COMMAND), want, "the decrypted block of a stock command");
}

#[test]
fn a_network_without_a_name_reads_its_device_name_as_one_zero() {
    let mut rig = logged_in(DIMMABLE);
    let key = rig.key.unwrap();
    rig.write(PAIR, &[&[4u8][..], &tl_aes_encrypt(&key, &[0; 16])].concat());
    rig.write(PAIR, &[&[5u8][..], &tl_aes_encrypt(&key, b"s3cret\0\0\0\0\0\0\0\0\0\0")].concat());
    assert_eq!(rig.read(PAIR)[0], 0x07);
    assert_eq!(rig.read(0x03), [0]);
}

#[test]
fn a_bulb_whose_address_ends_in_zero_is_device_one() {
    let mut address = ADDRESS;
    address[0] = 0;
    let mut rig = Rig::boot(Rig::world(address, None, 0), DIMMABLE);
    rig.connect();
    assert!(rig.login(b"unpaired", b"1234"));
    rig.enable_status();
    rig.run(400);
    assert_eq!(rig.statuses_from()[0].2, 0x0001, "the source of its status");
    let packet = rig.command_packet(0x0001, OP_POWER, &[0]);
    rig.write(COMMAND, &packet);
    rig.run(300);
    assert!(!rig.is_on());
}

#[test]
fn a_change_that_finds_the_transmit_queue_full_is_notified_later() {
    let mut rig = logged_in(DIMMABLE);
    rig.central().silent = true;
    for level in [10, 30, 50, 70, 90, 110, 130, 150] {
        rig.bulb.handle(bulb_core::Command::MoveToLevel { level, transition_ms: 0 });
        rig.run(300);
    }
    assert_eq!(rig.state(), LinkState::Connected);
    rig.central().silent = false;
    rig.run(600);
    let levels: Vec<u8> = rig.statuses().iter().map(|s| s.1[3]).collect();
    assert_eq!(levels, [5, 15, 25, 35, 45, 75], "five while the queue had room, then the level when it had room again");
}

#[test]
fn the_apps_status_waits_for_notifications() {
    let mut rig = connected(DIMMABLE);
    assert!(rig.login(b"unpaired", b"1234"));
    let key = rig.key.unwrap();
    rig.write(COMMAND, &block_packet(&key, 2, &[0x0a, 0, 0]));
    rig.run(400);
    rig.write(COMMAND, &block_packet(&key, 0, &[0x00, 0x00, 0x01, 0x06, 0x00, 0]));
    rig.run(400);
    assert!(!rig.is_on());
    assert!(rig.central().notifications.is_empty());
    rig.write(0x13, &[1, 0]);
    rig.run(400);
    let (s, other) = rig.app_statuses();
    assert_eq!((s.len(), other), (1, 0));
    assert_eq!(s[0][12], 0x00, "off");
    rig.write(COMMAND, &block_packet(&key, 2, &[OP_STATUS, 0x60, 0x01, 0x10]));
    rig.run(400);
    assert_eq!(rig.app_statuses().0.len(), 1, "a status request in the app's format");
}

#[test]
fn zcl_tunnel_commands_the_bulb_does_not_take_are_ignored() {
    let mut rig = logged_in(COLOUR);
    let key = rig.key.unwrap();
    let before = rig.bulb.state();
    for payload in [
        &[0x00, 0x00, 0x02, 0x06, 0x00, 0x00][..],
        &[0x00, 0x00, 0x01, 0x06][..],
        &[0x00, 0x00, 0x01, 0x06, 0x00, 0x40],
        &[0x00, 0x00, 0x01, 0x06, 0x00],
        &[0x00, 0x00, 0x01, 0x08, 0x00, 0x04],
        &[0x00, 0x00, 0x01, 0x08, 0x00, 0x05, 10],
        &[0x00, 0x00, 0x01, 0x04, 0x00, 0x00, 1],
        &[0x00, 0x00, 0x01, 0x00, 0x03, 0x41, 2, 1, 0],
        &[0x00, 0x00, 0x01, 0x00, 0x03, 0x41, 1, 1],
        &[0x00, 0x00, 0x01, 0x00, 0x03, 0x00, 10, 4, 0, 0],
        &[0x00, 0x00, 0x01, 0x00, 0x03, 0x00, 10],
        &[0x00, 0x00, 0x01, 0x00, 0x03, 0x03],
        &[0x00, 0x00, 0x01, 0x00, 0x03, 0x06, 10],
        &[0x00, 0x00, 0x01, 0x00, 0x03, 0x07, 1, 2, 3],
        &[0x00, 0x00, 0x01, 0x00, 0x03, 0x0a, 0x2c],
        &[0x00, 0x00, 0x01, 0x00, 0x03, 0x02, 10, 0, 0, 0],
        &[0x00, 0x00, 0x01, 0x00, 0x03],
    ] {
        rig.write(COMMAND, &block_packet(&key, 0, payload));
        rig.run(300);
        assert_eq!(rig.bulb.state(), before, "{payload:02x?}");
    }
}

#[test]
fn the_apps_hue_and_saturation_commands() {
    let mut rig = logged_in(COLOUR);
    let key = rig.key.unwrap();
    let tunnel = |zcl: &[u8]| block_packet(&key, 0, &[&[0x00, 0x00, 0x01, 0x00, 0x03][..], zcl].concat());
    rig.write(COMMAND, &tunnel(&[0x06, 84, 200, 0, 0]));
    rig.run(400);
    rig.write(COMMAND, &tunnel(&[0x00, 255, 0, 0, 0]));
    rig.run(400);
    assert_eq!((rig.bulb.state().hue, rig.bulb.state().sat), (254, 200), "Move to Hue keeps the saturation; 255 is 254");
    rig.write(COMMAND, &tunnel(&[0x03, 100, 0, 0]));
    rig.run(400);
    assert_eq!((rig.bulb.state().hue, rig.bulb.state().sat), (254, 100), "Move to Saturation keeps the hue");
    rig.write(COMMAND, &tunnel(&[0x41, 1, 0x00, 0x10]));
    rig.run(400);
    assert!(rig.bulb.state().hue_moving);
    rig.write(COMMAND, &tunnel(&[0x41, 0, 0x00, 0x00]));
    rig.run(400);
    assert!(!rig.bulb.state().hue_moving, "Enhanced Move Hue with mode stop");
    let mut rig = logged_in(TUNABLE);
    let key = rig.key.unwrap();
    let before = rig.bulb.state();
    rig.write(COMMAND, &block_packet(&key, 0, &[0x00, 0x00, 0x01, 0x00, 0x03, 0x06, 84, 200, 0, 0]));
    rig.run(400);
    assert_eq!(rig.bulb.state(), before, "no colour on a white bulb");
}

#[test]
fn command_packets_of_no_known_format_are_ignored() {
    let mut rig = logged_in(DIMMABLE);
    let key = rig.key.unwrap();
    let before = rig.bulb.state();
    let short = block_packet(&key, 2, &[OP_POWER, 0x60, 0x01, 0]);
    rig.write(COMMAND, &short[..16]);
    rig.write(COMMAND, &block_packet(&key, 2, &[0x55, 0, 0]));
    rig.run(300);
    assert_eq!(rig.bulb.state(), before);
    let mut want = [0u8; 20];
    want[1..5].copy_from_slice(&[3, 0x55, 0, 0]);
    want[0] = crc8_6c(&want[1..5]);
    assert_eq!(rig.read(COMMAND), want, "an unknown command of the stock format is taken and does nothing");
    rig.write(COMMAND, &block_packet(&key, 3, &[OP_POWER, 0x60, 0x01, 0]));
    rig.run(300);
    assert_eq!(rig.bulb.state(), before, "a block of kind 3 is not a command to this bulb");
}

#[test]
fn pair_writes_of_no_known_kind_change_nothing() {
    let mut rig = logged_in(DIMMABLE);
    let key = rig.key.unwrap();
    let name = tl_aes_encrypt(&key, b"EgloTest\0\0\0\0\0\0\0\0");
    let password = tl_aes_encrypt(&key, b"s3cret\0\0\0\0\0\0\0\0\0\0");
    rig.write(PAIR, &[&[4u8][..], &name].concat());
    for value in [[&[7u8][..], &password].concat(), [&[3u8][..], &password].concat(), [&[5u8][..], &password[..15]].concat(), vec![]] {
        rig.write(PAIR, &value);
        assert_eq!(rig.read(PAIR), [0; 20], "{value:02x?}");
    }
    assert_eq!(rig.read(0x03), b"unpaired");
    rig.write(PAIR, &[&[5u8][..], &password].concat());
    assert_eq!(rig.read(PAIR)[0], 0x07, "the name written before still counts");
}

#[test]
fn a_transition_is_notified_once_with_where_it_goes() {
    let mut rig = logged_in(DIMMABLE);
    rig.bulb.handle(bulb_core::Command::MoveToLevel { level: 100, transition_ms: 3000 });
    rig.run(4000);
    let s = rig.statuses();
    assert_eq!(s.iter().map(|s| s.1[3]).collect::<Vec<_>>(), [50], "{s:?}");
}

#[test]
fn a_blob_read_at_the_end_of_the_value_is_empty_and_a_range_from_handle_zero_is_invalid() {
    let mut rig = connected(DIMMABLE);
    assert_eq!(rig.att(&[0x0c, 0x0b, 0x00, 0x04, 0x00]), [0x0d]);
    assert_eq!(rig.att(&[0x0c, 0x0b, 0x00, 0x05, 0x00]), [0x01, 0x0c, 0x0b, 0x00, 0x07]);
    assert_eq!(rig.att(&[0x0c, 0x0b, 0x00, 0x00, 0x00]), b"\x0dAwoX");
    for opcode in [0x08, 0x10] {
        assert_eq!(rig.att(&[opcode, 0x00, 0x00, 0xff, 0xff, 0x00, 0x28]), [0x01, opcode, 0x00, 0x00, 0x01]);
    }
    assert_eq!(rig.att(&[0x04, 0x00, 0x00, 0xff, 0xff]), [0x01, 0x04, 0x00, 0x00, 0x01]);
    assert_eq!(rig.att(&[0x04, 0x05, 0x00, 0x05, 0x00]), [0x05, 0x01, 0x05, 0x00, 0x03, 0x28], "a range of one handle");
    assert_eq!(rig.att(&[0x04, 0x2e, 0x00, 0xff, 0xff]).len(), 2 + 18, "the last handle");
}

#[test]
fn the_scan_response_shows_the_mesh_address_the_light_and_a_colour_in_hue_and_saturation() {
    let mut rig = logged_in(COLOUR);
    let key = rig.key.unwrap();
    rig.write(COMMAND, &block_packet(&key, 0, &[0x00, 0x00, 0x01, 0x00, 0x03, 0x06, 84, 200, 0, 0]));
    rig.run(400);
    rig.write(COMMAND, &block_packet(&key, 0, &[0x00, 0x00, 0x01, 0x08, 0x00, 0x04, 90, 0, 0]));
    rig.run(400);
    rig.command(0xe0, &[0x21, 0x03]);
    rig.central().terminate();
    rig.run(300);
    rig.central().scan = true;
    rig.run(400);
    let rsp = rig.central().scan_responses.last().unwrap().clone();
    assert_eq!(rsp[8 + 13..8 + 24], [0x21, 0x03, 1, 90, 0xff, 84, 200, 0xff, 0x04, 0x3f, 0x20]);
    assert_eq!(rsp[8 + 4..8 + 6], 0x2096u16.to_le_bytes());
}

#[test]
fn read_by_type_stops_at_the_first_value_of_another_length() {
    let mut rig = connected(DIMMABLE);
    assert_eq!(rig.att(&[0x08, 0x0e, 0x00, 0x20, 0x00, 0x03, 0x28]), [0x09, 7, 0x0e, 0x00, 0x02, 0x0f, 0x00, 0x27, 0x2a]);
    let rsp = rig.att(&[0x08, 0x11, 0x00, 0x20, 0x00, 0x03, 0x28]);
    assert_eq!(rsp[..4], [0x09, 21, 0x11, 0x00], "one 128-bit declaration fills the default MTU");
    assert_eq!(rsp.len(), 2 + 21);
    assert_eq!(rig.att(&[0x02, 64, 0]), [0x03, 64, 0]);
    let rsp = rig.att(&[0x08, 0x11, 0x00, 0x20, 0x00, 0x03, 0x28]);
    assert_eq!((rsp[1], rsp.len()), (21, 2 + 2 * 21), "two in 64 octets, all of one length");
    assert_eq!(rsp[2 + 21..2 + 23], [0x14, 0x00]);
}

#[test]
fn power_0xee_is_not_a_command_and_brightness_and_temperature_are_clamped() {
    let mut rig = logged_in(TUNABLE);
    rig.command(OP_POWER, &[0xee]);
    assert!(rig.is_on(), "0xee is no power state");
    rig.command(OP_POWER, &[0x02]);
    assert!(rig.is_on(), "anything but 0 is on");
    rig.command(OP_BRIGHTNESS, &[0]);
    assert_eq!((rig.is_on(), rig.level()), (true, 1), "the dimmest, not off");
    rig.command(OP_BRIGHTNESS, &[200]);
    assert_eq!(rig.level(), 254);
    rig.command(OP_TEMPERATURE, &[200]);
    assert_eq!(rig.bulb.state().color_temp, 454, "past 127 is the warmest");
    let key = rig.key.unwrap();
    rig.write(COMMAND, &block_packet(&key, 2, &[0xe0, 0x60, 0x01, 0x21]));
    assert_eq!(rig.world.borrow().nv_writes, 0, "an address needs two octets");
    rig.write(COMMAND, &block_packet(&key, 2, &[0xe0, 0x60, 0x01, 0x21, 0x00]));
    assert_eq!(rig.world.borrow().nv_writes, 1);
}

#[test]
fn a_mesh_reset_ends_the_connection_300_ms_later() {
    let mut rig = logged_in(DIMMABLE);
    let packet = rig.command_packet(0xffff, OP_RESET, &[0]);
    rig.write(COMMAND, &packet);
    rig.run(200);
    assert_eq!(rig.state(), LinkState::Connected);
    rig.run(200);
    assert_eq!(rig.state(), LinkState::Advertising);
}

#[test]
fn the_zcl_tunnel_takes_the_bulbs_address_with_or_without_the_group_bit() {
    let mut rig = logged_in(DIMMABLE);
    let key = rig.key.unwrap();
    for (address, taken) in [(0x8033u16, true), (0x0033, true), (0x8034, false), (0x0133, false)] {
        rig.bulb.handle(bulb_core::Command::On);
        rig.run(300);
        let [hi, lo] = address.to_be_bytes();
        rig.write(COMMAND, &block_packet(&key, 0, &[hi, lo, 0x01, 0x06, 0x00, 0]));
        rig.run(300);
        assert_eq!(!rig.is_on(), taken, "{address:#06x}");
    }
}

fn long_block(key: &[u8; 16], payload: &[u8], extra: usize) -> Vec<u8> {
    let mut whole = vec![payload.len() as u8];
    whole.extend_from_slice(payload);
    let mut plain = [0u8; 16];
    plain[0] = crc8_6c(&whole);
    let head = whole.len().min(15);
    plain[1..1 + head].copy_from_slice(&whole[..head]);
    let mut v = vec![2u8];
    v.extend_from_slice(&tl_aes_encrypt(key, &plain));
    v.extend_from_slice(&whole[head..]);
    v.resize(v.len() + extra, 0);
    v
}

#[test]
fn a_stock_block_carries_at_most_18_octets_and_needs_all_of_them() {
    let mut rig = logged_in(DIMMABLE);
    let key = rig.key.unwrap();
    assert_eq!(rig.att(&[0x02, 64, 0]), [0x03, 64, 0]);
    let mut power = vec![OP_POWER, 0x60, 0x01, 0];
    power.resize(18, 0);
    let mut too_long = power.clone();
    too_long.push(0);
    rig.write(COMMAND, &long_block(&key, &too_long, 0));
    rig.run(300);
    assert!(rig.is_on(), "19 octets");
    let cut = long_block(&key, &power, 0);
    rig.write(COMMAND, &cut[..cut.len() - 1]);
    rig.run(300);
    assert!(rig.is_on(), "the last octet missing");
    rig.write(COMMAND, &long_block(&key, &power, 0));
    rig.run(300);
    assert!(!rig.is_on(), "18 octets");
}

#[test]
fn malformed_writes_and_searches_get_their_errors_and_an_empty_pdu_nothing() {
    let mut rig = connected(DIMMABLE);
    assert_eq!(rig.att(&[0x12, 0x40, 0x00, 0x01]), [0x01, 0x12, 0x40, 0x00, 0x01], "write to a handle that does not exist");
    assert_eq!(rig.att(&[0x12, 0x15]), [0x01, 0x12, 0x00, 0x00, 0x04], "write without a whole handle");
    assert_eq!(rig.att(&[0x06, 0x01, 0x00, 0xff, 0xff, 0x00]), [0x01, 0x06, 0x00, 0x00, 0x04], "Find By Type Value without its type");
    assert_eq!(rig.att(&[0x06, 0x05, 0x00, 0x01, 0x00, 0x00, 0x28]), [0x01, 0x06, 0x05, 0x00, 0x01], "a range that ends before it starts");
    rig.central().send_att(&[]);
    rig.run(300);
    assert!(rig.central().att.is_empty());
    assert_eq!(rig.read(0x0b), b"AwoX");
}

#[test]
fn the_apps_tunnel_without_a_transition_time_is_instant_and_a_stock_block_0x01_resets_the_mesh() {
    let mut rig = logged_in(DIMMABLE);
    let key = rig.key.unwrap();
    rig.write(COMMAND, &block_packet(&key, 0, &[0x00, 0x00, 0x01, 0x08, 0x00, 0x04, 64]));
    rig.run(20);
    assert_eq!(rig.level(), 64);
    for (op, value) in [(4u8, *b"EgloTest\0\0\0\0\0\0\0\0"), (5, *b"s3cret\0\0\0\0\0\0\0\0\0\0")] {
        rig.write(PAIR, &[&[op][..], &tl_aes_encrypt(&key, &value)].concat());
    }
    assert_eq!(rig.read(0x03), b"EgloTest");
    rig.write(COMMAND, &block_packet(&key, 2, &[0x01, 0, 0]));
    rig.run(600);
    assert_eq!(rig.state(), LinkState::Advertising, "the bulb ended the connection");
    rig.connect();
    assert_eq!(rig.read(0x03), b"unpaired");
}

#[test]
fn a_new_connection_starts_with_the_default_mtu_and_notifications_off() {
    let mut rig = connected(DIMMABLE);
    assert_eq!(rig.att(&[0x02, 64, 0]), [0x03, 64, 0]);
    rig.write(0x13, &[1, 0]);
    rig.central().send_fragments(&[&[0x03, 0x00, 0x04, 0x00, 0x0a][..]]);
    rig.run(300);
    rig.central().terminate();
    rig.run(300);
    rig.connect();
    assert_eq!(rig.read(0x13), [0, 0]);
    assert_eq!(rig.att(&[0x04, 0x01, 0x00, 0xff, 0xff]).len(), 2 + 5 * 4, "23 octets again");
}

#[test]
fn responses_fill_the_mtu_exactly_and_cut_a_long_value_to_it() {
    let mut rig = connected(DIMMABLE);
    let rsp = rig.att(&[0x08, 0x01, 0x00, 0xff, 0xff, 0x03, 0x28]);
    assert_eq!((rsp[1], rsp.len()), (7, 23), "three declarations of 7 octets in 23");
    assert_eq!(rig.att(&[0x02, 26, 0]), [0x03, 64, 0]);
    assert_eq!(rig.att(&[0x04, 0x01, 0x00, 0xff, 0xff]).len(), 26, "six 16-bit entries in 26 octets");
    let info = [0x6f, 0x5e, 0x4d, 0x3c, 0x2b, 0x1a, 0x2f, 0x8e, 0x3d, 0x4c, 0x4b, 0x5a, 0x01, 0x00, 0x10, 0xe6];
    rig.ble.borrow_mut().set_diag_info(&ble::diag::Info { tunable: false, watchdog: false, sense: false, release: 1, ieee: 0, model: "M".repeat(30).leak(), firmware: "F" });
    let rsp = rig.att(&[&[0x08, 0x01, 0x00, 0xff, 0xff][..], &info].concat());
    assert_eq!((rsp[0], rsp[1], rsp.len()), (0x09, 2 + 22, 26), "the value cut to what the MTU leaves");
    assert_eq!(rsp[2..5], [0x22, 0x00, 0x01]);
}

fn stored(flags: u8) -> Rig {
    let rig = Rig::new(DIMMABLE);
    let mut nv = vec![0xa7];
    nv.extend_from_slice(b"EgloTest\0\0\0\0\0\0\0\0");
    nv.extend_from_slice(b"s3cret\0\0\0\0\0\0\0\0\0\0");
    nv.extend_from_slice(&[0x11; 16]);
    nv.extend_from_slice(&[0, 0, flags]);
    rig.world.borrow_mut().nv = Some(nv);
    rig.restart()
}

#[test]
fn the_stored_flags_say_whether_the_network_is_paired_and_provisioned() {
    for (flags, paired, provisioned) in [(0x00, false, false), (0x01, true, false), (0x02, false, true), (0x03, true, true)] {
        let mut rig = stored(flags);
        rig.central().scan = true;
        rig.run(400);
        let rsp = rig.central().scan_responses.last().unwrap().clone();
        assert_eq!(rsp[8 + 22], if provisioned { 0x60 } else { 0x3f }, "flags {flags}");
        rig.connect();
        assert_eq!(!rig.login(b"EgloTest", b"1234"), paired, "flags {flags}: a paired network takes its own password only");
        assert!(rig.login(b"EgloTest", if paired { &b"s3cret"[..] } else { &b"1234"[..] }), "flags {flags}");
    }
}

#[test]
fn an_odd_level_rounds_up_to_the_mesh_brightness_and_a_white_colour_bulb_shows_no_hue() {
    let mut rig = logged_in(COLOUR);
    rig.bulb.handle(bulb_core::Command::MoveToLevel { level: 129, transition_ms: 0 });
    rig.run(400);
    assert_eq!(rig.statuses().last().unwrap().1[3], 65);
    rig.central().terminate();
    rig.run(300);
    rig.central().scan = true;
    rig.run(400);
    let rsp = rig.central().scan_responses.last().unwrap().clone();
    assert_eq!(rsp[8 + 18..8 + 20], [0xff, 0xff], "in colour temperature mode");
}
