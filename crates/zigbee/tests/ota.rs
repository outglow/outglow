//! The OTA Upgrade client against a simulated server.

mod sim;

use sim::*;
use zigbee::nwk;
use zigbee::stack::OtaState;

const MFG: u16 = 0x1141;
const TYPE: u16 = 0x0390;
const NEW: u32 = 0x0002_0000;

fn offer(net: &mut Net, image: &[u8], version: u32) {
    net.coord.ota_file = Some(OtaFile { manufacturer: MFG, image_type: TYPE, version, data: make_ota_file(image, MFG, TYPE, version, 0, false) });
}

/// The staged image as the bootloader would see it.
fn staged(net: &Net, n: usize, len: usize) -> Vec<u8> {
    net.air.borrow().ports[net.nodes[n].id].staging[..len].to_vec()
}

fn activated(net: &Net, n: usize) -> bool {
    net.air.borrow().ports[net.nodes[n].id].activated
}

#[test]
fn downloads_checks_and_activates_a_newer_image() {
    let mut net = Net::new();
    let n = net.join_node();
    let short = net.nodes[n].short();
    let image = make_image(3000, 7);
    offer(&mut net, &image, NEW);
    // the server holds its answer to the Upgrade End Request back
    net.coord.ota_upgrade_time = None;
    net.coord.ota_notify(short);
    net.run_until(60_000, "upgrade end", |net| net.coord.ota_end.is_some());
    assert_eq!(net.coord.ota_end, Some(0x00));
    // until the server says "now", the image is in place but not bootable
    let mut held = image.clone();
    held[8] = 0xFF;
    net.run(1_000);
    assert!(!activated(&net, n));
    assert_eq!(staged(&net, n, image.len()), held);
    net.coord.ota_upgrade_end_response(short, MFG, TYPE, NEW, 0);
    net.run(2_000);
    assert!(activated(&net, n));
    assert_eq!(staged(&net, n, image.len()), image, "byte for byte, boot marker included");
    assert!(net.air.borrow().ports[net.nodes[n].id].staging[image.len()..].iter().all(|b| *b == 0xFF));
    // 3000 bytes of image plus the file's headers, 48 bytes a block
    assert_eq!(net.coord.ota_blocks as usize, (image.len() + 62).div_ceil(48));
    // the light kept working meanwhile
    let seq = net.coord.zcl(short, 0x0006, 0x01, 0x00, &[]);
    net.run(100);
    assert_eq!(net.coord.last_zcl(short, 0x0006, 0x0B).unwrap(), vec![0x18, seq, 0x0B, 0x00, 0x00]);
}

#[test]
fn asks_by_itself_after_joining_and_takes_nothing_it_already_has() {
    let mut net = Net::new();
    let n = net.join_node();
    let image = make_image(500, 1);
    // the same version as the running firmware
    offer(&mut net, &image, IDENTITY.file_version);
    assert_eq!(net.coord.ota_queries, 0);
    net.run(40_000);
    assert_eq!(net.coord.ota_queries, 1, "one query, half a minute after joining");
    assert_eq!(net.coord.ota_blocks, 0);
    assert_eq!(net.nodes[n].stack.ota_state(), OtaState::Idle);
    // an image of another type is not for this bulb
    net.coord.ota_file = Some(OtaFile { manufacturer: MFG, image_type: 0x0381, version: NEW, data: make_ota_file(&image, MFG, 0x0381, NEW, 0, false) });
    net.coord.ota_notify(net.nodes[n].short());
    net.run(5_000);
    assert_eq!(net.coord.ota_blocks, 0);
    // not even when a confused server offers it anyway
    net.coord.ota_offer_anyway = true;
    net.coord.ota_notify(net.nodes[n].short());
    net.run(5_000);
    assert_eq!(net.coord.ota_queries, 3);
    assert_eq!(net.coord.ota_blocks, 0);
}

