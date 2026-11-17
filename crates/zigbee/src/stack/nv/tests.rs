use super::digest;

#[test]
fn the_digest_is_32_bit_fnv_1a() {
    assert_eq!(digest(b""), 0x811C_9DC5);
    assert_eq!(digest(b"a"), 0xE40C_292C);
    assert_eq!(digest(b"foobar"), 0xBF9C_F968);
}
