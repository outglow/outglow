// go/ble/aes128/aes128_test.go
use super::{hex, unhex};
use crate::aes128::*;

fn big(s: &str) -> u128 {
    u128::from_str_radix(s, 16).unwrap()
}

#[test]
fn fips197() {
    let k: Vec<u8> = (0..16).collect();
    let p = unhex("00112233445566778899aabbccddeeff");
    let c = encrypt(&k, &p);
    assert_eq!(hex(&c), "69c4e0d86a7b0430d8cdb78070b4c55a");
    assert_eq!(decrypt(&k, &c).to_vec(), p);
    let t = tables();
    assert!(t.sbox[0x53] == 0xED && t.inv[0xED] == 0x53, "S-box");
}

#[test]
fn rfc3610() {
    let key = unhex("c0c1c2c3c4c5c6c7c8c9cacbcccdcecf");
    let nonce = unhex("00000003020100a0a1a2a3a4a5");
    let pkt: Vec<u8> = (0..31).collect();
    let out = ccm(&key, &nonce, &pkt[..8], &pkt[8..], 8);
    assert_eq!(hex(&out), "588c979a61c663d2f066d0c2c0f989806d5f6b61dac38417e8d12cfdf926e0");
}

#[test]
fn smp() {
    let c = c1(
        0,
        big("5783D52156AD6F0E6388274EC6702EE0"),
        &unhex("01010000100707"),
        &unhex("02030000080005"),
        1,
        &unhex("a6a5a4a3a2a1"),
        0,
        &unhex("b6b5b4b3b2b1"),
    );
    assert_eq!(c, big("1E1E3FEF878988EAD2A74DC5BEF13B86"));
    let s = s1(0, big("000F0E0D0C0B0A091122334455667788"), big("010203040506070899AABBCCDDEEFF00"));
    assert_eq!(s, big("9A1FE1F0E8B0F49B5B4216AE796DA062"));
}

#[test]
fn ah_rpa() {
    assert_eq!(ah(big("EC0234A357C8AD05341010A60A397D9B"), 0x708194), 0x0DFBAA);
    let irk = unhex("9b7d390aa610103405adc857a33402ec");
    assert_eq!(hex(&rpa(&irk, 0x708194)), "aafb0d948170");
}

#[test]
fn ll_encryption() {
    let sk = e(big("4C68384139F574D836BCF34E9DFB01BF"), big("0213243546576879ACBDCEDFE0F10213"));
    assert_eq!(sk, big("99AD1B5226A37E3E058E3B8E27C2C666"));
    let mut nonce = vec![0, 0, 0, 0, 0x80];
    nonce.extend(unhex("24abdcbabebaafde"));
    let enc = ccm_encrypt(&bytes128(sk), &nonce, 0x0F, &[0x06]);
    assert_eq!(hex(&enc), "9fcda7f448");
    let (dec, ok) = ccm_decrypt(&bytes128(sk), &nonce, 0x0F, &enc);
    assert!(ok && dec == vec![6]);
    let tail = ccm_encrypt(&bytes128(sk), &nonce, 0x0F, &[]);
    let (short, ok) = ccm_decrypt(&bytes128(sk), &nonce, 0x0F, &[0, 0, tail[tail.len() - 1]]);
    assert!(!ok && short.is_empty());
}

#[test]
fn isa_inverse() {
    for hw in 0..=0xFFFFu32 {
        let hw = hw as u16;
        assert_eq!(crate::isa::to_tc32(crate::isa::to_thumb(hw)), hw);
    }
}