#[test]
fn a_damaged_image_is_reported_and_never_activated() {
    let mut net = Net::new();
    let n = net.join_node();
    let short = net.nodes[n].short();
    for damage in 0..3 {
        let mut image = make_image(1000, 3);
        match damage {
            // content changed after the CRC was made
            0 => image[500] ^= 0x01,
            // wrong length in the header, CRC consistent with it
            1 => {
                image[0x18..0x1C].copy_from_slice(&999u32.to_le_bytes());
                seal_image(&mut image);
            }
            // no boot marker, CRC consistent
            _ => {
                image[9] = b'X';
                seal_image(&mut image);
            }
        }
        offer(&mut net, &image, NEW + damage);
        net.coord.ota_end = None;
        net.coord.ota_notify(short);
        net.run_until(60_000, "upgrade end", |net| net.coord.ota_end.is_some());
        assert_eq!(net.coord.ota_end, Some(0x96), "damage {damage}: INVALID_IMAGE");
        net.run(5_000);
        assert!(!activated(&net, n));
        assert_eq!(net.air.borrow().ports[net.nodes[n].id].staging[8], 0xFF, "boot marker never written");
    }
}

#[test]
fn lost_blocks_are_asked_for_again() {
    let mut net = Net::new();
    let n = net.join_node();
    let short = net.nodes[n].short();
    let image = make_image(800, 9);
    offer(&mut net, &image, NEW);
    net.coord.ota_drop_blocks = 3;
    net.coord.ota_notify(short);
    net.run_until(120_000, "upgrade end", |net| net.coord.ota_end.is_some());
    net.run(2_000);
    assert!(activated(&net, n));
    assert_eq!(staged(&net, n, image.len()), image);
}

#[test]
fn a_server_that_stops_answering_is_given_up_on() {
    let mut net = Net::new();
    let n = net.join_node();
    let short = net.nodes[n].short();
    let image = make_image(800, 9);
    offer(&mut net, &image, NEW);
    net.coord.ota_notify(short);
    net.run_until(10_000, "download started", |net| net.coord.ota_blocks > 2);
    net.coord.ota_drop_blocks = 1000;
    net.run(60_000);
    assert_eq!(net.nodes[n].stack.ota_state(), OtaState::Idle);
    assert!(!activated(&net, n));
    // and the next notification starts over from the beginning
    net.coord.ota_drop_blocks = 0;
    net.coord.ota_notify(short);
    net.run_until(120_000, "upgrade end", |net| net.coord.ota_end.is_some());
    net.run(2_000);
    assert_eq!(staged(&net, n, image.len()), image);
}

#[test]
fn file_headers_and_other_elements_are_skipped() {
    let mut net = Net::new();
    let n = net.join_node();
    let short = net.nodes[n].short();
    let image = make_image(700, 5);
    // a longer header (optional fields) and an element that is not an image before the image
    net.coord.ota_file = Some(OtaFile { manufacturer: MFG, image_type: TYPE, version: NEW, data: make_ota_file(&image, MFG, TYPE, NEW, 13, true) });
    net.coord.ota_notify(short);
    net.run_until(60_000, "upgrade end", |net| net.coord.ota_end.is_some());
    assert_eq!(net.coord.ota_end, Some(0x00));
    net.run(2_000);
    assert_eq!(staged(&net, n, image.len()), image);
}

#[test]
fn a_file_that_does_not_fit_is_not_started() {
    let mut net = Net::new();
    let n = net.join_node();
    let short = net.nodes[n].short();
    let image = make_image(STAGING_MAX + 100, 5);
    offer(&mut net, &image, NEW);
    net.coord.ota_notify(short);
    net.run(10_000);
    assert_eq!(net.coord.ota_queries, 1);
    assert_eq!(net.coord.ota_blocks, 0);
    assert_eq!(net.nodes[n].stack.ota_state(), OtaState::Idle);
}

#[test]
fn a_power_cut_during_the_download_costs_only_the_part_since_the_last_checkpoint() {
    let mut net = Net::new();
    let n = net.join_node();
    let short = net.nodes[n].short();
    // more than two flash sectors
    let image = make_image(9500, 11);
    offer(&mut net, &image, NEW);
    let whole = (image.len() + 62).div_ceil(48) as u32;
    net.coord.ota_notify(short);
    // cut the power when most of it is there (past the second checkpoint at 8192 bytes)
    net.run_until(60_000, "download well under way", |net| net.coord.ota_blocks >= whole - 10);
    net.power_cycle(n);
    assert!(!activated(&net, n));
    net.run(2_000);
    let before = net.coord.ota_blocks;
    net.coord.ota_notify(short);
    net.run_until(120_000, "upgrade end", |net| net.coord.ota_end.is_some());
    net.run(2_000);
    assert_eq!(net.coord.ota_end, Some(0x00));
    assert!(activated(&net, n));
    assert_eq!(staged(&net, n, image.len()), image, "the image is whole and intact");
    let again = net.coord.ota_blocks - before;
    assert!(again < 40, "{again} blocks after the power cut, {whole} for the whole file");
    // a different file is not mistaken for the interrupted one
    let other = make_image(9500, 12);
    offer(&mut net, &other, NEW + 1);
    net.coord.ota_end = None;
    net.coord.ota_notify(short);
    net.run_until(10_000, "second download under way", |net| net.coord.ota_blocks > before + again + 100);
    net.power_cycle(n);
    net.run(2_000);
    let third = make_image(9500, 13);
    offer(&mut net, &third, NEW + 2);
    let before = net.coord.ota_blocks;
    net.coord.ota_notify(short);
    net.run_until(120_000, "upgrade end", |net| net.coord.ota_end.is_some());
    net.run(2_000);
    assert_eq!(staged(&net, n, third.len()), third);
    assert!(net.coord.ota_blocks - before >= whole, "downloaded from the beginning");
}

