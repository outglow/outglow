// SPDX-License-Identifier: Apache-2.0
//! The Go package's tests (ble_test.go, air_test.go, tpll_test.go), ported:
//! the radio, reset, central, air and TPLL checks of checks/ble_models_check.py
//! and checks/ble_air_check.py, on a machine that is never run (only its
//! registers, memory and clock). pyrandom_test.go and sc_test.go are next to
//! their modules.

use std::cell::RefCell;
use std::rc::Rc;

use tc32emu::{aes128, Config, Flash, Machine};

use crate::air::{Air, MILLION, TO_HOST, TO_KEYBOARD, WAY_NAMES};
use crate::central::{default_options, fmt_pdu, Central, CentralRc, Options, ParamRequest, ScriptValue};
use crate::gofmt::hex;
use crate::pyrandom::PyRandom;
use crate::radio::{ll_name_of, rf_suspend_regs, Peer, PeerRc, RadioRef, TpllPeer, TpllTx, ADV_AA, DONE, RX, TX};
use crate::{millionths, round3, same_peer, settings};

#[derive(Default)]
struct Recorder {
    replies: RefCell<Vec<Vec<u8>>>,
}

impl Peer for Recorder {
    fn on_adv(&self, _m: &mut Machine, _pdu: &[u8], _ch: u8) -> Option<Vec<u8>> {
        None
    }
    fn on_connect_sent(&self, _m: &mut Machine, _pdu: &[u8], _t: f64) {}
    fn drop_pdu(&self, _m: &mut Machine, _pdu: &[u8]) -> bool {
        false
    }
    fn on_air(&self, _m: &mut Machine, pdu: &[u8], _ch: u8, _tx: u32) -> Option<Vec<u8>> {
        self.replies.borrow_mut().push(pdu.to_vec());
        None
    }
}

fn machine() -> Machine {
    let cfg = Config::env();
    let mut fl = Flash::new(cfg.flash_size);
    fl.mem[8] = 0x4B; // the boot ROM's bootable-slot mark
    Machine::new(fl, -1, 1).unwrap()
}

fn run_until(m: &mut Machine, ms: f64) {
    while m.ms() < ms {
        m.cycles += m.cpu_hz / 100_000; // 10 us
        m.update_time().unwrap();
    }
}

fn w(m: &mut Machine, o: usize, size: usize, v: u32) {
    m.reg_write(o, size, v).unwrap();
}

fn rd(m: &mut Machine, o: usize, size: usize) -> u32 {
    m.reg_read(o, size).unwrap()
}

fn wbytes(m: &mut Machine, a: u32, b: &[u8]) {
    for (i, &v) in b.iter().enumerate() {
        m.write(a + i as u32, 1, v as u32).unwrap();
    }
}

fn read_bytes(m: &mut Machine, a: u32, n: usize) -> Vec<u8> {
    (0..n).map(|i| m.read(a + i as u32, 1).unwrap() as u8).collect()
}

fn quiet() -> Options {
    let mut o = default_options();
    o.log = None;
    o
}

fn peer_of(c: &CentralRc) -> PeerRc {
    c.clone()
}

fn is(p: &Option<PeerRc>, c: &CentralRc) -> bool {
    p.as_ref().is_some_and(|x| same_peer(x, &peer_of(c)))
}

#[test]
fn radio() {
    let mut m = machine();
    let m = &mut m;
    let r = RadioRef::new(m, None);
    w(m, 0x74A, 1, 0xC1 | 0x02);

    RadioRef::set_status(m, 0x21);
    w(m, 0xF20, 2, 0x0001);
    assert_eq!(rd(m, 0xF20, 2), 0x0020, "RF status write-one-to-clear");

    let (empty, data, rxbuf) = (0x840100u32, 0x840140u32, 0x840200u32);
    wbytes(m, empty, &[2, 0, 0, 0, 0x01, 0]);
    wbytes(m, data, &[5, 0, 0, 0, 0x02, 3, b'a', b'b', b'c']);
    w(m, 0xC2A, 1, 0x10);
    w(m, 0xC2C, 2, empty & 0xFFFF);
    w(m, 0xC2C, 2, data & 0xFFFF);
    let fifo = [rd(m, 0xC2A, 1), rd(m, 0xC2B, 1)];
    w(m, 0xC2A, 1, 0x40 | 1);
    let set1 = rd(m, 0xC2A, 1);
    w(m, 0xC2A, 1, 0x20);
    let step = rd(m, 0xC2A, 1);
    w(m, 0xC2A, 1, 0x40);
    assert!(fifo == [0, 2] && set1 == 1 && step == 2 && rd(m, 0xC2A, 1) == 0, "TX FIFO: {:?} {} {}", fifo, set1, step);

    let aa = [0x71u8, 0x76, 0x41, 0x29];
    for (i, &v) in aa.iter().enumerate() {
        w(m, 0x408 + i, 1, v as u32);
    }
    w(m, 0x40D, 1, 7);
    w(m, 0xF03, 1, 0x10);
    w(m, 0xC08, 2, rxbuf & 0xFFFF);
    w(m, 0xC0C, 2, empty & 0xFFFF);
    let start = m.stimer_now().wrapping_add(1600);
    w(m, 0xF18, 4, start);
    let f16 = rd(m, 0xF16, 1);
    w(m, 0xF16, 1, f16 | 4);
    w(m, 0xF00, 1, 0x82);
    let rec = Rc::new(Recorder::default());
    r.b().central = Some(rec.clone());
    let pdu = [0x01u8, 0];
    let early = r.central_tx(m, &pdu, 7, &aa);
    let t0 = m.ms() + 0.2;
    run_until(m, t0);
    let anchor = m.stimer_now();
    let ok = r.central_tx(m, &pdu, 7, &aa);
    run_until(m, t0 + 1.0);
    assert!(!early && !r.b().misses.is_empty() && r.b().misses[0].why == "BRX scheduled later" && ok, "BRX at 0xf18");
    let ent = read_bytes(m, rxbuf, 20);
    let ts = u32::from_le_bytes([ent[9], ent[10], ent[11], ent[12]]);
    assert!(
        u32::from_le_bytes([ent[0], ent[1], ent[2], ent[3]]) == 13 && ent[4..6] == pdu && ts.wrapping_sub(anchor) == 0x500,
        "RX DMA entry: {}",
        hex(&ent)
    );
    let replies = rec.replies.borrow().clone();
    assert!(
        replies.len() == 1 && hex(&replies[0]) == "0603616263" && rd(m, 0xC2A, 1) == 1 && rd(m, 0xF22, 1) & 1 == 0 && rd(m, 0xF23, 1) == 0x10,
        "ack and reply"
    );
    let before = r.b().late_starts.len();
    let ago = m.stimer_now().wrapping_sub(1600);
    w(m, 0xF18, 4, ago);
    w(m, 0xF00, 1, 0x82);
    let ls = r.b().late_starts.clone();
    assert!(before == 0 && ls.len() == 1 && ls[0][1] >= 99.0 && ls[0][1] <= 101.0 && r.b().brx.is_some(), "late BRX");
}

