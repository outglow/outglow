use super::*;
use crate::sim;
use bulb_core::hal::ConfigStore;
use zigbee::hal::NvItem;

pub fn reset_statics() {
    crate::radio::tests::reset_radio_state();
    #[cfg(feature = "ble")]
    crate::ble_radio::tests::reset_statics();
    staging::release(staging::Owner::Zigbee);
    staging::release(staging::Owner::Ble);
    zb::BLE_NEXT.set(None);
    zb::LOOP_MAX_US.set(0);
}

pub fn crc32(data: &[u8]) -> u32 {
    let mut c = 0xffff_ffffu32;
    for &b in data {
        c ^= b as u32;
        for _ in 0..8 {
            c = if c & 1 != 0 { (c >> 1) ^ 0xedb8_8320 } else { c >> 1 };
        }
    }
    c
}

pub fn image(size: usize) -> Vec<u8> {
    let mut v: Vec<u8> = (0..size).map(|i| (i * 7 + 3) as u8).collect();
    v[8..12].copy_from_slice(b"\xffNLT");
    v[0x18..0x1c].copy_from_slice(&(size as u32).to_le_bytes());
    let mut sum = v[..size - 4].to_vec();
    sum[8] = 0x4b;
    let crc = crc32(&sum);
    v[size - 4..].copy_from_slice(&crc.to_le_bytes());
    v
}


#[test]
fn idling_arms_the_timer_and_waits_for_the_tick() {
    let _s = sim::session();
    crate::radio::tests::drain();
    let until = sim::with(|c| c.tick).wrapping_add(500 * reg::TICKS_PER_US);
    bulb_idle(until);
    assert!(sim::with(|c| c.writes()).contains(&sim::Op::W32(reg::SYSTEM_TICK_IRQ, until)));
    assert!((sim::with(|c| c.tick).wrapping_sub(until) as i32) >= 0);
}

#[test]
fn a_factory_reset_keeps_only_the_frame_counters() {
    let _s = sim::session();
    let mut nv = nv::open();
    nv.store(NvItem::Network as u8, &[1, 2, 3]);
    nv.store(NvItem::Counters as u8, &[4, 5]);
    board::FlashStore::new().store(&bulb_core::config::Config::default().encode());
    #[cfg(feature = "ble")]
    crate::flash::write(crate::ble::MESH_SECTOR, &[0]);
    factory_reset();
    let nv = nv::open();
    let mut buf = [0u8; 8];
    assert_eq!(nv.load(NvItem::Network as u8, &mut buf), None);
    assert_eq!(nv.load(NvItem::Counters as u8, &mut buf), Some(2));
    assert_eq!(board::FlashStore::new().load(&mut buf), None);
    #[cfg(feature = "ble")]
    assert!(crate::flash::blank(crate::ble::MESH_SECTOR));
}

const STOP: &str = "the test stops the main loop";

fn run_main(loops: usize) -> Vec<sim::Op> {
    zb::LINK.release();
    let mut feeds = 0;
    sim::with(|c| {
        c.on(move |_, op| {
            if op == sim::Op::W8(reg::TIMER_STATUS, 0x08) {
                feeds += 1;
                if feeds > loops {
                    panic!("{STOP}");
                }
            }
        })
    });
    let hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(|_| {}));
    let r = std::panic::catch_unwind(|| run());
    std::panic::set_hook(hook);
    let e = r.err().expect("the main loop never returns");
    assert_eq!(e.downcast_ref::<String>().map(String::as_str), Some(STOP));
    sim::with(|c| {
        c.hook = None;
        c.log.clone()
    })
}

#[test]
fn idling_waits_a_millisecond_or_until_300_us_before_the_next_ble_event() {
    let end = 0xffff_ff00u32;
    let us = reg::TICKS_PER_US;
    assert_eq!(idle_until(end, None), Some(end.wrapping_add(1_000 * us)));
    assert_eq!(idle_until(end, Some(end.wrapping_add(2_000 * us))), Some(end.wrapping_add(700 * us)));
    assert_eq!(idle_until(end, Some(end.wrapping_add(1_300 * us))), Some(end.wrapping_add(700 * us)));
    assert_eq!(idle_until(end, Some(end.wrapping_add(1_000 * us))), Some(end.wrapping_add(700 * us)));
    assert_eq!(idle_until(end, Some(end.wrapping_add(999 * us))), Some(end.wrapping_add(699 * us)));
    assert_eq!(idle_until(end, Some(end.wrapping_add(350 * us))), Some(end.wrapping_add(50 * us)));
    assert_eq!(idle_until(end, Some(end.wrapping_add(350 * us - 1))), None);
    assert_eq!(idle_until(end, Some(end)), None);
    assert_eq!(idle_until(end, Some(end.wrapping_sub(1))), None);
    assert_eq!(idle_until(end, Some(end.wrapping_add(0x8000_0000))), None);
    assert_eq!(idle_until(end, Some(end.wrapping_add(0x7fff_ffff))), Some(end.wrapping_add(700 * us)));
}

fn feeds(log: &[sim::Op]) -> Vec<usize> {
    log.iter().enumerate().filter(|(_, o)| **o == sim::Op::W8(reg::TIMER_STATUS, 0x08)).map(|(i, _)| i).collect()
}

