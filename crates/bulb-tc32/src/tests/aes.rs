use super::*;
use crate::sim;

fn software(key: &[u8; 16], data: &[u8; 16]) -> [u8; 16] {
    let mut d = *data;
    zigbee::aes::Aes128::new(key).encrypt(&mut d);
    d
}

#[test]
fn the_engine_passes_its_self_test_and_encrypts_like_aes_128() {
    let _s = sim::session();
    STATE.set(0);
    let key = [0x2b, 0x7e, 0x15, 0x16, 0x28, 0xae, 0xd2, 0xa6, 0xab, 0xf7, 0x15, 0x88, 0x09, 0xcf, 0x4f, 0x3c];
    let plain = [0x32, 0x43, 0xf6, 0xa8, 0x88, 0x5a, 0x30, 0x8d, 0x31, 0x31, 0x98, 0xa2, 0xe0, 0x37, 0x07, 0x34];
    let mut data = plain;
    assert!(encrypt(&key, &mut data));
    assert_eq!(data, software(&key, &plain));
    assert_eq!(data[..4], [0x39, 0x25, 0x84, 0x1d]);
    assert_eq!(STATE.get(), 1);
}

#[test]
fn an_engine_that_never_finishes_is_given_up_for_good() {
    let _s = sim::session();
    STATE.set(0);
    sim::with(|c| c.aes_ready = false);
    let mut data = [0u8; 16];
    assert!(!encrypt(&[0; 16], &mut data));
    assert_eq!(STATE.get(), 2);
    sim::with(|c| c.aes_ready = true);
    assert!(!encrypt(&[0; 16], &mut data));
}

#[test]
fn a_block_that_times_out_after_the_self_test_disables_the_engine() {
    let _s = sim::session();
    STATE.set(1);
    sim::with(|c| c.aes_ready = false);
    let mut data = [0u8; 16];
    assert!(!encrypt(&[0; 16], &mut data));
    assert_eq!(STATE.get(), 2);
    STATE.set(0);
}

#[test]
fn an_engine_that_stops_asking_for_data_too_early_is_given_up() {
    let _s = sim::session();
    STATE.set(1);
    sim::with(|c| c.aes_takes = 2);
    let mut data = [7u8; 16];
    assert!(!encrypt(&[0; 16], &mut data));
    assert_eq!(data, [7; 16]);
    assert_eq!(STATE.get(), 2);
    let fed = sim::with(|c| c.writes()).iter().filter(|o| matches!(o, sim::Op::W32(0x548, _))).count();
    assert_eq!(fed, 2);
}

#[test]
fn an_engine_that_asks_for_more_is_given_one_block() {
    let _s = sim::session();
    STATE.set(1);
    sim::with(|c| c.aes_takes = 6);
    let key = [0x11; 16];
    let plain: [u8; 16] = core::array::from_fn(|i| i as u8);
    let mut data = plain;
    assert!(encrypt(&key, &mut data));
    assert_eq!(data, software(&key, &plain));
    let fed: Vec<sim::Op> = sim::with(|c| c.writes()).into_iter().filter(|o| matches!(o, sim::Op::W32(0x548, _))).collect();
    assert_eq!(fed, vec![sim::Op::W32(0x548, 0x0302_0100), sim::Op::W32(0x548, 0x0706_0504), sim::Op::W32(0x548, 0x0b0a_0908), sim::Op::W32(0x548, 0x0f0e_0d0c)]);
}

#[test]
fn the_engine_gets_the_key_octet_by_octet_after_choosing_encryption() {
    let _s = sim::session();
    STATE.set(1);
    sim::with(|c| c.poke8(0x540, 0xff));
    let key: [u8; 16] = core::array::from_fn(|i| 0xa0 + i as u8);
    let mut data = [0u8; 16];
    assert!(encrypt(&key, &mut data));
    let w = sim::with(|c| c.writes());
    assert_eq!(w[0], sim::Op::W8(0x540, 0xfa));
    assert_eq!(w[1..17].to_vec(), (0..16).map(|i| sim::Op::W8(0x550 + i, 0xa0 + i as u8)).collect::<Vec<_>>());
}

#[test]
fn the_engine_is_given_200_us_to_finish() {
    for (late, ok) in [(200 * 16u32, true), (200 * 16 + 16, false)] {
        let _s = sim::session();
        STATE.set(1);
        let mut start = None;
        sim::with(|c| {
            c.aes_ready = false;
            c.on(move |c, op| {
                if let sim::Op::R32(0x740, t) = op {
                    let s = *start.get_or_insert(t);
                    if t.wrapping_sub(s) == late {
                        c.aes_ready = true;
                    }
                }
            })
        });
        let mut data = [0u8; 16];
        assert_eq!(encrypt(&[0; 16], &mut data), ok, "{late}");
    }
}

#[test]
fn an_engine_that_fails_the_fips_197_vector_is_never_used() {
    for flip in [0x01u8, 0x80] {
        let _s = sim::session();
        STATE.set(0);
        sim::with(|c| c.aes_corrupt = flip);
        let mut data = [0u8; 16];
        assert!(!encrypt(&[0; 16], &mut data));
        assert_eq!(STATE.get(), 2);
        assert_eq!(sim::with(|c| c.writes()).iter().filter(|o| matches!(o, sim::Op::W32(0x548, _))).count(), 4);
    }
    let _s = sim::session();
    STATE.set(0);
    assert!(self_test());
    assert_eq!(STATE.get(), 0);
}