// ------------------------------------------------------------ finding the server

#[test]
fn the_server_is_found_before_it_is_asked_on_the_endpoint_it_names() {
    // the server is on endpoint 5 of the coordinator (ZCL 11.8: found by a Match Descriptor request)
    let mut net = Net::new();
    net.coord.ota_endpoint = 5;
    let n = net.join_node();
    assert_eq!((net.coord.match_requests, net.coord.ota_queries), (0, 0));
    net.run(40_000);
    assert_eq!((net.coord.match_requests, net.coord.ota_queries), (1, 1), "one request to find it, one query");
    assert_eq!(net.coord.ota_asked_on, Some(5));
    assert_eq!(net.nodes[n].stack.ota_state(), OtaState::Idle);
    // a server that is known is not looked for again: the next query (after an Image Notify
    // to everyone, which says nothing about where the server is) goes straight to it
    net.coord.ota_notify(0xFFFD);
    net.run(5_000);
    assert_eq!((net.coord.match_requests, net.coord.ota_queries), (1, 2));
    // and it downloads from there
    let image = make_image(600, 4);
    offer(&mut net, &image, NEW);
    net.coord.ota_notify(0xFFFD);
    net.run_until(60_000, "upgrade end", |net| net.coord.ota_end.is_some());
    net.run(2_000);
    assert!(activated(&net, n));
    assert_eq!(net.coord.ota_asked_on, Some(5));
}

#[test]
fn nothing_is_asked_of_a_server_that_was_not_found() {
    let mut net = Net::new();
    net.coord.ota_hidden = true;
    let n = net.join_node();
    let image = make_image(600, 4);
    offer(&mut net, &image, NEW);
    // the coordinator is asked, then everyone; nobody says "here"
    net.run(60_000);
    assert_eq!(net.coord.match_requests, 2, "to the coordinator, then to all");
    assert_eq!(net.coord.ota_queries, 0);
    assert_eq!(net.nodes[n].stack.ota_state(), OtaState::Idle);
    // an Image Notify sent to this bulb is answered to its sender (ZCL 11.13.3.4), which is
    // a server by that: no search
    let short = net.nodes[n].short();
    net.coord.ota_endpoint = 9;
    net.coord.ota_notify(short);
    net.run_until(60_000, "upgrade end", |net| net.coord.ota_end.is_some());
    assert_eq!((net.coord.match_requests, net.coord.ota_queries), (2, 1));
    assert_eq!(net.coord.ota_asked_on, Some(9));
}

