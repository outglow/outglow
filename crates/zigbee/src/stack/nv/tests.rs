use super::super::address_tests::{Scripted, IDENTITY};
use super::digest;
use crate::stack::Stack;
extern crate std;
use std::vec::Vec;

#[test]
fn the_digest_is_32_bit_fnv_1a() {
    assert_eq!(digest(b""), 0x811C_9DC5);
    assert_eq!(digest(b"a"), 0xE40C_292C);
    assert_eq!(digest(b"foobar"), 0xBF9C_F968);
}

#[test]
fn only_joined_children_are_stored() {
    let stored = |unjoined: bool| {
        let mut s = Stack::new(Scripted { random: Vec::new(), now: 1_000, sent: Vec::new(), deaf: Vec::new(), busy: false, attempts: 0, bx: None }, IDENTITY);
        let c = &mut s.children[0];
        (c.used, c.joined, c.ext, c.short) = (true, true, 0xA, 0x2000);
        if unjoined {
            let c = &mut s.children[1];
            (c.used, c.joined, c.ext, c.short) = (true, false, 0xB, 0x2001);
        }
        s.store_children();
        s.children_stored[0]
    };
    assert_eq!(stored(true), stored(false));
}
