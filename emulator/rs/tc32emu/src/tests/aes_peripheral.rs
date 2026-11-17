use super::thumb::machine;
use super::{hex, unhex};
use crate::Machine;

fn block(m: &mut Machine, inp: &[u8]) -> Vec<u8> {
    for i in (0..16).step_by(4) {
        m.reg_write(0x548, 4, u32::from_le_bytes(inp[i..i + 4].try_into().unwrap())).unwrap();
    }
    let mut out = vec![];
    for _ in 0..4 {
        out.extend_from_slice(&m.reg_read(0x548, 4).unwrap().to_le_bytes());
    }
    out
}

fn encrypt(key: &[u8], pt: &[u8]) -> String {
    let mut m = machine();
    m.reg_write(0x540, 1, 0).unwrap();
    m.regs[0x550..0x560].copy_from_slice(key);
    hex(&block(&mut m, pt))
}

#[test]
fn aes_peripheral_matches_fips_197_appendix_c1() {
    let key: Vec<u8> = (0..16).collect();
    assert_eq!(encrypt(&key, &unhex("00112233445566778899aabbccddeeff")), "69c4e0d86a7b0430d8cdb78070b4c55a");
}

#[test]
fn aes_peripheral_encrypts_the_zero_block_under_the_zero_key() {
    assert_eq!(encrypt(&[0; 16], &[0; 16]), "66e94bd4ef8a2c3b884cfa59ca342b2e");
}

#[test]
fn aes_peripheral_ccm_mode_chains_blocks_like_cbc_mac() {
    let key: Vec<u8> = (0..16).collect();
    let p0 = unhex("00112233445566778899aabbccddeeff");
    let c0 = unhex("69c4e0d86a7b0430d8cdb78070b4c55a");
    let p1: Vec<u8> = p0.iter().zip(&c0).map(|(a, b)| a ^ b).collect();
    let mut m = machine();
    m.regs[0x550..0x560].copy_from_slice(&key);
    m.reg_write(0x540, 1, 0x80).unwrap();
    assert_eq!(hex(&block(&mut m, &p0)), hex(&c0), "first block: AES(P0)");
    m.reg_write(0x540, 1, 0x80).unwrap();
    assert_eq!(hex(&block(&mut m, &p1)), hex(&c0), "second block: AES(P1 xor C0) = AES(P0)");
    let mut e = machine();
    e.regs[0x550..0x560].copy_from_slice(&key);
    e.reg_write(0x540, 1, 0).unwrap();
    block(&mut e, &p0);
    e.reg_write(0x540, 1, 0).unwrap();
    assert_ne!(hex(&block(&mut e, &p1)), hex(&c0), "without CCM the second block is not chained");
}

#[test]
fn aes_peripheral_ccm_mode_starts_a_new_chain_after_leaving_it() {
    let key: Vec<u8> = (0..16).collect();
    let p0 = unhex("00112233445566778899aabbccddeeff");
    let mut m = machine();
    m.regs[0x550..0x560].copy_from_slice(&key);
    m.reg_write(0x540, 1, 0x80).unwrap();
    block(&mut m, &p0);
    m.reg_write(0x540, 1, 0).unwrap();
    m.reg_write(0x540, 1, 0x80).unwrap();
    assert_eq!(hex(&block(&mut m, &p0)), "69c4e0d86a7b0430d8cdb78070b4c55a");
}