#[test]
fn radio_reset() {
    let mut m = machine();
    let m = &mut m;
    let r = RadioRef::new(m, None);
    let ran = Rc::new(RefCell::new(Vec::<&str>::new()));
    let (a, b) = (ran.clone(), ran.clone());
    r.at(5.0, Box::new(move |_| a.borrow_mut().push("radio")), "radio");
    r.at(5.0, Box::new(move |_| b.borrow_mut().push("central")), "central");
    w(m, 0xC2A, 1, 0x10);
    w(m, 0xC2C, 2, 0x0100);
    w(m, 0xF00, 1, 0x82);
    let wp = rd(m, 0xC2B, 1) as usize;
    let before = {
        let rr = r.b();
        [wp, rr.brx.is_some() as usize, rr.sched.len()]
    };
    m.reset(0, false).unwrap();
    let wp = rd(m, 0xC2B, 1) as usize;
    let after = {
        let rr = r.b();
        [wp, rr.brx.is_some() as usize, rr.sched.len()]
    };
    w(m, 0x74A, 1, 0xC1 | 0x02);
    run_until(m, 6.0);
    assert!(before == [1, 1, 2] && after == [0, 0, 1] && *ran.borrow() == ["central"], "reset: {:?} -> {:?}, {:?}", before, after, ran);
}

#[test]
fn fifo_clear() {
    if std::env::var_os("TC32EMU_TXFIFO_CLEAR").is_none() {
        assert_eq!(settings::txfifo_clear(), "position");
    }
    for (mode, want) in [("position", [3u32, 3]), ("zero", [0, 0])] {
        settings::set_txfifo_clear(mode);
        let mut m = machine();
        let m = &mut m;
        RadioRef::new(m, None);
        w(m, 0xC2A, 1, 0x10);
        for _ in 0..3 {
            w(m, 0xC2C, 2, 0x0100);
        }
        w(m, 0xC2A, 1, 0x20);
        w(m, 0xC2A, 1, 0x20);
        w(m, 0xC2A, 1, 0x10);
        assert_eq!([rd(m, 0xC2A, 1), rd(m, 0xC2B, 1)], want, "TX FIFO clear, {}", mode);
    }
}

#[test]
fn rx_busy() {
    if std::env::var_os("TC32EMU_RX_BUSY").is_none() {
        assert_eq!(settings::rx_busy(), "off");
    }
    let pdu = [0x01u8, 0];
    let aa = [0x71u8, 0x76, 0x41, 0x29];
    for (mode, want) in [("off", [0u32, 0]), ("air", [1, 0]), ("stuck", [1, 1])] {
        settings::set_rx_busy(mode);
        let mut m = machine();
        let m = &mut m;
        let r = RadioRef::new(m, None);
        r.b().central = Some(Rc::new(Recorder::default()));
        w(m, 0x74A, 1, 0xC1 | 0x02);
        for (i, &v) in aa.iter().enumerate() {
            w(m, 0x408 + i, 1, v as u32);
        }
        w(m, 0x40D, 1, 7);
        w(m, 0xF16, 1, 0);
        w(m, 0xF00, 1, 0x82);
        let t = m.ms() + 0.05;
        run_until(m, t);
        let ok = r.central_tx(m, &pdu, 7, &aa);
        let t0 = m.ms();
        run_until(m, t0 + 0.04);
        let mid = (rd(m, 0x448, 1) >> 5) & 1;
        run_until(m, t0 + 0.1);
        let after = (rd(m, 0x448, 1) >> 5) & 1;
        assert!(ok && [mid, after] == want, "0x448 bit 5, {}: {} {}", mode, mid, after);
    }
}

#[test]
fn central_skip() {
    if std::env::var_os("TC32EMU_CENTRAL_SKIP_EVERY").is_none() {
        assert_eq!(settings::central_skip_every(), 0);
    }
    for (n, want) in [(0i64, vec![]), (7, vec![21i64, 28, 35])] {
        settings::set_central_skip_every(n);
        let mut m = machine();
        let m = &mut m;
        let r = RadioRef::new(m, None);
        let ce = Central::new(m, &r, quiet());
        ce.borrow_mut().state = "connected";
        for k in 14..40 {
            ce.borrow_mut().conn_event(m, k);
        }
        let sent: Vec<i64> = ce.borrow().sent.iter().map(|s| s[0] as i64).collect();
        let skipped: Vec<i64> = (14..40).filter(|k| !sent.contains(k)).collect();
        assert_eq!(skipped, want, "central skip {}", n);
    }
    settings::set_central_skip_every(0);
}

#[test]
fn terminate_ack() {
    for (name, no_ack, silent, want) in [("default", false, false, 1usize), ("NoTerminateAck", true, false, 0), ("gone silent", false, true, 0)] {
        let mut m = machine();
        let m = &mut m;
        let r = RadioRef::new(m, None);
        let mut o = quiet();
        o.no_terminate_ack = no_ack;
        let c = Central::new(m, &r, o);
        let mut c = c.borrow_mut();
        c.state = "connected";
        c.base_t = 0.0;
        c.base_k = 0;
        c.interval_ms = 30.0;
        c.conn_event(m, 1);
        c.nesn ^= 1;
        c.ll_ctrl(m, &[0x02, 0x13]);
        if silent {
            c.go_silent(2);
        }
        let after_ind = c.state;
        for k in 2..6 {
            c.conn_event(m, k);
        }
        let after: Vec<i64> = c.sent.iter().filter(|s| s[0] >= 2.0).map(|s| s[0] as i64).collect();
        let acked = c.transcript.iter().any(|l| l.contains("(acknowledges LL_TERMINATE_IND)") && l.contains("NESN1 SN0"));
        let want_ind = if no_ack { "terminated" } else { "terminate-ack" };
        assert!(
            after.len() == want && (want != 1 || (after[0] == 2 && acked)) && c.state == "terminated" && after_ind == want_ind,
            "LL_TERMINATE_IND, {}: {} then {}; sent at {:?} (ack {})",
            name,
            after_ind,
            c.state,
            after,
            acked
        );
    }
}

/// One ADV_IND by STX2RX on channel 37 (a random AdvA), and an answer's time.
fn advertise(m: &mut Machine) {
    let buf = 0x840300u32;
    wbytes(m, buf, &[8, 0, 0, 0, 0x40, 6, 0xc1, 0xc2, 0xc3, 0xc4, 0xc5, 0xc6]);
    w(m, 0xC0C, 2, buf & 0xFFFF);
    for (i, &v) in ADV_AA.iter().enumerate() {
        w(m, 0x408 + i, 1, v as u32);
    }
    w(m, 0x40D, 1, 37);
    w(m, 0xF00, 1, 0x87);
    let t = m.ms() + 1.0;
    run_until(m, t);
}

