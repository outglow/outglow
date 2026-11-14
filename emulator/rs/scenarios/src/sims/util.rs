//! Small helpers shared by the scenario ports (texts in Python's forms,
//! the scripts' recurring `command()` and image paths).

use super::Sim;
use crate::image::Image;
use crate::session::R;

/// `x and hex(x)` of an optional address: "0x491e" or "None".
pub fn hexo(v: Option<u16>) -> String {
    v.map(|x| format!("{x:#x}")).unwrap_or_else(|| "None".into())
}

/// `[hex(x) for x in v]` as Python prints the list.
pub fn hex_list(v: &[u16]) -> String {
    format!("[{}]", v.iter().map(|x| format!("'{x:#x}'")).collect::<Vec<_>>().join(", "))
}

/// The image path relative to the repository root (`os.path.relpath(path, REPO)`).
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

/// `command()` of the scripts: a cluster command with a default response to
/// `short` on endpoint 1; ms until the Default Response, or None.
pub fn command(s: &mut Sim, short: u16, cluster: u16, cmd: u8, payload: &[u8], max_ms: f64) -> R<Option<f64>> {
    let n0 = s.w.coord.zcl_rsp.len();
    let t0 = s.now();
    s.w.coord.zcl_cmd(&mut s.w.air, short, 1, cluster, cmd, payload);
    let got = s.run_until(max_ms, 10.0, |s| {
        s.w.coord.zcl_rsp[n0..].iter().any(|r| r.src == short && r.cluster == cluster && r.cmd == Some(0x0B))
    })?;
    Ok(if got { Some(s.now() - t0) } else { None })
}

/// Number of verify-key events with ok=True.
pub fn verified(s: &Sim) -> usize {
    s.events("verify-key").iter().filter(|e| e.get("ok") == Some("True")).count()
}

/// `f"after {dt:.0f} ms" if dt is not None else otherwise`.
pub fn after(dt: Option<f64>, otherwise: &str) -> String {
    dt.map(|d| format!("after {d:.0} ms")).unwrap_or_else(|| otherwise.into())
}

/// How deep the stacks of a Rust image went so far, as a line (`SIM_STACK=1`
/// only; None otherwise or when the image has no ELF with the link script's
/// symbols). Emulated SRAM starts zeroed and a reset does not clear it, so
/// the lowest non-zero byte between the end of .bss and the top of the main
/// stack is the deepest the main stack has been since power-up (a zero
/// written there would be missed: a lower bound, good to a few bytes); the
/// same from the bottom of the interrupt stack up.
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

/// `stack_report` so far, and then the main stack below the part in use
/// cleared, so that the final report shows how deep the bulb went after
/// this point (the start of the firmware builds its stacks and goes deeper
/// than anything later). Only while the main stack is the one in use (not
/// inside an interrupt); `SIM_STACK=1` only.
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

/// `", ".join("lost" if x is None else f"{x:.0f}" for x in times) + " ms"`.
pub fn times(t: &[Option<f64>]) -> String {
    t.iter().map(|x| x.map(|d| format!("{d:.0}")).unwrap_or_else(|| "lost".into())).collect::<Vec<_>>().join(", ") + " ms"
}
