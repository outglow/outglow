// SPDX-License-Identifier: Apache-2.0
//! zbnode: a TLSR8258 firmware image in the tc32emu port with the 8258 layer,
//! the 802.15.4 radio and the BLE model, driven in lockstep over stdin/stdout
//! by the scenario scripts. A port of emulator/go/cmd/zbnode (main.go): the
//! same arguments, environment (ZBNODE_*, TC32EMU_*) and output, line for
//! line. The protocol is described there.
//!
//! The node itself is the library (`lib.rs`, [`zbnode::Node`]); this binary
//! only speaks the protocol.

use std::cell::RefCell;
use std::env;
use std::fmt::Write as _;
use std::io::{self, BufRead, Write};
use std::process;
use std::rc::Rc;

use ble::gofmt::{f as ff, g, hex, unhex};
use ble::{default_options, Central, CentralRc};
use tc32emu::Machine;

use zbnode::gostr::{atoi, float0, hex_i64, parse_uint_lossy};
use zbnode::{bleatt, blespec, must, Frame, Node, RunEvent};

type Out = Rc<RefCell<String>>;

fn flush(out: &Out) {
    let mut o = out.borrow_mut();
    if o.is_empty() {
        return;
    }
    let mut so = io::stdout().lock();
    let _ = so.write_all(o.as_bytes());
    let _ = so.flush();
    o.clear();
}

