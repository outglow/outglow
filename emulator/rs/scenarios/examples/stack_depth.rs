//! The deepest the main stack of a Rust image has been, at a few moments of a
//! session in which it joins the coordinator (scan of the SRAM, see
//! `sims::util::stack_report`). After the start (2 s) the part of the stack
//! below the one in use is cleared once, so that the later lines show how
//! deep the running bulb goes, apart from the start.
//!
//! `cargo run -p scenarios --example stack_depth -- [image.bin]`

use scenarios::image::Image;
use scenarios::session::{Session, SessionOpts};

fn main() {
    std::env::set_var("SIM_STACK", "1");
    let path = std::env::args().nth(1).unwrap_or_else(|| "target/tc32/bulb-tc32.bin".into());
    let img = Image::parse(&format!("rust={path}")).unwrap();
    let floor = img.symbol("_end_bss_").expect("the image's ELF with _end_bss_");
    let top = img.symbol("_stack_end_").expect("_stack_end_");
    let mut s = Session::new(&img, SessionOpts { cpi: 3, join: false, ..Default::default() }).unwrap();
    let mut cleared = false;
    for t in [5.0, 20.0, 200.0, 1000.0, 2000.0, 3000.0, 10000.0, 30000.0] {
        let dt = t - s.now();
        s.run(dt).unwrap();
        if t >= 2000.0 && !cleared {
            // only while the main stack pointer is the one in r13 (not in an interrupt)
            while !cleared {
                let sp = s.w.air.dut.m.r[13];
                if (floor..top).contains(&sp) {
                    let (lo, hi) = ((floor - 0x840000) as usize, (sp - 0x840000) as usize - 64);
                    s.w.air.dut.m.sram[lo..hi].fill(0);
                    cleared = true;
                    println!("{:8.0} ms: cleared below {:#x}", s.now(), sp - 64);
                } else {
                    s.run(0.01).unwrap();
                }
            }
        }
        let l = scenarios::sims::util::stack_report(&s.w.air.dut, &img).unwrap_or_default();
        println!("{:8.0} ms joined={}{l}", s.now(), s.w.coord.devices.iter().any(|d| d.announced));
    }
}
