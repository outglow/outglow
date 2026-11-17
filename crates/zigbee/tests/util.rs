use zigbee::util::{le, pack};

#[test]
fn fields_are_packed_by_or_at_their_bit_positions() {
    let table: [(&[(u16, u32)], u16); 6] = [
        (&[], 0x0000),
        (&[(0x0001, 0)], 0x0001),
        (&[(0x0003, 2)], 0x000C),
        (&[(0x0001, 15)], 0x8000),
        (&[(0x0003, 0), (0x0001, 0)], 0x0003),
        (&[(0x0FFF, 0), (0x0001, 8), (0x0003, 14)], 0xCFFF),
    ];
    for (fields, packed) in table {
        assert_eq!(pack(fields), packed, "{fields:?}");
    }
    assert_eq!(pack(&[(0x0Fu8, 0), (0x03, 2)]), 0x0F);
    assert_eq!(pack(&[(0x1234_5678u64, 32), (0x9ABC_DEF0, 0), (1, 0)]), 0x1234_5678_9ABC_DEF1);
}

#[test]
fn up_to_four_little_endian_bytes_are_a_number() {
    let table: [(&[u8], u32); 6] = [
        (&[], 0),
        (&[0x7F], 0x7F),
        (&[0x34, 0x12], 0x1234),
        (&[0x56, 0x34, 0x12], 0x0012_3456),
        (&[0x78, 0x56, 0x34, 0x12], 0x1234_5678),
        (&[0x78, 0x56, 0x34, 0x12, 0xFF], 0x1234_5678),
    ];
    for (bytes, value) in table {
        assert_eq!(le(bytes), value, "{bytes:02X?}");
    }
}
