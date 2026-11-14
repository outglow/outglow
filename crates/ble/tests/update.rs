//! Firmware update over the mesh service's update characteristics.

mod sim;

use sim::{make_image, ota_packet, Rig, DIMMABLE};

const OTA: u16 = 0x1b;
const OTA_CLASSIC: u16 = 0x18;

/// Sends an image the way flasher/awox_ota.py does: 16-byte blocks, the
/// last one padded with 0xff. Returns the number of blocks.
fn send_blocks(rig: &mut Rig, handle: u16, image: &[u8]) -> u16 {
    for (i, block) in image.chunks(16).enumerate() {
        let mut b = block.to_vec();
        b.resize(16, 0xff);
        rig.write(handle, &ota_packet(i as u16, &b));
    }
    image.len().div_ceil(16) as u16
}

fn erase(rig: &mut Rig, image: &[u8]) {
    let mut packet = [0u8; 20];
    packet[2] = (image.len() / 4096 + 2) as u8;
    rig.write(OTA, &packet);
}

fn status(rig: &mut Rig) -> (u8, u8, u16) {
    let s = rig.read(OTA);
    assert_eq!(s.len(), 20);
    assert_eq!(s[2..4], s[4..6]);
    (s[0], s[1], u16::from_le_bytes([s[2], s[3]]))
}

#[test]
fn an_image_is_staged_checked_and_started() {
    let mut rig = Rig::new(DIMMABLE);
    rig.connect();
    assert_eq!(status(&mut rig), (0, 0, 0));
    let image = make_image(20000, 3);
    erase(&mut rig, &image);
    assert_eq!(status(&mut rig).0, 1, "erasing, one sector at a time");
    rig.run(500);
    assert_eq!(status(&mut rig), (2, 0, 0));
    assert_eq!(rig.world.borrow().erased_sectors, [0, 0x1000, 0x2000, 0x3000, 0x4000, 0x5000]);
    assert!(rig.ble.borrow().ota_active());

    let blocks = send_blocks(&mut rig, OTA, &image);
    assert_eq!(status(&mut rig), (3, 0, blocks));
    assert_eq!(rig.read(OTA)[7], 100, "percent");
    // end packet: the block count and its CRC
    rig.write(OTA, &ota_packet(blocks, &[]));
    assert_eq!(status(&mut rig), (4, 0, blocks));
    {
        let w = rig.world.borrow();
        assert!(!w.activated, "the status can still be read before the restart");
        assert_eq!(w.staging[8], 0xff, "not bootable until then");
        assert_eq!(w.staging[..8], image[..8]);
        assert_eq!(w.staging[9..image.len()], image[9..]);
    }
    rig.run(1200);
    let w = rig.world.borrow();
    assert!(w.activated);
    assert_eq!(w.staging[..image.len()], image[..], "the staging area holds the image, bootable mark included");
}

#[test]
fn the_app_way_without_an_erase_packet_and_with_ffff_at_the_end() {
    let mut rig = Rig::new(DIMMABLE);
    rig.connect();
    let image = make_image(0x1100, 9);
    send_blocks(&mut rig, OTA_CLASSIC, &image);
    assert_eq!(rig.world.borrow().erased_sectors, [0, 0x1000], "sectors erased as the image reached them");
    rig.write(OTA_CLASSIC, &ota_packet(0xffff, &[]));
    assert_eq!(status(&mut rig).0, 4);
    rig.run(1200);
    let w = rig.world.borrow();
    assert!(w.activated);
    assert_eq!(w.staging[..image.len()], image[..]);
}