#[test]
fn a_cold_start_counts_the_power_cycle_starts_the_watchdog_and_runs_the_bulb() {
    let _s = sim::session();
    let log = run_main(40);
    let w: Vec<sim::Op> = log.iter().copied().filter(sim::Op::is_write).collect();
    assert_eq!(w[..2].to_vec(), vec![sim::Op::W8(reg::IRQ_ENABLE, 0), sim::Op::W32(reg::IRQ_MASK, 0)]);
    assert_eq!([link::BULB_NET.read(4), link::BULB_NET.read(5), link::BULB_NET.read(6)], [1, 0, 0]);
    let mut word = [0u8; 4];
    crate::flash::read(0xc_f000, &mut word);
    assert_eq!(word, [0x00, 0x00, 0xfe, 0xff]);
    let wd = w.iter().position(|o| *o == sim::Op::W32(reg::TIMER2_TICK, 0)).expect("the watchdog is started");
    assert_eq!(w[wd + 1], sim::Op::W32(reg::TIMER_CTRL, 366 << 9 | 1 << 6 | 1 << 23));
    assert_eq!(w[wd + 2], sim::Op::W8(reg::TIMER_STATUS, 0x08));
    assert_eq!(w[wd + 3], sim::Op::W8(reg::SYSTEM_TICK_MODE, 0x02));
    let f = feeds(&log);
    assert!(f.len() >= 40);
    let first_loop = f.iter().position(|&i| i > log.iter().position(|o| *o == sim::Op::W8(reg::SYSTEM_TICK_MODE, 0x02)).unwrap_or(0)).unwrap_or(0);
    let mut longest = 0;
    let mut idles = 0;
    for p in f[first_loop..].windows(2) {
        let seg = &log[p[0]..p[1]];
        let start = seg.iter().find_map(|o| match *o {
            sim::Op::R32(reg::SYSTEM_TICK, t) => Some(t),
            _ => None,
        });
        let idle = seg.iter().position(|o| matches!(o, sim::Op::W32(reg::SYSTEM_TICK_IRQ, _)));
        let end_at = idle.unwrap_or(seg.len());
        let end = seg[..end_at].iter().rev().find_map(|o| match *o {
            sim::Op::R32(reg::SYSTEM_TICK, t) => Some(t),
            _ => None,
        });
        if let (Some(s), Some(e)) = (start, end) {
            longest = longest.max(e.wrapping_sub(s) / reg::TICKS_PER_US);
        }
        if let (Some(i), Some(e)) = (idle, end) {
            idles += 1;
            let sim::Op::W32(_, until) = seg[i] else { unreachable!() };
            let wait = until.wrapping_sub(e);
            assert!(wait == 1_000 * 16 || (50 * 16..=700 * 16).contains(&wait), "{wait}");
        }
    }
    assert!(idles > 0);
    let span = sim::with(|c| c.ticks_read());
    let total = span.last().copied().unwrap_or(0).wrapping_sub(span[0]) / reg::TICKS_PER_US;
    assert!(longest > 16 && longest <= zb::LOOP_MAX_US.get() && zb::LOOP_MAX_US.get() <= total, "{longest} {} {total}", zb::LOOP_MAX_US.get());
}

#[test]
fn a_warm_start_does_not_count_a_power_cycle() {
    let _s = sim::session();
    sim::with(|c| c.set_analog(0x3a, 0x01));
    run_main(3);
    assert_eq!([link::BULB_NET.read(4), link::BULB_NET.read(5), link::BULB_NET.read(6)], [0, 0, 1]);
    assert!(crate::flash::blank(0xc_f000));
}

#[test]
fn the_fifth_quick_power_cycle_resets_the_bulb_to_factory_settings() {
    let _s = sim::session();
    sim::with(|c| {
        c.flash[0xc_f000..0xc_f004].copy_from_slice(&[0x00, 0x00, 0x80, 0xff]);
    });
    nv::open().store(NvItem::Network as u8, &[1, 2, 3]);
    nv::open().store(NvItem::Counters as u8, &[0xe8, 0x03, 0, 0, 0xd0, 0x07, 0, 0]);
    run_main(3);
    assert_eq!(link::BULB_NET.read(5), 1);
    let mut buf = [0u8; 8];
    assert_eq!(nv::open().load(NvItem::Network as u8, &mut buf), None);
    assert_eq!(nv::open().load(NvItem::Counters as u8, &mut buf), Some(8));
    assert!(u32::from_le_bytes([buf[0], buf[1], buf[2], buf[3]]) > 1_000);
}

#[test]
fn idling_ends_on_the_tick_or_as_soon_as_a_frame_is_waiting() {
    let _s = sim::session();
    crate::radio::tests::drain();
    let until = sim::with(|c| c.tick).wrapping_add(300 * reg::TICKS_PER_US);
    bulb_idle(until);
    assert_eq!(sim::with(|c| c.ticks_read()).last().copied(), Some(until));
    crate::radio::tests::queue(&[0x00, 0x80, 0x01, 0x34, 0x12], 60, 0);
    sim::with(|c| c.log.clear());
    bulb_idle(sim::with(|c| c.tick).wrapping_add(300 * reg::TICKS_PER_US));
    assert_eq!(sim::with(|c| c.ticks_read()).len(), 1);
    let mut buf = [0u8; 127];
    assert!(crate::radio::receive(&mut buf).is_some());
}

#[test]
fn every_zigbee_item_up_to_the_last_is_kept_in_flash() {
    let _s = sim::session();
    for id in 1..=NvItem::MAX_ID {
        nv::open().store(id, &[id, 0x5a]);
    }
    for _ in 0..60 {
        nv::open().store(NvItem::Network as u8, &[0x33; 120]);
    }
    let nv = nv::open();
    let mut buf = [0u8; 4];
    for id in 2..=NvItem::MAX_ID {
        assert_eq!(nv.load(id, &mut buf), Some(2), "item {id}");
        assert_eq!(buf[..2], [id, 0x5a]);
    }
}