#[test]
fn a_server_on_another_node_is_found_by_asking_everyone() {
    let mut net = Net::new();
    net.coord.ota_hidden = true;
    let n = net.join_node();
    let bulb = net.nodes[n].short();
    net.coord.permit(120);
    net.run(600);
    // a device that joined through the bulb has the server, on its endpoint 7
    let mut server = Zed::new(&net.air, false);
    assert!(server.join(&mut net, bulb));
    let asked = |server: &Zed, cluster: u16, profile: u16| {
        server.rx.iter().find(|(_, p)| p.len() > 8 && p[2..4] == cluster.to_le_bytes() && p[4..6] == profile.to_le_bytes()).cloned()
    };
    let mut request = None;
    for _ in 0..600 {
        net.run(100);
        server.drain();
        request = asked(&server, 0x0006, 0x0000);
        if request.is_some() {
            break;
        }
    }
    let (h, aps) = request.expect("a Match Descriptor request that reaches every node");
    assert_eq!(h.dst, 0xFFFD);
    // for the address "all with their receivers on", profile 0x0104, one input cluster: 0x0019
    assert_eq!(&aps[9..], &[0xFD, 0xFF, 0x04, 0x01, 1, 0x19, 0x00, 0]);
    assert_eq!(asked(&server, 0x0019, 0x0104), None, "nothing asked before the server is found");
    let s = server.short.to_le_bytes();
    let rsp = [0x00, 0x00, 0x06, 0x80, 0x00, 0x00, 0x00, 0x41, aps[8], 0x00, s[0], s[1], 1, 7];
    let h = server.header(nwk::FRAME_DATA, bulb);
    server.nwk_send(h, &rsp);
    net.run(500);
    server.drain();
    let (h, query) = asked(&server, 0x0019, 0x0104).expect("a Query Next Image Request to the node that answered");
    assert_eq!((h.dst, query[1]), (server.short, 7), "its address, its endpoint");
    assert_eq!((query[8] & 0x0B, query[10]), (0x01, 0x01), "cluster command to the server: Query Next Image Request");
    assert_eq!(net.coord.ota_queries, 0);
}

// ------------------------------------------------------------ a server that takes its time

#[test]
fn a_server_that_says_wait_is_waited_for_and_not_given_up_on() {
    let mut net = Net::new();
    let n = net.join_node();
    let short = net.nodes[n].short();
    let image = make_image(600, 2);
    offer(&mut net, &image, NEW);
    // WAIT_FOR_DATA ten times in a row, 5 s each (ZCL 11.11.4: as often as the server likes)
    net.coord.ota_wait = (10, 5);
    net.coord.ota_notify(short);
    net.run_until(120_000, "upgrade end", |net| net.coord.ota_end.is_some());
    assert_eq!(net.coord.ota_end, Some(0x00));
    assert!(net.coord.ota_blocks as usize >= 10 + (image.len() + 62).div_ceil(48));
    net.run(2_000);
    assert!(activated(&net, n));
    // a long wait is waited out in full (ZCL 11.13.8.4: at a minimum the request time)
    let mut net = Net::new();
    let n = net.join_node();
    let short = net.nodes[n].short();
    offer(&mut net, &image, NEW);
    net.coord.ota_wait = (1, 700);
    net.coord.ota_notify(short);
    net.run_until(10_000, "first block request", |net| net.coord.ota_blocks == 1);
    net.run(699_000);
    assert_eq!(net.coord.ota_blocks, 1, "no request before the time the server named");
    assert_eq!(net.nodes[n].stack.ota_state(), OtaState::Downloading);
    net.run_until(60_000, "upgrade end", |net| net.coord.ota_end.is_some());
    assert_eq!(net.coord.ota_end, Some(0x00));
}

/// A bulb with the image downloaded and verified; the server answered the
/// Upgrade End Request with `upgrade_time` (or not at all).
fn downloaded(upgrade_time: Option<u32>) -> (Net, usize, u16, Vec<u8>) {
    let mut net = Net::new();
    let n = net.join_node();
    let short = net.nodes[n].short();
    let image = make_image(700, 6);
    offer(&mut net, &image, NEW);
    net.coord.ota_upgrade_time = upgrade_time;
    net.coord.ota_notify(short);
    net.run_until(60_000, "upgrade end", |net| net.coord.ota_end.is_some());
    assert_eq!(net.coord.ota_end, Some(0x00));
    net.run(1_000);
    assert!(!activated(&net, n));
    (net, n, short, image)
}

#[test]
fn an_image_told_to_wait_for_a_command_is_kept_until_the_command_comes() {
    // upgrade time 0xFFFFFFFF: "wait for my command" (ZCL Table 11-15, 11.16)
    let (mut net, n, short, image) = downloaded(Some(0xFFFF_FFFF));
    assert_eq!(net.nodes[n].stack.ota_state(), OtaState::Held);
    let blocks = net.coord.ota_blocks;
    net.run(120_000);
    assert_eq!(net.nodes[n].stack.ota_state(), OtaState::Held);
    // commands for other images are not this bulb's: another manufacturer, type, version
    net.coord.ota_upgrade_end_response(short, MFG + 1, TYPE, NEW, 0);
    net.coord.ota_upgrade_end_response(short, MFG, 0x0381, NEW, 0);
    net.coord.ota_upgrade_end_response(0xFFFD, MFG, TYPE, NEW + 1, 0);
    net.run(3_000);
    assert!(!activated(&net, n));
    assert_eq!(net.nodes[n].stack.ota_state(), OtaState::Held);
    // the command, sent to all clients at once and for every image (ZCL 11.13.9.7): switch in 2 s
    net.coord.ota_upgrade_end_response(0xFFFD, 0xFFFF, 0xFFFF, 0xFFFF_FFFF, 2);
    net.run(1_500);
    assert!(!activated(&net, n));
    net.run(1_500);
    assert!(activated(&net, n));
    assert_eq!(staged(&net, n, image.len()), image);
    assert_eq!(net.coord.ota_blocks, blocks, "nothing was downloaded again");
}