#[test]
fn centrals() {
    let mut m = machine();
    let m = &mut m;
    let r = RadioRef::new(m, None);
    w(m, 0x74A, 1, 0xC1 | 0x02);
    let o = quiet();
    let mut oa = o.clone();
    oa.connect_after_ms = 1e9;
    oa.active_scan = true;
    let a = Central::new(m, &r, oa);
    let mut ob = o.clone();
    ob.connect_after_ms = 0.0;
    ob.join = true;
    ob.aa = 0x5A3C9617;
    let b = Central::new(m, &r, ob);
    advertise(m);
    {
        let rr = r.b();
        assert!(is(&rr.central, &a) && rr.centrals.len() == 1 && a.borrow().state == "scan" && b.borrow().state == "connected");
        assert!(is(&rr.conn_central, &b) && rr.scan_reqs.is_empty());
    }
    assert!(is(&r.link_central(), &b));
    r.b().centrals.clear();
    assert!(is(&r.link_central(), &a), "the connecting central detached: the link does not go to Radio.Central");

    let mut m2 = machine();
    let m = &mut m2;
    let r = RadioRef::new(m, None);
    w(m, 0x74A, 1, 0xC1 | 0x02);
    let mut o = quiet();
    o.connect_after_ms = 1e9;
    let a = Central::new(m, &r, o.clone());
    let mut ob = o.clone();
    ob.active_scan = true;
    ob.join = true;
    ob.own_addr = "rpa".into();
    let b = Central::new(m, &r, ob);
    let mut oc = o.clone();
    oc.active_scan = true;
    oc.join = true;
    let c = Central::new(m, &r, oc);
    advertise(m);
    let rr = r.b();
    assert_eq!(rr.scan_reqs.len(), 1, "no central connects: SCAN_REQs");
    let sr = &rr.scan_reqs[0].pdu;
    assert!(same_peer(&rr.scan_reqs[0].central, &peer_of(&b)));
    assert!(sr[0] == 0xC3 && sr[1] == 12 && sr[2..8] == b.borrow().init_a[..] && hex(&sr[8..14]) == "c1c2c3c4c5c6");
    assert!(a.borrow().state == "scan" && b.borrow().state == "scan" && c.borrow().state == "scan" && rr.conn_central.is_none());
}

#[test]
fn privacy() {
    let mut m = machine();
    let m = &mut m;
    let r = RadioRef::new(m, None);
    let o = quiet();
    let public = Central::new(m, &r, o.clone());
    assert!(public.borrow().init_a == public.borrow().identity && public.borrow().init_a_type == 0);
    let mut gs = Vec::new();
    for own in ["rpa", "rpa", "rpa-wrong-irk"] {
        let mut oo = o.clone();
        oo.own_addr = own.into();
        oo.connect_after_ms = 0.0;
        let c = Central::new(m, &r, oo);
        let a = c.borrow().init_a.clone();
        let prand = a[3] as u32 | (a[4] as u32) << 8 | (a[5] as u32) << 16;
        let h = a[0] as u32 | (a[1] as u32) << 8 | (a[2] as u32) << 16;
        let resolves = aes128::ah(aes128::from_le(&c.borrow().irk), prand) == h;
        gs.push((hex(&a), prand >> 22, resolves));
        let ci = c.on_adv(m, &[0x40, 6, 0xc1, 0xc2, 0xc3, 0xc4, 0xc5, 0xc6], 37).unwrap();
        assert!(ci[0] == 0xC5 && ci[2..8] == a[..], "own_addr {}: CONNECT_IND {}", own, hex(&ci));
    }
    assert!(gs[0].1 == 1 && gs[1].1 == 1 && gs[2].1 == 1 && gs[0].0 != gs[1].0 && gs[0].2 && gs[1].2 && !gs[2].2, "RPAs: {:?}", gs);
    for (dist, pres_init, want) in [(false, 0u8, false), (true, 0x02, true), (true, 0, false)] {
        let mut oo = o.clone();
        oo.dist_id = dist;
        let c = Central::new(m, &r, oo);
        let mut c = c.borrow_mut();
        c.start_pairing(m);
        c.txq.clear();
        c.smp.pres = vec![0x02, 0x03, 0x00, 0x01, 0x10, pres_init, 0x03];
        c.smp_rx(m, &[0x09, 0x00, 1, 2, 3, 4, 5, 6]);
        let sent: Vec<String> = c.txq.iter().map(|q| hex(&q.payload[4..])).collect();
        let want_preq = if dist { 0x02 } else { 0 };
        let mut ok = c.smp.preq[5] == want_preq;
        if want {
            ok = ok && sent.len() == 2 && sent[0] == "08".to_string() + &hex(&c.irk) && sent[1] == "0900".to_string() + &hex(&c.identity);
        } else {
            ok = ok && sent.is_empty();
        }
        assert!(ok, "dist_id {}, response InitKeyDist {:#x}: preq {}, sent {:?}", dist, pres_init, hex(&c.smp.preq), sent);
    }
}

#[test]
fn adv_fifo() {
    let mut m = machine();
    let m = &mut m;
    let r = RadioRef::new(m, None);
    w(m, 0x74A, 1, 0xC1 | 0x02);
    let adv = 0x840300u32;
    wbytes(m, adv, &[8, 0, 0, 0, 0x00, 6, 0xC0, 1, 2, 3, 4, 0xC5]);
    w(m, 0xC0C, 2, adv & 0xFFFF);
    w(m, 0x40D, 1, 37);
    for (name, fifo, fmt) in [
        ("FIFO emptied", 0i64, 1u32),
        ("one packet queued", 1, 1),
        ("two packets queued", 2, 1),
        ("one packet queued, TPLL format", 1, 2),
        ("emptied again", 0, 1),
    ] {
        w(m, 0x404, 1, fmt);
        w(m, 0xC2A, 1, 0x10);
        for _ in 0..fifo {
            w(m, 0xC2C, 2, 0x0100);
        }
        let (n_tx, n_rec, n_miss) = {
            let rr = r.b();
            (rr.adv_tx.len(), rr.adv_fifo.len(), rr.misses.len())
        };
        w(m, 0xF00, 1, 0x85);
        let t = m.ms() + 0.1;
        run_until(m, t);
        let rr = r.b();
        let (sent, rec, miss) = (rr.adv_tx.len() - n_tx, &rr.adv_fifo[n_rec..], &rr.misses[n_miss..]);
        let want = if fifo > 0 && fmt == 1 { 1 } else { 0 };
        let mut ok = sent == 1 && rec.len() == want && miss.len() == want;
        if ok && want == 1 {
            ok = rec[0].ch == 37
                && (rec[0].wptr - rec[0].rptr) & 0xF == fifo
                && miss[0].why == format!("advertising with {} packet(s) in the TX FIFO (0xc2a {}, 0xc2b {})", fifo, rec[0].rptr, rec[0].wptr);
        }
        assert!(ok, "advertising, {}: sent {}, recorded {:?}, missed {:?}", name, sent, rec, miss);
    }
}

