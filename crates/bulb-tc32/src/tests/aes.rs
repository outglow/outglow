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
