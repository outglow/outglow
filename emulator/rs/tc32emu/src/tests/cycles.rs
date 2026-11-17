use super::thumb::*;
use crate::Machine;

fn cost(hw: u16, accurate: bool, setup: impl FnOnce(&mut Machine)) -> i64 {
    let mut m = machine();
    m.cycle_accurate = accurate;
    put(&mut m, CODE, &[hw]);
    m.r[15] = CODE;
    m.r[13] = STACK;
    m.r[1] = DATA;
    setup(&mut m);
    let c0 = m.cycles;
    m.step().unwrap();
    m.cycles - c0
}

#[test]
fn flat_cpi_charges_every_instruction_the_same() {
    let mov = cost(mov_imm(2, 1), false, |_| {});
    let mul = cost(alu(MUL, 2, 1), false, |m| m.r[2] = 3);
    let ldr = cost(ldr_imm(2, 1, 0), false, |_| {});
    let b = cost(HALT, false, |_| {});
    assert_eq!((mov, mul, ldr, b), (1, 1, 1, 1));
}

#[test]
fn cycle_accurate_mode_charges_arm7tdmi_sequential_and_nonsequential_cycles() {
    let mov = cost(mov_imm(2, 1), true, |_| {});
    let ldr = cost(ldr_imm(2, 1, 0), true, |_| {});
    let str = cost(str_imm(2, 1, 0), true, |_| {});
    let mul = cost(alu(MUL, 0, 1), true, |m| {
        m.r[0] = 7;
        m.r[1] = 9;
    });
    let taken = cost(HALT, true, |_| {});
    let not_taken = cost(0xD000 | EQ << 8 | 4, true, |m| m.z = 0);
    assert_eq!(mov, 1, "MOV: 1S");
    assert_eq!(ldr, 3, "LDR: 1S+1N+1I");
    assert_eq!(str, 2, "STR: 2N");
    assert!(mul >= 2 && mul > mov, "MUL: 1S+mI, got {}", mul);
    assert_eq!(taken, 3, "taken branch: 2S+1N");
    assert_eq!(not_taken, 1, "branch not taken: 1S");
}

#[test]
fn cycle_accurate_block_transfers_charge_one_cycle_per_register() {
    let ldm2 = cost(ldmia(1, 0x0C), true, |_| {});
    let ldm4 = cost(ldmia(1, 0x3C), true, |_| {});
    let push2 = cost(push(0x03), true, |_| {});
    let push3 = cost(push(0x07), true, |_| {});
    assert_eq!(ldm4 - ldm2, 2, "LDMIA: nS+1N+1I");
    assert_eq!(push3 - push2, 1, "PUSH: (n-1)S+2N");
}