#[test]
fn an_image_that_fails_its_checksum_is_not_started() {
    for damage in ["body", "magic", "size", "short"] {
        let mut rig = Rig::new(DIMMABLE);
        rig.connect();
        let mut image = make_image(2000, 1);
        match damage {
            "body" => image[700] ^= 1,
            "size" => image[0x19] ^= 0x40,
            "short" => image.truncate(1600),
            _ => {}
        }
        let mut sent = image.clone();
        if damage == "magic" {
            sent[9] = b'X';
        }
        let blocks = send_blocks(&mut rig, OTA, &sent);
        // a wrong magic is seen in the first block; the rest only at the end
        if damage != "magic" {
            assert_eq!(status(&mut rig), (3, 0, blocks), "{damage}");
            rig.write(OTA, &ota_packet(blocks, &[]));
        }
        assert_eq!(status(&mut rig).0..=status(&mut rig).1, 4..=2, "{damage}: done, failed");
        rig.run(1500);
        let w = rig.world.borrow();
        assert!(!w.activated, "{damage}");
        assert_ne!(w.staging[8], b'K', "{damage}: never marked bootable");
    }
}

#[test]
fn blocks_must_come_in_order_and_whole() {
    let mut rig = Rig::new(DIMMABLE);
    rig.connect();
    let image = make_image(1024, 5);
    let block = |i: usize| ota_packet(i as u16, &image[16 * i..16 * i + 16]);
    rig.write(OTA, &block(0));
    rig.write(OTA, &block(1));
    // the last block again: a retransmission
    rig.write(OTA, &block(1));
    assert_eq!(status(&mut rig), (3, 0, 2));
    // a damaged packet is ignored
    let mut bad = block(2);
    bad[5] ^= 0x10;
    rig.write(OTA, &bad);
    assert_eq!(status(&mut rig), (3, 0, 2));
    rig.write(OTA, &block(2));
    // a gap ends the transfer
    rig.write(OTA, &block(4));
    assert_eq!(status(&mut rig).0..=status(&mut rig).1, 4..=2);
    // and a new one can start from block 0
    rig.write(OTA, &block(0));
    assert_eq!(status(&mut rig), (3, 0, 1));
}

#[test]
fn an_end_packet_must_match() {
    let mut rig = Rig::new(DIMMABLE);
    rig.connect();
    let image = make_image(640, 5);
    let blocks = send_blocks(&mut rig, OTA, &image);
    // the wrong block count
    rig.write(OTA, &ota_packet(blocks - 1, &[]));
    assert_eq!(status(&mut rig).1, 2);
    rig.run(1500);
    assert!(!rig.world.borrow().activated);

    // an end packet with nothing received
    let mut rig = Rig::new(DIMMABLE);
    rig.connect();
    rig.write(OTA, &ota_packet(0xffff, &[]));
    assert_eq!(status(&mut rig).1, 2);
}

#[test]
fn a_transfer_that_failed_while_erasing_is_not_started() {
    let mut rig = Rig::new(DIMMABLE);
    rig.connect();
    let image = make_image(20000, 3);
    erase(&mut rig, &image);
    // an end packet out of place, with the erase timer still running
    rig.central().send_att(&[&[0x52, OTA as u8, 0][..], &ota_packet(0xffff, &[])].concat());
    rig.run(100);
    assert_eq!(status(&mut rig).0..=status(&mut rig).1, 4..=2);
    rig.run(2000);
    assert!(!rig.world.borrow().activated);
}

#[test]
fn an_image_the_flash_did_not_keep_is_reported_as_failed() {
    let mut rig = Rig::new(DIMMABLE);
    rig.connect();
    rig.world.borrow_mut().flash_fault = true;
    let image = make_image(640, 5);
    let blocks = send_blocks(&mut rig, OTA, &image);
    rig.write(OTA, &ota_packet(blocks, &[]));
    assert_eq!(status(&mut rig).0..=status(&mut rig).1, 4..=0, "the transfer itself was good");
    rig.run(1200);
    assert!(!rig.world.borrow().activated);
    assert_eq!(status(&mut rig).0..=status(&mut rig).1, 4..=2, "the check before the restart was not");
}