fn main() {
    let args: Vec<String> = env::args().collect();
    if args.len() < 2 {
        eprintln!("usage: zbnode <image.bin> [cpi] [link_offset_hex]");
        process::exit(2);
    }
    let img = must(std::fs::read(&args[1]).map_err(|e| format!("open {}: {}", args[1], io_err_text(&e))));
    let mut cpi = 3i64;
    if args.len() > 2 {
        cpi = atoi(&args[2]);
    }
    let link_shift = if args.len() > 3 { hex_i64(&args[3]) } else { 0 };
    let mut node = must(Node::new(&img, cpi, link_shift));

    let mut central: Option<CentralRc> = None;
    let mut attq: Option<bleatt::QueueRc> = None;
    let (mut rsp_sent, mut ntf_sent) = (0usize, 0usize);
    let mut spec = blespec::Spec::default();
    let out: Out = Rc::new(RefCell::new(String::new()));

    let flush_ble = |out: &Out, attq: &Option<bleatt::QueueRc>, central: &Option<CentralRc>, rsp_sent: &mut usize, ntf_sent: &mut usize| {
        let Some(q) = attq else { return };
        let mut o = out.borrow_mut();
        let q = q.borrow();
        while *rsp_sent < q.rsps.len() {
            let x = &q.rsps[*rsp_sent];
            let _ = writeln!(o, "ATTRSP {} {}", ff(x.ms, 6), hex(&x.data));
            *rsp_sent += 1;
        }
        let c = central.as_ref().unwrap().borrow();
        while *ntf_sent < c.ntf.len() {
            let n = &c.ntf[*ntf_sent];
            let _ = writeln!(o, "ATTNTF {} {} {}", ff(n.ms, 6), n.handle, n.value.replace(' ', "_"));
            *ntf_sent += 1;
        }
    };

    let stdin = io::stdin();
    let mut input = stdin.lock();
    let mut line = Vec::new();
    loop {
        line.clear();
        match input.read_until(b'\n', &mut line) {
            Ok(0) | Err(_) => break,
            Ok(_) => {}
        }
        if line.last() == Some(&b'\n') {
            line.pop();
        }
        // bufio.Scanner with a 1 MB buffer: a longer line ends the input.
        if line.len() >= 1 << 20 {
            break;
        }
        if line.last() == Some(&b'\r') {
            line.pop();
        }
        let text = String::from_utf8_lossy(&line);
        let f: Vec<String> = text.split_whitespace().map(|s| s.to_string()).collect();
        if f.is_empty() {
            continue;
        }
        let mut o = String::new();
        match f[0].as_str() {
            "RUN" => {
                let target = float0(&f[1]);
                loop {
                    // in the order Go writes its buffered output: what this loop printed, then the ATT lines
                    out.borrow_mut().push_str(&o);
                    o.clear();
                    flush_ble(&out, &attq, &central, &mut rsp_sent, &mut ntf_sent);
                    match node.step_run(target) {
                        RunEvent::Tx(fr) => {
                            let _ = writeln!(o, "TX {} {} {} {}", ff(fr.t0, 6), ff(fr.t1, 6), fr.channel, hex(&fr.psdu));
                            break;
                        }
                        RunEvent::BleTx(a) => {
                            let _ = writeln!(o, "BLETX {} {} {} {:08x} {}", ff(a.t0, 6), ff(a.t1, 6), a.ch, a.aa, hex(&a.pdu));
                            break;
                        }
                        RunEvent::At => {
                            let _ = writeln!(o, "AT {}", ff(node.m.ms(), 6));
                            break;
                        }
                        RunEvent::Ran => {}
                        RunEvent::SramBoot { pc, ram_code } => {
                            let _ = writeln!(o, "RESET {} sram_boot {:#x} ram_code={:#x}", ff(node.m.ms(), 6), pc, ram_code);
                        }
                        RunEvent::Reset { cause, pc } => {
                            let _ = writeln!(o, "RESET {} {} {:#x} slot={:#x}", ff(node.m.ms(), 6), cause.replace(' ', "_"), pc, node.m.boot_slot);
                        }
                        RunEvent::Error(e) => {
                            let _ = writeln!(o, "ERROR {} {} pc={:#x}", ff(node.m.ms(), 6), e, node.m.r[15]);
                            out.borrow_mut().push_str(&o);
                            flush(&out);
                            process::exit(1);
                        }
                        RunEvent::Fatal(e) => {
                            eprintln!("{}", e);
                            process::exit(1);
                        }
                    }
                }
            }
            "RX" => {
                let t0 = float0(&f[1]);
                let t1 = float0(&f[2]);
                let ch = atoi(&f[3]);
                let rssi = atoi(&f[4]);
                let (psdu, _) = unhex(&f[5]);
                node.inject_rx(Frame { t0, t1, channel: ch, psdu }, rssi);
                o.push_str("OK\n");
            }
            "BLERX" => {
                let t0 = float0(&f[1]);
                let t1 = float0(&f[2]);
                let ch = atoi(&f[3]);
                let aa = parse_uint_lossy(&f[4], 16, 32);
                let (pdu, _) = unhex(&f[5]);
                node.ble_radio.air_rx(&mut node.m, pdu, ch as u8, aa as u32, t0, t1);
                o.push_str("OK\n");
            }
            "BLEAIR" => {
                let br = node.ble_radio.b();
                let _ = write!(o, "BLEAIR ok={} miss={}", br.air_rx_ok, br.air_rx_miss.len());
                let ms = &br.air_rx_miss[br.air_rx_miss.len().saturating_sub(6)..];
                for x in ms {
                    let _ = write!(o, " [{} ch{} {}]", ff(x.ms, 3), x.ch, x.why.replace(' ', "_"));
                }
                o.push('\n');
            }
            "STATE" => {
                let s = node.radio.b();
                let m = &node.m;
                let dropped: i64 = s.dropped.values().sum();
                let _ = writeln!(
                    o,
                    "STATE {} {} {:02x} {} {} {} {} {:02x} {:02x}",
                    ff(m.ms(), 6),
                    hex(&m.regs[0x780..0x7B0]),
                    m.regs[0x596],
                    node.resets,
                    s.sent,
                    s.received,
                    dropped,
                    m.regs[0x58e],
                    m.regs[0x5ab]
                );
            }
            "DROPS" => {
                let s = node.radio.b();
                let parts: Vec<String> = s.dropped.iter().map(|(k, n)| format!("{}={}", k.replace(' ', "_"), n)).collect();
                let _ = writeln!(o, "DROPS {}", parts.join(" "));
            }
            "FLASH" => {
                let a = hex_i64(&f[1]) as usize;
                let n = atoi(&f[2]) as usize;
                let _ = writeln!(o, "FLASH {}", hex(&node.m.flash.mem[a..a + n]));
            }
            "MEM" => {
                let m = &node.m;
                let a = hex_i64(&f[1]);
                let n = atoi(&f[2]);
                if a >= 0x840000 && a + n <= 0x840000 + m.sram.len() as i64 {
                    let s = (a - 0x840000) as usize;
                    let _ = writeln!(o, "MEM {}", hex(&m.sram[s..s + n as usize]));
                } else if a >= 0x800000 && a + n <= 0x800000 + m.regs.len() as i64 {
                    let s = (a - 0x800000) as usize;
                    let _ = writeln!(o, "MEM {}", hex(&m.regs[s..s + n as usize]));
                } else if a >= 0 && a + n <= m.flash.mem.len() as i64 {
                    let _ = writeln!(o, "MEM {}", hex(&m.flash.mem[a as usize..(a + n) as usize]));
                } else {
                    let _ = writeln!(o, "ERROR bad range {:#x}+{}", a, n);
                }
            }
            "BP" => {
                let a = hex_i64(&f[1]);
                let mut n = 64i64;
                if f.len() > 2 {
                    n = atoi(&f[2]);
                }
                let mut reg = 0usize;
                if f.len() > 3 {
                    reg = atoi(&f[3]) as usize;
                }
                let bo = out.clone();
                node.m.hooks.insert((a & !1) as u32, Rc::new(move |mm: &mut Machine| {
                    let r0 = mm.r[reg];
                    let nn = n as u32;
                    let b: &[u8] = if r0 >= 0x840000 && r0.wrapping_add(nn) <= 0x840000 + mm.sram.len() as u32 {
                        &mm.sram[(r0 - 0x840000) as usize..(r0 - 0x840000 + nn) as usize]
                    } else if r0.wrapping_add(nn) <= mm.flash.mem.len() as u32 {
                        &mm.flash.mem[r0 as usize..(r0 + nn) as usize]
                    } else {
                        &[]
                    };
                    let mut hs = hex(b);
                    if hs.is_empty() {
                        hs = "-".into();
                    }
                    let _ = writeln!(
                        bo.borrow_mut(),
                        "BP {} {:#x} {:#x} {:#x} {:#x} {:#x} {:#x} {}",
                        ff(mm.ms(), 6),
                        mm.r[15],
                        mm.r[0],
                        mm.r[1],
                        mm.r[2],
                        mm.r[3],
                        mm.r[14],
                        hs
                    );
                    flush(&bo);
                    Ok(())
                }));
                o.push_str("OK\n");
            }
            "WRITEFLASH" => {
                let (data, _) = unhex(&f[2]);
                node.write_flash(hex_i64(&f[1]) as usize, &data);
                o.push_str("OK\n");
            }
            "ERASE" => {
                node.erase(hex_i64(&f[1]) as usize, hex_i64(&f[2]) as usize);
                o.push_str("OK\n");
            }
            "REBOOT" => {
                must(node.reboot());
                let _ = writeln!(o, "OK slot={:#x}", node.m.boot_slot);
            }
            "SOFTRESET" => {
                must(node.soft_reset(f.len() > 1 && f[1] == "wd"));
                let _ = writeln!(o, "OK slot={:#x}", node.m.boot_slot);
            }
            "ANALOG" => {
                let a = (hex_i64(&f[1]) & 0xFF) as usize;
                if f.len() > 2 {
                    node.m.analog[a] = hex_i64(&f[2]) as u8;
                }
                let _ = writeln!(o, "ANALOG {:02x}", node.m.analog[a]);
            }
            "BLECONNECT" => {
                let iv = atoi(&f[1]);
                let mut op = default_options();
                op.log = None;
                op.pair = false;
                op.interval = iv;
                op.timeout = 400;
                op.max_exch = 8;
                op.connect_after_ms = node.m.ms();
                if f.len() > 2 {
                    let scan = float0(&f[2]);
                    op.active_scan = true;
                    op.connect_after_ms = node.m.ms() + scan;
                }
                let q = bleatt::new_queue();
                op.att_script = Some(q.clone());
                attq = Some(q);
                central = Some(Central::new(&mut node.m, &node.ble_radio, op));
                rsp_sent = 0;
                ntf_sent = 0;
                o.push_str("OK\n");
            }
            "BLEDISCONNECT" => {
                if let Some(c) = &central {
                    c.borrow_mut().terminate(0x13);
                }
                o.push_str("OK\n");
            }
            "BLEADV" => match &central {
                Some(c) if c.borrow().adv_pdu.len() >= 8 => {
                    let _ = writeln!(o, "BLEADV {}", hex(&c.borrow().adv_pdu[8..]));
                }
                _ => o.push_str("BLEADV -\n"),
            },
            "BLESTATE" => match &central {
                None => o.push_str("BLESTATE none - 0\n"),
                Some(c) => {
                    let c = c.borrow();
                    let adv = if c.adv_a.is_empty() { "-".to_string() } else { hex(&c.adv_a) };
                    let _ = writeln!(o, "BLESTATE {} {} {}", c.state, adv, bleatt::started(attq.as_ref().unwrap()));
                }
            },
            "PC" => {
                let m = &node.m;
                let rg = &m.regs;
                let _ = writeln!(
                    o,
                    "PC {:#x} lr={:#x} sp={:#x} irq_en={:#x} ibit={} src={:#x} mask={:#x}",
                    m.r[15],
                    m.r[14],
                    m.r[13],
                    rg[0x643],
                    m.ibit,
                    rg[0x648] as u32 | (rg[0x649] as u32) << 8 | (rg[0x64a] as u32) << 16,
                    rg[0x640] as u32 | (rg[0x641] as u32) << 8 | (rg[0x642] as u32) << 16
                );
            }
            "BLELOG" => {
                let n = atoi(&f[1]);
                let c = central.as_ref().expect("BLELOG without a central").borrow();
                let tr = &c.transcript;
                let tr = if tr.len() as i64 > n { &tr[(tr.len() as i64 - n) as usize..] } else { &tr[..] };
                for l in tr {
                    let _ = writeln!(o, "LOG {}", l);
                }
                for e in &c.att_log {
                    let _ = writeln!(o, "LOG att {} {} {} {}", g(e.ms), e.dir, e.name, e.hex);
                }
                let br = node.ble_radio.b();
                for x in &br.misses[br.misses.len().saturating_sub(8)..] {
                    let _ = writeln!(o, "LOG miss {} ch{} {}", ff(x.ms, 3), x.ch, x.why);
                }
                for e in &br.fifo_log[br.fifo_log.len().saturating_sub(12)..] {
                    let _ = writeln!(o, "LOG fifo {}", e.go_plus_v());
                }
                let c0c = node.m.regs[0xC0C] as u16 | (node.m.regs[0xC0D] as u16) << 8;
                let _ = writeln!(o, "LOG fifo rptr={} wptr={} c0c={:#x}", br.rptr, br.wptr, c0c);
                let _ = writeln!(
                    o,
                    "LOG stats events={} replies={} missed={} retx={} state={}",
                    c.stats.events, c.stats.replies, c.stats.missed, c.stats.retx, c.state
                );
                o.push_str("END\n");
            }
            "BLECRCERR" => {
                node.crc_err_every.set(atoi(&f[1]));
                let _ = writeln!(o, "OK {}", node.crc_errs.get());
            }
            "BLESTAT" => {
                let st = central.as_ref().map(|c| c.borrow().stats).unwrap_or_default();
                let br = node.ble_radio.b();
                let _ = writeln!(
                    o,
                    "BLESTAT events={} replies={} missed={} retx={} late={} misses={} advfifo={} refused={} cmds={} scanreqs={}",
                    st.events,
                    st.replies,
                    st.missed,
                    st.retx,
                    br.late_starts.len(),
                    br.misses.len(),
                    br.adv_fifo.len(),
                    br.rf_refused.len(),
                    br.cmds.len(),
                    br.scan_reqs.len()
                );
            }
            "BLERF" => {
                let from = float0(&f[1]);
                let br = node.ble_radio.b();
                for c in &br.cmds {
                    if c.ms >= from {
                        let _ = writeln!(o, "RF {} cmd {:#x} ch {}", ff(c.ms, 3), c.cmd, c.ch);
                    }
                }
                for a in &br.adv_tx {
                    if a.ms >= from {
                        let _ = writeln!(o, "RF {} adv ch {} {}", ff(a.ms, 3), a.ch, hex(&a.pdu));
                    }
                }
                o.push_str("END\n");
            }
            "CALL" => {
                let parse = |s: &str| -> u32 { hex_i64(s) as u32 };
                let args: Vec<u32> = (0..4).filter(|i| f.len() > 2 + i).map(|i| parse(&f[2 + i])).collect();
                let budget = if f.len() > 6 { atoi(&f[6]) } else { 50000000 };
                match node.call(parse(&f[1]), &args, budget) {
                    zbnode::CallOutcome::Err { r0, steps, pc, err } => {
                        let _ = writeln!(o, "CALLERR {:#x} steps={} pc={:#x} {}", r0, steps, pc, err.replace(' ', "_"));
                    }
                    zbnode::CallOutcome::Timeout { r0, steps, pc } => {
                        let _ = writeln!(o, "CALLTIMEOUT {:#x} steps={} pc={:#x}", r0, steps, pc);
                    }
                    zbnode::CallOutcome::Done { r, pc, steps } => {
                        let _ = writeln!(o, "CALLDONE {:#x} {:#x} {:#x} {:#x} pc={:#x} steps={}", r[0], r[1], r[2], r[3], pc, steps);
                    }
                }
            }
            "ATT" => {
                let (pdu, _) = unhex(&f[1]);
                bleatt::push(attq.as_ref().expect("ATT without BLECONNECT"), central.as_ref().unwrap(), &mut node.m, pdu);
                o.push_str("OK\n");
            }
            "SLOT" => {
                let _ = writeln!(o, "SLOT {:#x}", node.m.boot_slot);
            }
            "QUIT" => {
                out.borrow_mut().push_str(&o);
                flush(&out);
                return;
            }
            _ => {
                // BS...: the commands of the BLE conformance suite (blespec.rs)
                spec.command(&f, &mut o, &mut node.m, &node.ble_radio);
            }
        }
        out.borrow_mut().push_str(&o);
        flush(&out);
    }
}

fn io_err_text(e: &io::Error) -> String {
    match e.kind() {
        io::ErrorKind::NotFound => "no such file or directory".into(),
        io::ErrorKind::PermissionDenied => "permission denied".into(),
        _ => e.to_string(),
    }
}
