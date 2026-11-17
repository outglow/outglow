use zigbee::aes::Aes128;
use zigbee::mac::{self, Addr, Beacon};
use zigbee::nwk;
use zigbee::util::{Buf, Buffer};

#[test]
fn a_mac_header_with_a_reserved_addressing_mode_is_not_a_frame() {
    assert_eq!(mac::Header::parse(&[0x01, 0x04, 0x07, 0x62, 0x1A, 0x34, 0x12]), None, "destination mode 1");
    assert_eq!(mac::Header::parse(&[0x01, 0x48, 0x07, 0x62, 0x1A, 0x34, 0x12, 0x00, 0x00]), None, "source mode 1");
    assert_eq!(mac::Header::parse(&[0x01, 0x88, 0x07, 0x62, 0x1A, 0x34]), None, "cut short");
}

#[test]
fn a_mac_source_without_pan_compression_has_its_own_pan() {
    let mpdu = [0x01, 0x88, 0x07, 0x62, 0x1A, 0x34, 0x12, 0x63, 0x1A, 0x78, 0x56, 0xEE];
    let (h, len) = mac::Header::parse(&mpdu).unwrap();
    assert_eq!(len, 11);
    assert_eq!((h.dst_pan, h.dst, h.src_pan, h.src), (0x1A62, Addr::Short(0x1234), 0x1A63, Addr::Short(0x5678)));
    let mut b = Buf::new();
    h.write(&mut b);
    assert_eq!(b.as_slice(), &mpdu[..11]);
    let ext = [0x03, 0xCC, 0x07, 0x62, 0x1A, 1, 2, 3, 4, 5, 6, 7, 8, 0xFF, 0xFF, 8, 7, 6, 5, 4, 3, 2, 1];
    let (h, len) = mac::Header::parse(&ext).unwrap();
    assert_eq!(len, ext.len());
    assert_eq!((h.frame_type, h.dst, h.src_pan, h.src), (mac::FRAME_CMD, Addr::Ext(0x0807060504030201), 0xFFFF, Addr::Ext(0x0102030405060708)));
}

fn beacon_header(src: Addr) -> mac::Header {
    mac::Header {
        frame_type: mac::FRAME_BEACON,
        security: false,
        pending: false,
        ack_request: false,
        seq: 1,
        dst_pan: 0,
        dst: Addr::None,
        src_pan: 0x1A62,
        src,
    }
}

fn zigbee_payload() -> Vec<u8> {
    let mut p = vec![0x00, 0x22, 0x84];
    p.extend_from_slice(&0xDDDDDDDDDDDDDDDDu64.to_le_bytes());
    p.extend_from_slice(&[0xFF, 0xFF, 0xFF]);
    p
}

#[test]
fn a_beacon_skips_gts_and_pending_addresses_and_needs_a_short_source_and_the_zigbee_protocol() {
    let mut payload = vec![0xFF, 0xCF, 0x02, 0x00, 0x11, 0x22, 0x33, 0x44, 0x55, 0x66, 0x7, 0x12];
    payload.extend_from_slice(&[0x21]);
    payload.extend_from_slice(&[0xAA, 0xBB, 0xCC, 0xDD]);
    payload.extend_from_slice(&[1, 2, 3, 4, 5, 6, 7, 8]);
    payload.extend_from_slice(&zigbee_payload());
    let b = Beacon::parse(&beacon_header(Addr::Short(0x0000)), &payload).unwrap();
    assert!(b.permit_join && b.pan_coordinator);
    assert_eq!((b.stack_profile, b.protocol_version, b.depth), (2, 2, 0));
    assert!(b.router_capacity && b.end_device_capacity);
    assert_eq!((b.ext_pan, b.update_id), (0xDDDDDDDDDDDDDDDD, 0), "without an update identifier");
    assert_eq!(Beacon::parse(&beacon_header(Addr::Ext(1)), &payload), None);
    let mut other = vec![0xFF, 0xCF, 0x00, 0x00];
    other.extend_from_slice(&zigbee_payload());
    other[4] = 0x01;
    assert_eq!(Beacon::parse(&beacon_header(Addr::Short(0)), &other), None, "another protocol");
}

#[test]
fn a_beacon_written_parses_back() {
    let b = Beacon {
        pan: 0x1A62,
        short: 0x1234,
        permit_join: true,
        pan_coordinator: false,
        stack_profile: 2,
        protocol_version: 2,
        router_capacity: true,
        end_device_capacity: false,
        depth: 3,
        ext_pan: 0x0011223344556677,
        update_id: 9,
    };
    let mut f = Buf::new();
    b.write(4, &mut f);
    let (h, len) = mac::Header::parse(f.as_slice()).unwrap();
    assert_eq!((h.frame_type, h.seq), (mac::FRAME_BEACON, 4));
    assert_eq!(Beacon::parse(&h, &f.as_slice()[len..]), Some(b));
}