#[test]
fn central_channels() {
    let mut m = machine();
    let m = &mut m;
    let r = RadioRef::new(m, None);
    for (chm, want) in [
        (vec![0xffu8, 0xff, 0xff, 0xff, 0x1f], vec![7i64, 14, 21, 28, 35, 5, 12, 19]),
        (vec![0x55, 0, 0, 0, 0x10], vec![4, 36, 2, 6, 0, 0, 4, 36]),
    ] {
        let mut o = quiet();
        o.chm = chm;
        let c = Central::new(m, &r, o);
        for (i, &wch) in want.iter().enumerate() {
            assert_eq!(c.borrow_mut().channel(), wch, "channel {}", i);
        }
    }
    let mut rxadd = Vec::new();
    for txadd in 0..2u8 {
        let mut o = quiet();
        o.connect_after_ms = 0.0;
        let c = Central::new(m, &r, o);
        let adv = [txadd << 6, 6, 0xc1, 0xc2, 0xc3, 0xc4, 0xc5, 0xc6];
        rxadd.push(c.on_adv(m, &adv, 37).unwrap()[0] >> 7);
    }
    assert_eq!(rxadd, [0, 1], "CONNECT_IND RxAdd");
}

#[test]
fn param_request() {
    let mut m = machine();
    let m = &mut m;
    let r = RadioRef::new(m, None);
    let req = [0x12u8, 0x07, 8, 0, 6, 0, 6, 0, 44, 0, 0x2c, 0x01];
    for (name, apply, refuse, result_want, action) in [
        ("default", false, false, 0i64, "accepted"),
        ("apply_update", true, false, 0, "applied"),
        ("refuse_update", false, true, 1, "refused"),
        ("both", true, true, 1, "refused"),
    ] {
        let mut o = quiet();
        o.apply_update = apply;
        o.refuse_update = refuse;
        let got: Rc<RefCell<Vec<(ParamRequest, String)>>> = Rc::new(RefCell::new(Vec::new()));
        let g = got.clone();
        o.on_param_request = Some(Rc::new(move |q: ParamRequest, a: &str| g.borrow_mut().push((q, a.to_string()))));
        let c = Central::new(m, &r, o);
        let mut c = c.borrow_mut();
        c.event = 40;
        c.l2cap(m, 5, &req);
        let mut result = -1i64;
        for q in &c.txq {
            let p = &q.payload;
            if p.len() >= 10 && p[2] == 5 && p[3] == 0 && p[4] == 0x13 {
                result = p[8] as i64 | (p[9] as i64) << 8;
            }
        }
        let got = got.borrow();
        assert_eq!(got.len(), 1, "{}: callbacks", name);
        let (q, a) = &got[0];
        let want_instant = if action == "applied" { 52 } else { -1 };
        assert!(
            result == result_want
                && a == action
                && q.interval_min == 6
                && q.interval_max == 6
                && q.latency == 44
                && q.timeout == 300
                && q.event == 40
                && q.instant == want_instant
                && c.update.is_some() == (action == "applied"),
            "{}: result {}, action {}, req {:?}",
            name,
            result,
            a,
            q
        );
    }
}

#[test]
fn suspend_rf() {
    let regs = rf_suspend_regs();
    let no401: Vec<usize> = regs.iter().copied().filter(|&a| a != 0x401).collect();
    for (i, (mode, setup)) in [("lost", regs.clone()), ("lost", no401), ("keep", regs.clone())].into_iter().enumerate() {
        settings::set_suspend_rf(mode);
        let mut m = machine();
        let m = &mut m;
        let r = RadioRef::new(m, None);
        let lost = mode == "lost";
        let power_on = m.regs[0x1276];
        for &a in &setup {
            w(m, a, 1, 0x5A);
        }
        m.analog[0x26] = 0;
        w(m, 0x6F, 1, 0x81);
        m.asleep = None;
        let want = if lost { power_on } else { 0x5A };
        assert_eq!(m.regs[0x1276], want, "case {}: 0x1276 after the suspend", i);
        let want_uninit = if lost { setup.len() } else { 0 };
        assert_eq!(r.b().rf_uninit.len(), want_uninit, "case {}: registers to set again", i);
        w(m, 0xF00, 1, 0x82);
        assert_eq!(r.b().brx.is_none(), lost, "case {}: a BRX before the setup", i);
        w(m, 0xF00, 1, 0x80);
        for &a in &setup[..setup.len() - 1] {
            w(m, a, 1, 0x5A);
        }
        w(m, 0xF00, 1, 0x82);
        assert_eq!(r.b().brx.is_none(), lost, "case {}: one register still missing", i);
        w(m, 0xF00, 1, 0x80);
        w(m, *setup.last().unwrap(), 1, 0x5A);
        w(m, 0xF00, 1, 0x82);
        assert!(r.b().brx.is_some(), "case {}: its setup written again: the BRX did not start", i);
    }
    settings::set_suspend_rf("lost");
    let mut m = machine();
    let m = &mut m;
    let r = RadioRef::new(m, None);
    for &a in &regs {
        w(m, a, 1, 0x5A);
    }
    m.analog[0x26] = 0;
    w(m, 0x6F, 1, 0x81);
    m.asleep = None;
    m.reset(-1, false).unwrap();
    w(m, 0xF00, 1, 0x82);
    let rr = r.b();
    assert!(rr.brx.is_some() && rr.rf_uninit.is_empty() && rr.rf_written.is_empty(), "a reset after the suspend did not end the refusal");
    drop(rr);
    settings::set_suspend_rf("keep");
}

// ------------------------------------------------------------ tpll_test.go

#[derive(Default)]
struct TpllRec {
    heard: RefCell<Vec<TpllTx>>,
    reply: Option<Vec<u8>>,
}

impl TpllPeer for TpllRec {
    fn on_tpll(&self, _m: &mut Machine, payload: &[u8], mhz: i64, ac: &[u8]) -> Option<Vec<u8>> {
        self.heard.borrow_mut().push(TpllTx { ms: 0.0, mhz, ac: ac.to_vec(), payload: payload.to_vec() });
        self.reply.clone()
    }
}

