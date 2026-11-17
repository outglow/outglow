use super::Sim;
use crate::image::Image;
use crate::session::R;

pub fn hexo(v: Option<u16>) -> String {
    v.map(|x| format!("{x:#x}")).unwrap_or_else(|| "None".into())
}

pub fn hex_list(v: &[u16]) -> String {
    format!("[{}]", v.iter().map(|x| format!("'{x:#x}'")).collect::<Vec<_>>().join(", "))
}

pub fn firmware_revision_ok(img: &Image, got: &[u8]) -> bool {
    if !is_outglow(img) {
        return got == b"3.0.2_1610";
    }
    let Ok(s) = std::str::from_utf8(got) else { return false };
    let parts: Vec<&str> = s.split('.').collect();
    parts.len() == 3 && parts.iter().all(|p| !p.is_empty() && p.bytes().all(|c| c.is_ascii_digit()))
}

pub fn is_outglow(img: &Image) -> bool {
    const TAG: &[u8] = b"outglow-board:";
    img.bytes().map(|b| b.windows(TAG.len()).any(|w| w == TAG)).unwrap_or(false)
}

pub fn rel(i: &Image) -> String {
    let root = crate::runner::repo_root();
    if let Ok(p) = i.path.strip_prefix(&root) {
        return p.display().to_string();
    }
    let (rc, pc) = (root.canonicalize().unwrap_or(root.clone()), i.path.canonicalize().unwrap_or(i.path.clone()));
    pc.strip_prefix(&rc).map(|p| p.display().to_string()).unwrap_or_else(|_| i.path.display().to_string())
}

pub fn onoff(on: bool) -> &'static str {
    if on {
        "on"
    } else {
        "off"
    }
}

pub fn command(s: &mut Sim, short: u16, cluster: u16, cmd: u8, payload: &[u8], max_ms: f64) -> R<Option<f64>> {
    let n0 = s.w.coord.zcl_rsp.len();
    let t0 = s.now();
    s.w.coord.zcl_cmd(&mut s.w.air, short, 1, cluster, cmd, payload);
    let got = s.run_until(max_ms, 10.0, |s| {
        s.w.coord.zcl_rsp[n0..].iter().any(|r| r.src == short && r.cluster == cluster && r.cmd == Some(0x0B))
    })?;
    Ok(if got { Some(s.now() - t0) } else { None })
}

pub fn verified(s: &Sim) -> usize {
    s.events("verify-key").iter().filter(|e| e.get("ok") == Some("True")).count()
}

pub fn key_confirmed(s: &Sim, short: u16) -> bool {
    let seqs: Vec<u8> = s.w.coord.confirm_keys.iter().filter(|c| c.0 == short).map(|c| c.1).collect();
    if seqs.is_empty() {
        return false;
    }
    let frames = &s.w.air.frames;
    frames.iter().enumerate().any(|(i, f)| {
        let Some(m) = crate::frames::mac_parse(&f.psdu) else { return false };
        let p = &m.payload;
        let last_hop = m.ty == 1
            && m.dst == Some(short as u64)
            && p.len() >= 8
            && u16::from_le_bytes([p[2], p[3]]) == short
            && u16::from_le_bytes([p[4], p[5]]) == 0x0000
            && seqs.contains(&p[7]);
        last_hop
            && frames[i + 1..].iter().take_while(|a| a.t0 < f.t1 + 1.0).any(|a| {
                a.who != f.who && a.psdu.len() == 5 && a.psdu[0] & 7 == 2 && a.psdu[2] == m.seq
            })
    })
}

pub fn after(dt: Option<f64>, otherwise: &str) -> String {
    dt.map(|d| format!("after {d:.0} ms")).unwrap_or_else(|| otherwise.into())
}

pub fn stack_report(node: &zbnode::Node, img: &Image) -> Option<String> {
    std::env::var_os("SIM_STACK")?;
    let floor = img.symbol("_end_bss_")?;
    let top = img.symbol("_stack_end_")?;
    let irq_top = img.symbol("_irq_stack_top_")?;
    const SRAM: u32 = 0x840000;
    let sram = &node.m.sram;
    let at = |a: u32| sram.get((a - SRAM) as usize).copied().unwrap_or(0);
    let deepest = (floor..top).find(|a| at(*a) != 0).unwrap_or(top);
    let irq_deepest = (top..irq_top).find(|a| at(*a) != 0).unwrap_or(irq_top);
    Some(format!(
        "  stack: main {} B used of {} B (deepest {deepest:#x}, {} B never touched above the end of .bss {floor:#x}); \
         interrupt {} B of {} B",
        top - deepest,
        top - floor,
        deepest - floor,
        irq_top - irq_deepest,
        irq_top - top
    ))
}

pub fn stack_rearm(node: &mut zbnode::Node, img: &Image) -> Option<String> {
    let line = stack_report(node, img)?;
    let (floor, top) = (img.symbol("_end_bss_")?, img.symbol("_stack_end_")?);
    let sp = node.m.r[13];
    if !(floor + 64..top).contains(&sp) {
        return Some(format!("{line} (not cleared: inside an interrupt)"));
    }
    let (lo, hi) = ((floor - 0x840000) as usize, (sp - 64 - 0x840000) as usize);
    node.m.sram[lo..hi].fill(0);
    Some(format!("{} at {:.0} ms; cleared below the stack in use", line.replacen("stack:", "stack so far:", 1), node.m.ms()))
}

pub fn times(t: &[Option<f64>]) -> String {
    t.iter().map(|x| x.map(|d| format!("{d:.0}")).unwrap_or_else(|| "lost".into())).collect::<Vec<_>>().join(", ") + " ms"
}
