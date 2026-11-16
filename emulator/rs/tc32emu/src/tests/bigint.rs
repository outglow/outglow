use crate::bigint::BigUint;

fn to_u128(b: &BigUint) -> u128 {
    let w = b.to_words(4);
    w.iter().rev().fold(0u128, |v, &x| (v << 32) | x as u128)
}

fn from_u128(v: u128) -> BigUint {
    BigUint::from_words(&[v as u32, (v >> 32) as u32, (v >> 64) as u32, (v >> 96) as u32])
}

#[test]
fn bigint_against_u128() {
    let mut s = 0x1234_5678_9abc_def0u64;
    let mut next = || {
        s ^= s << 13;
        s ^= s >> 7;
        s ^= s << 17;
        s
    };
    for i in 0..20000 {
        let a = (next() as u128) << (next() % 64) | next() as u128;
        let mut b = (next() as u128) >> (next() % 63);
        if i % 3 == 0 {
            b = (next() as u128) << 40 | next() as u128;
        }
        if b == 0 {
            b = 1;
        }
        let (ba, bb) = (from_u128(a), from_u128(b));
        let (q, r) = ba.divrem(&bb);
        assert_eq!((to_u128(&q), to_u128(&r)), (a / b, a % b), "{:x} / {:x}", a, b);
        let x = (next() >> 1) as u128;
        let y = (next() >> 1) as u128;
        assert_eq!(to_u128(&from_u128(x).mul(&from_u128(y))), x * y);
        assert_eq!(to_u128(&from_u128(x).add(&from_u128(y))), x + y);
        let m = (next() | 1) as u128;
        assert_eq!(to_u128(&BigUint::mod_sub(&from_u128(x), &from_u128(y), &from_u128(m))), {
            let (xm, ym) = (x % m, y % m);
            if xm >= ym {
                xm - ym
            } else {
                xm + m - ym
            }
        });
    }
    let a = BigUint::from_hex("ffffffff00000001000000000000000000000000fffffffffffffffffffffffe").unwrap();
    let b = BigUint::from_hex("6b17d1f2e12c4247f8bce6e563a440f277037d812deb33a0f4a13945d898c296").unwrap();
    let r = BigUint::from_hex("1234567").unwrap();
    let (q, rem) = a.mul(&b).add(&r).divrem(&b);
    assert_eq!(q, a);
    assert_eq!(rem, r);
    let inv = BigUint::mod_inverse(&b, &a.add(&BigUint::from_u64(1))).unwrap();
    assert_eq!(inv.mul(&b).rem(&a.add(&BigUint::from_u64(1))), BigUint::from_u64(1));
    assert_eq!(format!("{:x}", BigUint::zero()), "0");
    assert_eq!(format!("{:x}", r), "1234567");
}