#[test]
fn tpll() {
    let mut m = machine();
    let m = &mut m;
    let r = RadioRef::new(m, None);
    w(m, 0x74A, 1, 0xC1 | 0x02);
    m.regs[0x404] = 0x02;
    m.regs[0x405] = 3;
    m.regs[0x408..0x40B].copy_from_slice(&[0xaa, 0xbb, 0xcc]);
    m.regs[0x402] = 0x01;
    m.regs[0x1220] = 0x04;
    m.regs[0x1244] = ((2450 & 0x3F) << 2) as u8;
    m.regs[0x1245] = (2450 >> 6) as u8;
    let (txbuf, rxbuf) = (0x840300u32, 0x840400u32);
    wbytes(m, txbuf, &[9, 0, 0, 0, 0x04, b'k', b'b', b'd', b'!']);
    w(m, 0xC0C, 2, txbuf & 0xFFFF);
    w(m, 0xC08, 2, rxbuf & 0xFFFF);
    let peer = Rc::new(TpllRec { reply: Some(vec![0x11, 0x22]), ..Default::default() });
    r.b().tpll = Some(peer.clone());
    assert!(
        crate::radio::tpll_mhz(m) == 2450 && crate::radio::tpll_access_code(m) == [0xaa, 0xbb, 0xcc] && RadioRef::tpll_airtime_us(m, 4) == 44.0,
        "registers"
    );
    let t0 = m.ms();
    w(m, 0xF00, 1, 0x85);
    run_until(m, t0 + 0.5);
    {
        let h = peer.heard.borrow();
        let rr = r.b();
        assert!(
            h.len() == 1
                && h[0].payload == b"kbd!"
                && h[0].mhz == 2450
                && m.regs[0xF20] & 0x02 != 0
                && rr.tpll_tx.len() == 1
                && !rr.misses.is_empty()
                && rr.misses.last().unwrap().why == "not listening",
            "STX"
        );
    }
    w(m, 0xF20, 1, 0xFF);
    w(m, 0xF00, 1, 0x86);
    w(m, 0xF00, 1, 0x85);
    let t1 = m.ms();
    run_until(m, t1 + 0.5);
    let ent = read_bytes(m, rxbuf, 17);
    assert!(
        ent[0] == 13 && ent[4] == 2 && ent[5] == 0x11 && ent[6] == 0x22 && ent[13..17] == [0, 0, 0xC8, 0] && m.regs[0xF20] & 0x01 != 0 && r.b().srx.is_none(),
        "SRX then STX: entry {}",
        hex(&ent)
    );
    w(m, 0xF20, 1, 0xFF);
    m.regs[0xF03] = 0x02;
    w(m, 0xF28, 4, 100);
    w(m, 0xF00, 1, 0x86);
    let t2 = m.ms();
    run_until(m, t2 + 0.05);
    let open_at_50 = r.b().srx.is_some();
    run_until(m, t2 + 0.2);
    assert!(
        open_at_50 && r.b().srx.is_none() && (m.regs[0xF20] as u32 | (m.regs[0xF21] as u32) << 8) & 0x400 != 0,
        "SRX first timeout"
    );
    r.b().tpll = None;
    w(m, 0xF00, 1, 0x80);
    assert!(r.b().brx.is_none() && r.b().srx.is_none(), "0x80 must close the windows");
}

// ------------------------------------------------------------ air_test.go

#[derive(Default)]
struct AirRec {
    replies: RefCell<Vec<Vec<u8>>>,
    lost: RefCell<Vec<Vec<u8>>>,
    connects: RefCell<i64>,
}

impl Peer for AirRec {
    fn on_adv(&self, _m: &mut Machine, _pdu: &[u8], _ch: u8) -> Option<Vec<u8>> {
        None
    }
    fn on_connect_sent(&self, _m: &mut Machine, _pdu: &[u8], _t: f64) {
        *self.connects.borrow_mut() += 1;
    }
    fn drop_pdu(&self, _m: &mut Machine, _pdu: &[u8]) -> bool {
        false
    }
    fn on_air(&self, _m: &mut Machine, pdu: &[u8], _ch: u8, _tx: u32) -> Option<Vec<u8>> {
        self.replies.borrow_mut().push(pdu.to_vec());
        None
    }
    fn air_lost(&self, _m: &mut Machine, pdu: &[u8]) {
        self.lost.borrow_mut().push(pdu.to_vec());
    }
}

fn must_air(seed: u32, lk: i64, lh: i64, bad: &[u8], bad_loss: i64, ppm: f64, jitter: i64) -> Air {
    Air::new(seed, lk, lh, bad, bad_loss, ppm, jitter).unwrap()
}

fn lost_of(a: &mut Air, way: usize, ch: u8, now: f64, n: usize) -> Vec<usize> {
    (0..n).filter(|_| a.lost(way, ch, now)).collect()
}

#[test]
fn air() {
    let mut a = must_air(1, 300_000, 50_000, &[], 0, 0.0, 0);
    let k = lost_of(&mut a, TO_KEYBOARD, 5, 0.0, 40);
    let h = lost_of(&mut a, TO_HOST, 5, 0.0, 40);
    let want_k = vec![0usize, 2, 6, 7, 8, 9, 16, 17, 21, 23, 25, 26, 27, 30, 34, 36, 37, 38];
    let want_h = vec![12usize, 32];
    assert!(k == want_k && h == want_h && a.sent == [40, 40] && a.lost_n == [want_k.len() as i64, want_h.len() as i64], "seed 1");

    let mut a = must_air(1, 0, 0, &[], 0, 0.0, 0);
    assert!(lost_of(&mut a, TO_KEYBOARD, 5, 0.0, 100).is_empty() && lost_of(&mut a, TO_HOST, 5, 0.0, 100).is_empty());
    assert_eq!(a.rng[TO_KEYBOARD].randrange(MILLION as u32), PyRandom::new(4).randrange(MILLION as u32), "probability 0: a number drawn");

    let bad: Vec<u8> = (11..=20).collect();
    let mut a = must_air(2, 0, 0, &bad, MILLION, 0.0, 0);
    let lost: Vec<u8> = (0..40u8).filter(|&ch| a.lost(TO_HOST, ch, 0.0)).collect();
    assert_eq!(lost, bad, "bad channels 11-20");

    let mut a = must_air(1, 300_000, 0, &[], 0, 0.0, 0);
    a.blackout_until = 10.0;
    for way in [TO_KEYBOARD, TO_HOST] {
        for now in [0.0, 9.999] {
            assert!(a.lost(way, 5, now), "blackout: a packet to the {} at {} ms arrived", WAY_NAMES[way], now);
        }
    }
    let after = lost_of(&mut a, TO_KEYBOARD, 5, 10.0, 40);
    assert!(after == want_k && !a.lost(TO_HOST, 5, 10.0), "after the blackout: {:?}", after);

    let mut a = must_air(1, 0, 0, &[], 0, 0.0, 20);
    let j: Vec<i64> = (0..12).map(|_| (a.jitter_ms() * 1000.0).round() as i64).collect();
    assert_eq!(j, [16, -15, 11, -4, -18, -20, -11, 17, 10, 3, 0, -19], "jitter 20 us, seed 1");
    assert_eq!(must_air(1, 0, 0, &[], 0, 0.0, 0).jitter_ms(), 0.0);

    assert!(must_air(1, 0, 0, &[], 0, 250.0, 0).clock() == 1.00025 && must_air(1, 0, 0, &[], 0, -40.0, 0).clock() == 0.99996);
    assert_eq!(must_air(1, 0, 0, &[], 0, 0.0, 0).clock(), 1.0);
    for (p, want) in [(0.05, 50_000), (1.0, MILLION), (0.000001, 1)] {
        assert_eq!(millionths(p).unwrap(), want, "Millionths({})", p);
    }
    assert!(Air::new(1, 0, MILLION + 1, &[], 0, 0.0, 0).is_err());
    assert!(Air::new(1, 0, 0, &[40], 0, 0.0, 0).is_err());
    assert!(Air::new(1, 0, 0, &[], 0, 0.0, -1).is_err());
}