#[test]
fn a_nwk_header_with_a_multicast_field_round_trips_and_too_many_relays_are_refused() {
    let mut h = nwk::Header::new(nwk::FRAME_DATA, 0x0001, 0x0000, 5, 3);
    h.multicast = Some(0x2A);
    let mut b = Buf::new();
    h.write(&mut b);
    assert_eq!(b.as_slice(), &[0x08, 0x03, 0x01, 0x00, 0x00, 0x00, 5, 3, 0x2A]);
    assert_eq!(nwk::Header::parse(b.as_slice()), Some((h, 9)));
    let mut relays = vec![0x08, 0x06, 0x01, 0x00, 0x00, 0x00, 5, 3, 13, 0];
    relays.extend_from_slice(&[0; 26]);
    assert_eq!(nwk::Header::parse(&relays), None);
    relays[8] = 12;
    assert!(nwk::Header::parse(&relays).is_some());
}

#[test]
fn nwk_security_needs_the_nwk_key_identifier_with_the_extended_nonce_and_a_whole_frame() {
    let key = Aes128::new(&[7; 16]);
    let mut f = Buf::new();
    nwk::Header::new(nwk::FRAME_DATA, 1, 2, 3, 4).write(&mut f);
    nwk::secure(&key, 0, 0x1122, 9, &mut f, 0, &[1, 2, 3]);
    let mut frame = f.as_slice().to_vec();
    assert!(nwk::parse_aux(&frame, 8).is_some());
    for sc in [0x20u8, 0x08, 0x30, 0x18] {
        let mut other = frame.clone();
        other[8] = sc;
        assert!(nwk::parse_aux(&other, 8).is_none(), "security control {sc:#04x}");
    }
    assert!(nwk::parse_aux(&frame, 40).is_none());
    let aux = nwk::parse_aux(&frame, 8).unwrap();
    let n = frame.len();
    assert_eq!(nwk::unsecure(&key, &aux, &mut frame[..8 + nwk::AUX_LEN + 3], 8), None);
    assert_eq!(nwk::unsecure(&key, &aux, &mut frame[..n], 8), Some(3));
}

#[test]
fn a_nwk_frame_too_long_to_secure_is_left_unsent() {
    let key = Aes128::new(&[7; 16]);
    let mut f = Buf::from_slice(&[0; 100]);
    nwk::secure(&key, 0, 0x1122, 9, &mut f, 0, &[0x55; 20]);
    assert!(!f.ok());
    assert!(!f.as_slice()[100..].contains(&0x55), "nothing past the room");
}

#[test]
fn a_buffer_overflows_rather_than_grow_and_clearing_it_starts_over() {
    let mut b: Buffer<4> = Buffer::default();
    assert!(b.is_empty() && b.ok());
    b.u16(0x0201);
    b.u32(0x06050403);
    assert!(!b.ok(), "six bytes do not fit in four");
    assert_eq!(b.as_slice(), &[0x01, 0x02]);
    assert_eq!(b.room(), 2);
    b.u8(3);
    assert_eq!(b.as_slice(), &[0x01, 0x02, 0x03]);
    b.set(7, 9);
    assert_eq!((b.get(2), b.get(7)), (3, 0));
    b.clear();
    assert!(b.is_empty() && b.ok());
    b.u64(1);
    assert!(!b.ok());
}

#[test]
fn pan_id_compression_is_only_for_frames_with_both_addresses() {
    let h = mac::Header {
        frame_type: mac::FRAME_CMD,
        security: false,
        pending: false,
        ack_request: false,
        seq: 9,
        dst_pan: 0xFFFF,
        dst: Addr::Short(0xFFFF),
        src_pan: 0xFFFF,
        src: Addr::None,
    };
    let mut b = Buf::new();
    h.write(&mut b);
    assert_eq!(b.as_slice(), &[0x03, 0x08, 9, 0xFF, 0xFF, 0xFF, 0xFF]);
    let h = mac::Header { dst: Addr::None, dst_pan: 0x1A62, src: Addr::Short(0x0001), src_pan: 0x1A62, ..h };
    let mut b = Buf::new();
    h.write(&mut b);
    assert_eq!(b.as_slice(), &[0x03, 0x80, 9, 0x62, 0x1A, 0x01, 0x00]);
}
