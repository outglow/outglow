use super::*;

#[test]
fn a_shared_array_reads_and_writes_inside_its_bounds_only() {
    let a: Shared<[u16; 3]> = Shared::new([1, 2, 3]);
    assert_eq!((a.at(0), a.at(2), a.at(3)), (Some(1), Some(3), None));
    a.set_at(2, 7);
    a.set_at(3, 9);
    assert_eq!(a.get(), [1, 2, 7]);
    a.update(|[x, y, z]| [z, y, x]);
    assert_eq!(a.get(), [7, 2, 1]);
}

#[test]
fn a_shared_buffer_reads_zero_and_ignores_writes_past_its_end() {
    let b: SharedBuf<8> = SharedBuf::new([0; 8]);
    b.write(7, 0x77);
    b.write(8, 0x88);
    assert_eq!((b.read(7), b.read(8)), (0x77, 0));
    b.copy_from(5, &[1, 2, 3, 4]);
    let mut out = [0xee; 5];
    b.copy_to(4, &mut out);
    assert_eq!(out, [0, 1, 2, 3, 0]);
    let mut none = [0u8; 0];
    b.copy_to(0, &mut none);
}

#[test]
fn a_shared_buffer_reads_and_writes_aligned_words_that_fit() {
    let b: SharedBuf<8> = SharedBuf::new([0; 8]);
    b.write32(4, 0x0403_0201);
    assert_eq!((0..8).map(|i| b.read(i)).collect::<Vec<_>>(), vec![0, 0, 0, 0, 1, 2, 3, 4]);
    assert_eq!(b.read32(4), 0x0403_0201);
    b.write32(0, 0x0807_0605);
    assert_eq!(b.read32(0), 0x0807_0605);
    for i in [1, 2, 3, 5, 8, 12] {
        b.write32(i, 0xffff_ffff);
        assert_eq!(b.read32(i), 0, "{i}");
    }
    assert_eq!((b.read32(0), b.read32(4)), (0x0807_0605, 0x0403_0201));
    let c: SharedBuf<6> = SharedBuf::new([1, 2, 3, 4, 5, 6]);
    assert_eq!(c.read32(4), 0);
    c.write32(4, 0);
    assert_eq!((c.read(4), c.read(5)), (5, 6));
    assert_ne!(b.addr(), c.addr());
}

#[test]
fn a_take_once_gives_its_value_only_the_first_time() {
    let once: &'static TakeOnce<u32> = Box::leak(Box::new(TakeOnce::new()));
    static ONCE: TakeOnce<u32> = TakeOnce::new();
    assert_eq!(once.take_with(|| 1).copied(), Some(1));
    let v = ONCE.take_with(|| 5).expect("the first take");
    *v += 1;
    assert!(ONCE.take_with(|| 9).is_none());
    ONCE.release();
    assert_eq!(ONCE.take_with(|| 11).copied(), Some(11));
}