/// A BRX on channel 7 with an empty packet to send.
fn air_brx(m: &mut Machine, r: &RadioRef, rec: PeerRc) -> ([u8; 4], u32) {
    let empty = 0x840100u32;
    let rxbuf = 0x840200u32;
    wbytes(m, empty, &[2, 0, 0, 0, 0x01, 0]);
    let aa = [0x71u8, 0x76, 0x41, 0x29];
    for (i, &v) in aa.iter().enumerate() {
        w(m, 0x408 + i, 1, v as u32);
    }
    w(m, 0x40D, 1, 7);
    w(m, 0xF03, 1, 0x10);
    w(m, 0xC08, 2, rxbuf & 0xFFFF);
    w(m, 0xC0C, 2, empty & 0xFFFF);
    for i in 0..20 {
        m.write(rxbuf + i, 1, 0xEE).unwrap();
    }
    let start = m.stimer_now().wrapping_add(1600);
    w(m, 0xF18, 4, start);
    let f16 = rd(m, 0xF16, 1);
    w(m, 0xF16, 1, f16 | 4);
    w(m, 0xF20, 2, 0xFFFF);
    w(m, 0xF00, 1, 0x82);
    r.b().central = Some(rec);
    (aa, rxbuf)
}

#[test]
fn air_radio() {
    let pdu = [0x01u8, 0];
    let mut m = machine();
    let m = &mut m;
    let r = RadioRef::new(m, None);
    w(m, 0x74A, 1, 0xC1 | 0x02);
    r.b().air = Some(must_air(1, MILLION, 0, &[], 0, 0.0, 0));
    let rec = Rc::new(AirRec::default());
    let (aa, rxbuf) = air_brx(m, &r, rec.clone());
    let early = r.central_tx(m, &pdu, 7, &aa);
    let (early_lost, drawn) = {
        let rr = r.b();
        (rr.tx_lost, rr.air.as_ref().unwrap().sent[TO_KEYBOARD])
    };
    let t = m.ms() + 0.2;
    run_until(m, t);
    let ok = r.central_tx(m, &pdu, 7, &aa);
    let t = m.ms() + 1.0;
    run_until(m, t);
    assert!(!early && !early_lost && drawn == 0 && r.b().misses.len() == 1, "a packet before the start tick");
    let entry = read_bytes(m, rxbuf, 6);
    let st = rd(m, 0xF20, 2);
    let rr = r.b();
    assert!(
        !ok && rr.tx_lost
            && rr.misses.len() == 1
            && rr.brx.is_some()
            && entry == [0xee; 6]
            && st == 0
            && rec.replies.borrow().is_empty()
            && rr.air.as_ref().unwrap().lost_n[TO_KEYBOARD] == 1,
        "a packet lost on air"
    );
    drop(rr);

    let mut m2 = machine();
    let m = &mut m2;
    let r = RadioRef::new(m, None);
    w(m, 0x74A, 1, 0xC1 | 0x02);
    r.b().air = Some(must_air(1, 0, MILLION, &[], 0, 0.0, 0));
    let rec = Rc::new(AirRec::default());
    let (aa, _) = air_brx(m, &r, rec.clone());
    let t = m.ms() + 0.2;
    run_until(m, t);
    let ok = r.central_tx(m, &pdu, 7, &aa);
    let t = m.ms() + 1.0;
    run_until(m, t);
    let st = rd(m, 0xF20, 2);
    assert!(
        ok && !r.b().tx_lost
            && rec.lost.borrow().len() == 1
            && hex(&rec.lost.borrow()[0]) == "0500"
            && rec.replies.borrow().is_empty()
            && r.b().brx.is_none()
            && st == RX | TX | DONE,
        "the reply lost on air: status {:#x}",
        st
    );
    let _ = rec.connects.borrow();
}

fn air_advertise(m: &mut Machine) {
    let buf = 0x840300u32;
    wbytes(m, buf, &[8, 0, 0, 0, 0x40, 6, 0xc1, 0xc2, 0xc3, 0xc4, 0xc5, 0xc6]);
    w(m, 0xC0C, 2, buf & 0xFFFF);
    for (i, &v) in ADV_AA.iter().enumerate() {
        w(m, 0x408 + i, 1, v as u32);
    }
    w(m, 0x40D, 1, 37);
    w(m, 0xF20, 2, 0xFFFF);
    w(m, 0xF00, 1, 0x87);
    let t = m.ms() + 1.0;
    run_until(m, t);
}

#[test]
fn air_advertising() {
    let mut m = machine();
    let m = &mut m;
    let r = RadioRef::new(m, None);
    w(m, 0x74A, 1, 0xC1 | 0x02);
    r.b().air = Some(must_air(1, 0, MILLION, &[], 0, 0.0, 0));
    let mut o = quiet();
    o.connect_after_ms = 0.0;
    let c = Central::new(m, &r, o.clone());
    air_advertise(m);
    let st = rd(m, 0xF20, 2);
    assert!(c.borrow().state == "scan" && r.b().adv_tx.len() == 1 && st == TX, "an advertising packet lost");

    let mut m2 = machine();
    let m = &mut m2;
    let r = RadioRef::new(m, None);
    w(m, 0x74A, 1, 0xC1 | 0x02);
    r.b().air = Some(must_air(1, MILLION, 0, &[], 0, 0.0, 0));
    o.establish = true;
    let c = Central::new(m, &r, o);
    air_advertise(m);
    let state = c.borrow().state;
    let t = m.ms() + 6.5 * 30.0 + 5.0;
    run_until(m, t);
    let st = rd(m, 0xF20, 2);
    let cc = c.borrow();
    assert!(
        state == "connected"
            && r.b().conn_central.is_none()
            && st == TX
            && cc.state == "failed"
            && cc.stats.events == 6
            && cc.records.contains_key("establish_failed_ms"),
        "a CONNECT_IND lost: {} then {} after {} events",
        state,
        cc.state,
        cc.stats.events
    );
}

/// Runs the radio's next scheduled step at its time and returns that time.
fn step(r: &RadioRef, m: &mut Machine) -> f64 {
    let s = r.b().sched.pop_front().unwrap();
    m.cycles = (s.ms * m.cpu_hz as f64 / 1000.0) as i64 + 1;
    (s.f)(m);
    s.ms
}