#[test]
fn a_lost_connection_starts_the_transfer_over() {
    let mut rig = Rig::new(DIMMABLE);
    rig.connect();
    let image = make_image(2048, 7);
    for (i, block) in image.chunks(16).take(40).enumerate() {
        rig.write(OTA, &ota_packet(i as u16, block));
    }
    rig.central().terminate();
    rig.run(300);
    assert!(!rig.ble.borrow().ota_active());
    rig.connect();
    assert_eq!(status(&mut rig), (0, 0, 0));
    // block 40 is no longer expected
    rig.write(OTA, &ota_packet(40, &image[640..656]));
    assert_eq!(status(&mut rig), (0, 0, 0));
    let blocks = send_blocks(&mut rig, OTA, &image);
    rig.write(OTA, &ota_packet(blocks, &[]));
    rig.run(1200);
    assert!(rig.world.borrow().activated);
}

/// `ota_busy`: what the firmware waits for before it stops BLE (the setting
/// over Zigbee): the transfer, the restart into a good image, and the
/// connection the update ran in, whose central reads the final status.
#[test]
fn an_update_is_busy_until_its_restart_or_the_end_of_its_connection() {
    let mut rig = Rig::new(DIMMABLE);
    rig.connect();
    assert!(!rig.ble.borrow().ota_busy(), "a connection without an update");
    let image = make_image(640, 5);
    let blocks = send_blocks(&mut rig, OTA, &image);
    assert!(rig.ble.borrow().ota_busy(), "the transfer");
    rig.write(OTA, &ota_packet(blocks, &[]));
    assert_eq!(status(&mut rig).0..=status(&mut rig).1, 4..=0);
    assert!(!rig.ble.borrow().ota_active());
    assert!(rig.ble.borrow().ota_busy(), "a good image waiting for its restart");

    let mut rig = Rig::new(DIMMABLE);
    rig.connect();
    let mut image = make_image(640, 5);
    image[300] ^= 1;
    let blocks = send_blocks(&mut rig, OTA, &image);
    rig.write(OTA, &ota_packet(blocks, &[]));
    assert_eq!(status(&mut rig).0..=status(&mut rig).1, 4..=2, "failed");
    assert!(rig.ble.borrow().ota_busy(), "its central still reads the status");
    rig.central().terminate();
    rig.run(300);
    assert!(!rig.ble.borrow().ota_busy(), "not once that connection has ended");
    rig.connect();
    assert!(!rig.ble.borrow().ota_busy(), "nor in the next one, which has not touched the update");
    assert_eq!(status(&mut rig).0..=status(&mut rig).1, 4..=2, "which still reads how the last one went");
    assert!(!rig.ble.borrow().ota_busy(), "reading the status holds nothing: anyone may");
    rig.write(OTA, &ota_packet(0, &image[..16]));
    assert!(rig.ble.borrow().ota_busy(), "a new transfer does");
}

#[test]
fn larger_blocks_after_an_mtu_exchange() {
    let mut rig = Rig::new(DIMMABLE);
    rig.connect();
    assert_eq!(rig.att(&[0x02, 64, 0]), [0x03, 64, 0]);
    let image = make_image(3000, 2);
    // MTU - 7 bytes per block
    for (i, block) in image.chunks(57).enumerate() {
        let mut b = block.to_vec();
        b.resize(57, 0xff);
        rig.write(OTA, &ota_packet(i as u16, &b));
    }
    rig.write(OTA, &ota_packet(image.len().div_ceil(57) as u16, &[]));
    assert_eq!(status(&mut rig).0..=status(&mut rig).1, 4..=0);
    rig.run(1200);
    let w = rig.world.borrow();
    assert!(w.activated);
    assert_eq!(w.staging[..image.len()], image[..]);
}

#[test]
fn an_image_larger_than_the_staging_area_is_refused() {
    let mut rig = Rig::new(DIMMABLE);
    rig.connect();
    let mut image = make_image(256, 5);
    image[0x18..0x1c].copy_from_slice(&(sim::STAGING as u32 + 16).to_le_bytes());
    rig.write(OTA, &ota_packet(0, &image[..16]));
    rig.write(OTA, &ota_packet(1, &image[16..32]));
    assert_eq!(status(&mut rig).1, 2);
}