#[test]
fn a_server_that_says_wait_is_asked_again_every_hour() {
    let (mut net, n, _, image) = downloaded(Some(0xFFFF_FFFF));
    assert_eq!(net.coord.ota_ends, 1);
    net.run(3_590_000);
    assert_eq!(net.coord.ota_ends, 1, "not more often than once an hour (ZCL 11.16)");
    // this time the server names a time: in 3700 s, more than an hour away
    net.coord.ota_upgrade_time = Some(3_700);
    net.run_until(20_000, "asked again after an hour", |net| net.coord.ota_ends == 2);
    net.run(100);
    assert_eq!(net.nodes[n].stack.ota_state(), OtaState::Waiting);
    net.run(3_695_000);
    assert!(!activated(&net, n), "not before the time the server named");
    net.run(10_000);
    assert!(activated(&net, n));
    assert_eq!(staged(&net, n, image.len()), image);
}

#[test]
fn a_lost_upgrade_end_response_is_asked_for_again_and_a_silent_server_does_not_cost_the_image() {
    // the response to the first request is lost; the request is repeated and answered
    let (mut net, n, _, image) = downloaded(None);
    assert_eq!(net.nodes[n].stack.ota_state(), OtaState::Finishing);
    let blocks = net.coord.ota_blocks;
    net.coord.ota_upgrade_time = Some(0);
    net.run(25_000);
    assert_eq!(net.coord.ota_ends, 2);
    assert!(activated(&net, n));
    assert_eq!(staged(&net, n, image.len()), image);
    assert_eq!(net.coord.ota_blocks, blocks);
    // the server never answers: asked three more times, then the image is used without
    // its word (ZCL 11.16; UpgradeTimeoutPolicy "apply after timeout")
    let (mut net, n, _, image) = downloaded(None);
    net.run(70_000);
    assert_eq!(net.coord.ota_ends, 4);
    assert!(!activated(&net, n));
    net.run(20_000);
    assert_eq!(net.coord.ota_ends, 4);
    assert!(activated(&net, n));
    assert_eq!(staged(&net, n, image.len()), image);
}

#[test]
fn a_power_cut_while_the_image_waits_costs_only_the_end_of_the_download() {
    let mut net = Net::new();
    let n = net.join_node();
    let short = net.nodes[n].short();
    let image = make_image(9500, 21);
    offer(&mut net, &image, NEW);
    net.coord.ota_upgrade_time = Some(0xFFFF_FFFF);
    net.coord.ota_notify(short);
    net.run_until(120_000, "upgrade end", |net| net.coord.ota_end.is_some());
    net.run(1_000);
    assert_eq!(net.nodes[n].stack.ota_state(), OtaState::Held);
    net.power_cycle(n);
    net.run(2_000);
    let before = net.coord.ota_blocks;
    net.coord.ota_upgrade_time = Some(0);
    net.coord.ota_notify(short);
    net.run_until(120_000, "upgrade end again", |net| net.coord.ota_ends == 2);
    net.run(2_000);
    assert!(activated(&net, n));
    assert_eq!(staged(&net, n, image.len()), image);
    let again = net.coord.ota_blocks - before;
    assert!(again < 40, "{again} blocks after the power cut");
    // once the image is in use the stored progress is gone: the same file offered again starts at 0
    net.air.borrow_mut().ports[net.nodes[n].id].activated = false;
    let before = net.coord.ota_blocks;
    net.coord.ota_end = None;
    net.coord.ota_notify(short);
    net.run_until(120_000, "upgrade end", |net| net.coord.ota_end.is_some());
    assert!(net.coord.ota_blocks - before >= (image.len() as u32 + 62).div_ceil(48));
}