#[test]
fn air_central() {
    let mut plain = [0.0f64; 2];
    for ppm in [0.0, 250.0, -40.0] {
        let mut m = machine();
        let m = &mut m;
        let r = RadioRef::new(m, None);
        w(m, 0x74A, 1, 0xC1 | 0x02);
        if ppm != 0.0 {
            r.b().air = Some(must_air(1, 0, 0, &[], 0, ppm, 0));
        }
        let mut o = quiet();
        o.win_offset = 2;
        let c = Central::new(m, &r, o);
        c.borrow_mut().adv_a = vec![0; 6];
        c.borrow_mut().on_connect_sent(m, &[], 0.0);
        let f = 1.0 + ppm / 1e6;
        let cc = c.borrow();
        for k in 0..4 {
            let got = cc.base_t + k as f64 * cc.interval_ms;
            let want = (1.25 + 2.5) * f + 0.3 + k as f64 * 30.0 * f;
            assert!((got - want).abs() <= 1e-9, "clock {} ppm: event {} at {}, want {}", ppm, k, got, want);
        }
        assert_eq!(r.b().sched[0].ms, cc.anchor0);
        if ppm == 0.0 {
            plain = [cc.anchor0, cc.interval_ms];
        }
    }
    assert_eq!(plain, [0.0 + 1.25 + 1.25 * 2.0 + 0.3, 24.0 * 1.25]);

    // A connection update under a clock.
    let mut m = machine();
    let m = &mut m;
    let r = RadioRef::new(m, None);
    w(m, 0x74A, 1, 0xC1 | 0x02);
    r.b().air = Some(must_air(1, 0, 0, &[], 0, 250.0, 0));
    let mut o = quiet();
    o.send_version = false;
    o.send_features = false;
    let c = Central::new(m, &r, o);
    c.borrow_mut().adv_a = vec![0; 6];
    c.borrow_mut().on_connect_sent(m, &[], 0.0);
    c.borrow_mut().connection_update(1, 3, 6, 0, 300, 2);
    for _ in 0..3 {
        step(&r, m);
    }
    let f = 1.00025;
    {
        let cc = c.borrow();
        let want = cc.anchor0 + 2.0 * 30.0 * f + 1.25 * 3.0 * f + 0.3;
        assert!((cc.base_t - want).abs() <= 1e-9 && (cc.interval_ms - 7.5 * f).abs() <= 1e-12 && cc.base_k == 2, "update under +250 ppm");
    }

    // Jitter moves each event's start, not the times they are counted from.
    let mut m = machine();
    let m = &mut m;
    let r = RadioRef::new(m, None);
    w(m, 0x74A, 1, 0xC1 | 0x02);
    r.b().air = Some(must_air(1, 0, 0, &[], 0, 0.0, 20));
    let c = Central::new(m, &r, quiet());
    c.borrow_mut().adv_a = vec![0; 6];
    c.borrow_mut().on_connect_sent(m, &[], 0.0);
    for (k, us) in [16, -15, 11, -4, -18, -20, -11, 17].into_iter().enumerate() {
        let got = step(&r, m);
        let want = c.borrow().anchor0 + k as f64 * 30.0 + us as f64 / 1000.0;
        assert_eq!(got, want, "jitter: event {}", k);
    }

    for (name, establish, hear, state, events) in [
        ("establish, nothing heard", true, false, "failed", 6),
        ("establish, a reply at event 3", true, true, "connected", 10),
        ("the default, nothing heard", false, false, "connected", 10),
    ] {
        let mut m = machine();
        let m = &mut m;
        let r = RadioRef::new(m, None);
        w(m, 0x74A, 1, 0xC1 | 0x02);
        let mut o = quiet();
        o.establish = establish;
        let c = Central::new(m, &r, o);
        c.borrow_mut().adv_a = vec![0; 6];
        c.borrow_mut().on_connect_sent(m, &[], 0.0);
        let mut k = 0;
        while k < 10 && !r.b().sched.is_empty() {
            step(&r, m);
            if hear && k == 3 {
                c.borrow_mut().on_air(m, &[0x01, 0], 0, 0);
            }
            k += 1;
        }
        let cc = c.borrow();
        assert!(cc.state == state && cc.stats.events == events, "{}: state {} after {} events", name, cc.state, cc.stats.events);
    }
}

/// A central in a connection that began at 0, with nothing queued; the machine is returned with it.
fn air_connected(set: impl FnOnce(&mut Options)) -> (Machine, RadioRef, CentralRc) {
    let mut m = machine();
    let r = RadioRef::new(&mut m, None);
    w(&mut m, 0x74A, 1, 0xC1 | 0x02);
    let mut o = quiet();
    o.send_version = false;
    o.send_features = false;
    set(&mut o);
    let c = Central::new(&mut m, &r, o);
    c.borrow_mut().adv_a = vec![0; 6];
    c.borrow_mut().on_connect_sent(&mut m, &[], 0.0);
    (m, r, c)
}

fn take_name((llid, p): (u8, Vec<u8>)) -> String {
    if llid == 1 && p.is_empty() {
        "empty".into()
    } else if llid == 3 && p[0] == 0 {
        format!("{} instant {}", ll_name_of(p[0]).unwrap(), p[10] as i64 | (p[11] as i64) << 8)
    } else if llid == 3 {
        ll_name_of(p[0]).map(|s| s.to_string()).unwrap_or(format!("0x{:x}", p[0]))
    } else {
        "L2CAP".into()
    }
}

#[test]
fn air_rules() {
    for on in [true, false] {
        let (mut m, _r, c) = air_connected(|o| o.mic_ends = on);
        let mut c = c.borrow_mut();
        c.event = 3;
        c.enc.sk = vec![0; 16];
        c.enc.iv = vec![0; 8];
        c.enc.rx = true;
        let mut p = vec![0x02u8, 9];
        p.extend_from_slice(&[0; 9]);
        let nxt = c.on_air(&mut m, &p, 0, 0);
        let rec = c.records.contains_key("mic_failure_ms");
        let want = if on { "mic-failed" } else { "connected" };
        assert!(c.state == want && nxt.is_none() && rec == on && c.enc.mic_fail == 1, "MIC failure, MicEnds {}", on);
    }

    for on in [true, false] {
        let (want_order, want_more, want_instant): (Vec<&str>, Vec<bool>, i64) = if on {
            (vec!["LL_ENC_REQ", "empty", "LL_START_ENC_RSP", "empty", "L2CAP", "LL_CONNECTION_UPDATE_IND instant 21"], vec![false, true, true], 21)
        } else {
            (vec!["LL_ENC_REQ", "L2CAP", "LL_START_ENC_RSP", "LL_CONNECTION_UPDATE_IND instant 17", "empty", "empty"], vec![true, true, false], 17)
        };
        let (mut m, _r, c) = air_connected(|o| o.ll_rules = on);
        let mut c = c.borrow_mut();
        c.event = 5;
        let mut enc_req = vec![0x03u8];
        enc_req.extend_from_slice(&[0; 22]);
        c.queue_ll(enc_req);
        c.queue_l2cap(5, &[0x13, 1, 2, 0, 0, 0]);
        c.connection_update(1, 0, 6, 0, 300, 5 + 12);
        let mut order = Vec::new();
        let mut more = Vec::new();
        order.push(take_name(c.take()));
        more.push(c.more());
        order.push(take_name(c.take()));
        c.enc.sk = vec![0; 16];
        c.enc.iv = vec![0; 8];
        c.ll_ctrl(&mut m, &[0x05]);
        more.push(c.more());
        order.push(take_name(c.take()));
        order.push(take_name(c.take()));
        c.ll_ctrl(&mut m, &[0x06]);
        c.event = 9;
        more.push(c.more());
        order.push(take_name(c.take()));
        order.push(take_name(c.take()));
        assert!(order == want_order && more == want_more && c.update.unwrap().instant == want_instant, "LLRules {}: {:?} {:?}", on, order, more);
    }

    // LLRules: an update first sent after event 2 has timed event 3, with its instant 3.
    let (mut m, r, lc) = air_connected(|o| o.ll_rules = true);
    for _ in 0..3 {
        step(&r, &mut m);
    }
    let ev = lc.borrow().event;
    lc.borrow_mut().connection_update(1, 0, 6, 0, 300, ev + 1);
    let lsent = take_name(lc.borrow_mut().take());
    let mut lstarts = Vec::new();
    for _ in 0..3 {
        let t = step(&r, &mut m);
        lstarts.push(t - lc.borrow().anchor0);
    }
    for (i, want) in [90.0, 90.3, 97.8].into_iter().enumerate() {
        assert!((lstarts[i] - want).abs() <= 1e-9, "LLRules, a late update: events at {:?}", lstarts);
    }
    {
        let l = lc.borrow();
        assert!(
            lsent == "LL_CONNECTION_UPDATE_IND instant 3" && l.update.unwrap().applied == round3(l.anchor0 + 90.3) && l.p.interval == 6 && l.stats.events == 5,
            "LLRules, a late update: {}",
            lsent
        );
    }

    // UpdateLead.
    let req = [0x12u8, 0x07, 8, 0, 6, 0, 6, 0, 44, 0, 0x2C, 0x01];
    for (lead, on, want) in [(0i64, false, 52i64), (1, false, 41), (0, true, 55), (1, true, 44)] {
        let (mut m, _r, c) = air_connected(|o| {
            o.apply_update = true;
            o.ll_rules = on;
            o.update_lead = lead;
        });
        let mut c = c.borrow_mut();
        c.event = 40;
        c.l2cap(&mut m, 5, &req);
        let answer = take_name(c.take());
        c.event = 43;
        let update = take_name(c.take());
        assert!(
            answer == "L2CAP" && update == format!("LL_CONNECTION_UPDATE_IND instant {}", want) && c.update.unwrap().instant == want,
            "UpdateLead {}, LLRules {}: {}, then {}",
            lead,
            on,
            answer,
            update
        );
    }

    // Reclock: an air put on the radio during the connection.
    let (mut m, r, rc) = air_connected(|_| {});
    let mut starts = Vec::new();
    for k in 0..6 {
        if k == 3 {
            r.b().air = Some(must_air(1, 0, 0, &[], 0, 250.0, 0));
            rc.borrow_mut().reclock();
        }
        starts.push(step(&r, &mut m));
    }
    let a0 = rc.borrow().anchor0;
    for (k, &s) in starts.iter().enumerate() {
        let want = if k > 3 { a0 + 90.0 + (k - 3) as f64 * 30.0 * 1.00025 } else { a0 + k as f64 * 30.0 };
        assert!((s - want).abs() <= 1e-9 && (k != 3 || s == a0 + 90.0), "Reclock: event {} at {}, want {}", k, s, want);
    }

    let (mut m, _r, c) = air_connected(|o| o.ll_rules = true);
    {
        let mut c = c.borrow_mut();
        c.event = 5;
        let mut enc_req = vec![0x03u8];
        enc_req.extend_from_slice(&[0; 22]);
        c.queue_ll(enc_req);
        c.queue_l2cap(5, &[0x13, 1, 2, 0, 0, 0]);
        let first = take_name(c.take());
        let held = take_name(c.take());
        c.ll_ctrl(&mut m, &[0x0D, 0x06]);
        let after = take_name(c.take());
        assert!(first == "LL_ENC_REQ" && held == "empty" && after == "L2CAP", "LLRules, LL_REJECT_IND: {} {} {}", first, held, after);
    }

    // LLRules: an update held behind the encryption start is not applied at its instant.
    for on in [true, false] {
        let (mut m, r, c) = air_connected(|o| o.ll_rules = on);
        let mut enc_req = vec![0x03u8];
        enc_req.extend_from_slice(&[0; 22]);
        c.borrow_mut().queue_ll(enc_req);
        let ev = c.borrow().event;
        c.borrow_mut().connection_update(1, 0, 12, 0, 300, ev + 3);
        for _ in 0..6 {
            step(&r, &mut m);
        }
        assert!(c.borrow().update.unwrap().is_applied != on, "LLRules {}: an update held behind LL_ENC_REQ", on);
    }

    // Another host's data: QueueData with its tag, OnAck and RxData.
    let (mut m, _r, c) = air_connected(|_| {});
    let got = Rc::new(RefCell::new(Vec::<String>::new()));
    let acked = Rc::new(RefCell::new(Vec::<String>::new()));
    let mut c = c.borrow_mut();
    c.event = 3;
    let g = got.clone();
    c.rx_data = Some(Rc::new(move |llid: u8, p: &[u8]| g.borrow_mut().push(format!("{} {}", llid, hex(p)))));
    let a = acked.clone();
    c.on_ack = Some(Rc::new(move |tag| a.borrow_mut().push(tag.downcast_ref::<&str>().unwrap().to_string())));
    c.queue_data(2, &[0x03, 0x00, 0x04, 0x00, 0x0a, 0x03, 0x00], Some(Rc::new("first")));
    c.queue_data(1, &[0x55], Some(Rc::new("second")));
    c.queue_ll(vec![0x12]);
    let mut sent = Vec::new();
    let shape = |p: Vec<u8>| format!("{} {}", p[0] & 3, hex(&p[2..]));
    sent.push(shape(c.next_pdu()));
    sent.push(shape(c.on_air(&mut m, &[0x01, 0], 0, 0).unwrap()));
    sent.push(shape(c.on_air(&mut m, &[0x02 | 1 << 2 | 1 << 3, 5, 0x01, 0x00, 0x04, 0x00, 0x0b], 0, 0).unwrap()));
    sent.push(shape(c.on_air(&mut m, &[0x01, 1, 0x77], 0, 0).unwrap()));
    let last = c.on_air(&mut m, &[0x01 | 1 << 2 | 1 << 3, 0], 0, 0);
    assert_eq!(sent, ["2 030004000a0300", "2 030004000a0300", "1 55", "3 12"]);
    assert_eq!(*acked.borrow(), ["first", "second"]);
    assert_eq!(*got.borrow(), ["2 010004000b", "1 77"]);
    assert!(c.l2.is_none() && last.is_none());
}

#[test]
fn fmt_pdu_of_an_empty_pdu() {
    assert_eq!(fmt_pdu(&[0x01, 0]), "[NESN0 SN0 MD0 len  0] empty");
}
