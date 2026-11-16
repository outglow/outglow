use super::zcl_foundation::{int, records, retry, Records};
use super::Test;
use crate::air::NodeId;
use crate::session::{expect, CmdAnswer, Session, Stop, R};
use crate::station::Tx;

const S: &str = "zcl-diag";
const DIAG: u16 = 0x0B05;
const ON_OFF: u16 = 0x0006;
const UNSUP_COMMAND: u8 = 0x81;

const TABLE: [(u16, &str, u8); 32] = [
    (0x0000, "NumberOfResets", 0x21),
    (0x0001, "PersistentMemoryWrites", 0x21),
    (0x0100, "MacRxBcast", 0x23),
    (0x0101, "MacTxBcast", 0x23),
    (0x0102, "MacRxUcast", 0x23),
    (0x0103, "MacTxUcast", 0x23),
    (0x0104, "MacTxUcastRetry", 0x21),
    (0x0105, "MacTxUcastFail", 0x21),
    (0x0106, "APSRxBcast", 0x21),
    (0x0107, "APSTxBcast", 0x21),
    (0x0108, "APSRxUcast", 0x21),
    (0x0109, "APSTxUcastSuccess", 0x21),
    (0x010A, "APSTxUcastRetry", 0x21),
    (0x010B, "APSTxUcastFail", 0x21),
    (0x010C, "RouteDiscInitiated", 0x21),
    (0x010D, "NeighborAdded", 0x21),
    (0x010E, "NeighborRemoved", 0x21),
    (0x010F, "NeighborStale", 0x21),
    (0x0110, "JoinIndication", 0x21),
    (0x0111, "ChildMoved", 0x21),
    (0x0112, "NWKFCFailure", 0x21),
    (0x0113, "APSFCFailure", 0x21),
    (0x0114, "APSUnauthorizedKey", 0x21),
    (0x0115, "NWKDecryptFailures", 0x21),
    (0x0116, "APSDecryptFailures", 0x21),
    (0x0117, "PacketBufferAllocateFailures", 0x21),
    (0x0118, "RelayedUcast", 0x21),
    (0x0119, "PhytoMACqueuelimitreached", 0x21),
    (0x011A, "PacketValidatedropcount", 0x21),
    (0x011B, "AverageMACRetryPerAPSMessageSent", 0x21),
    (0x011C, "LastMessageLQI", 0x20),
    (0x011D, "LastMessageRSSI", 0x28),
];

macro_rules! wrapped {
    ($($w:ident => $f:ident),* $(,)?) => {
        $(fn $w(s: &mut Session) -> R<String> { retry(s, $f) })*
    };
}

wrapped!(
    w_revision => revision, w_types => types, w_last_message => last_message, w_no_commands => no_commands,
    w_rx_counters => rx_counters,
);

pub fn tests() -> Vec<Test> {
    vec![
        Test { id: "ZCL-DIAG-01", doc: "ZCL8", clause: "3.15.1.1",
               req: "The ClusterRevision attribute of the Diagnostics server is the highest revision of the cluster \
                     in ZCL 8, 3.",
               session: S, run: w_revision, mandatory: false, ..Test::D },
        Test { id: "ZCL-DIAG-02", doc: "ZCL8", clause: "3.15.2.2 Tables 3-130, 3-131",
               req: "Every attribute the Diagnostics server has in the ranges of the two attribute sets is one of \
                     the table and has the table's data type, and it reads with that type.",
               session: S, run: w_types, mandatory: false, ..Test::D },
        Test { id: "ZCL-DIAG-03", doc: "ZCL8", clause: "3.15.2.2.2.29, 3.15.2.2.2.30",
               req: "LastMessageLQI and LastMessageRSSI read by a device are those of the Read Attributes command \
                     that reads them: the RSSI is the signal strength the command arrived with, and the link quality \
                     of a command that arrives 30 dB weaker is lower.",
               session: S, run: w_last_message, mandatory: false,
               ambiguous: Some("the text leaves the calculation of the LQI to the implementation (only 0 is the \
                                worst and 255 the best link); the test takes a lower LQI for a much weaker signal \
                                and the RSSI in dBm within 3 dB of the emulated signal"),
               ..Test::D },
        Test { id: "ZCL-DIAG-04", doc: "ZCL8", clause: "3.15.2.3; 2.5.12.2",
               req: "The Diagnostics server receives no commands: a cluster-specific command to it is answered with \
                     a Default Response with the status UNSUP_COMMAND.",
               session: S, run: w_no_commands, mandatory: false, ..Test::D },
        Test { id: "ZCL-DIAG-05", doc: "ZCL8", clause: "3.15.2.2.2.3, 3.15.2.2.2.9",
               req: "MacRxUcast and APSRxUcast are incremented each time the MAC and the APS layer receive a \
                     unicast: between two reads, by at least the number of unicasts sent to the device.",
               session: S, run: w_rx_counters, mandatory: false, ..Test::D },
        Test { id: "ZCL-DIAG-06", doc: "ZCL8", clause: "3.15.2.2.1.1",
               req: "NumberOfResets is incremented each time the device restarts: after a power cycle it reads one \
                     more than before.",
               session: "", run: resets, mandatory: false, ..Test::D },
    ]
}

fn discovered(s: &mut Session) -> R<Vec<(u16, u8)>> {
    let mut out = Vec::new();
    let mut start = 0u16;
    for _ in 0..8 {
        let st = start.to_le_bytes();
        let Some(f) = s.zcl(DIAG, 0x0C, &[st[0], st[1], 16], Tx::new().general())? else {
            return Err(Stop::Inconclusive("no Discover Attributes Response".into()));
        };
        let z = f.zcl().unwrap();
        if z.specific || z.cmd != 0x0D || z.payload.is_empty() {
            return Err(Stop::Fail(format!("Discover Attributes answered with command {:#04x}", z.cmd)));
        }
        let recs: Vec<(u16, u8)> = z.payload[1..].chunks(3).filter(|c| c.len() == 3).map(|c| (u16::from_le_bytes([c[0], c[1]]), c[2])).collect();
        let complete = z.payload[0] == 1;
        let last = recs.last().map(|r| r.0);
        out.extend(recs);
        match (complete, last) {
            (false, Some(l)) if l < 0xFFFF => start = l + 1,
            _ => break,
        }
    }
    Ok(out)
}

fn read(s: &mut Session, attrs: &[u16]) -> R<Records> {
    match s.read(DIAG, attrs, 1500.0)? {
        Some(r) => Ok(records(r)),
        None => Err(Stop::Inconclusive("no Read Attributes Response".into())),
    }
}

fn revision(s: &mut Session) -> R<String> {
    s.need_cluster(DIAG)?;
    let r = read(s, &[0xFFFD])?;
    let v = r.get(&0xFFFD).and_then(|x| int(&x.2));
    expect(v == Some(3), format!("ClusterRevision {v:?}"))?;
    Ok("ClusterRevision 3".into())
}

fn types(s: &mut Session) -> R<String> {
    s.need_cluster(DIAG)?;
    let found = discovered(s)?;
    let inside: Vec<(u16, u8)> = found.into_iter().filter(|a| a.0 <= 0x01FF).collect();
    if inside.is_empty() {
        return Err(Stop::NotApplicable("no attribute of the two attribute sets".into()));
    }
    let mut bad = Vec::new();
    for (id, ty) in &inside {
        match TABLE.iter().find(|t| t.0 == *id) {
            None => bad.push(format!("{id:#06x} is in no table")),
            Some(t) if t.2 != *ty => bad.push(format!("{} ({id:#06x}) type {ty:#04x}, the table says {:#04x}", t.1, t.2)),
            _ => {}
        }
    }
    for chunk in inside.chunks(8) {
        let ids: Vec<u16> = chunk.iter().map(|a| a.0).collect();
        let r = read(s, &ids)?;
        for (id, ty) in chunk {
            match r.get(id) {
                Some((0, Some(t), Some(_))) if t == ty => {}
                other => bad.push(format!("{id:#06x} reads as {other:?}")),
            }
        }
    }
    expect(bad.is_empty(), bad.join("; "))?;
    Ok(format!("{} attributes: {}", inside.len(), inside.iter().map(|a| format!("{:#06x}", a.0)).collect::<Vec<_>>().join(" ")))
}

fn read_at(s: &mut Session, rssi: i64) -> R<(Option<i64>, Option<i64>)> {
    s.w.air.rssi_fn = Some(Box::new(move |snd, rcv, _| if snd == NodeId::Coord && rcv == NodeId::Dut { rssi } else { -40 }));
    let r = read(s, &[0x011C, 0x011D]);
    s.w.air.rssi_fn = None;
    let r = r?;
    let lqi = r.get(&0x011C).and_then(|x| int(&x.2));
    let rs = r.get(&0x011D).and_then(|x| int(&x.2)).map(|v| if v > 127 { v - 256 } else { v });
    Ok((lqi, rs))
}

fn last_message(s: &mut Session) -> R<String> {
    s.need_cluster(DIAG)?;
    let have = discovered(s)?;
    let (has_lqi, has_rssi) = (have.iter().any(|a| a.0 == 0x011C), have.iter().any(|a| a.0 == 0x011D));
    if !has_lqi && !has_rssi {
        return Err(Stop::NotApplicable("neither LastMessageLQI nor LastMessageRSSI".into()));
    }
    let strong = read_at(s, -45)?;
    let weak = read_at(s, -75)?;
    let again = read_at(s, -45)?;
    let obs = format!("at -45 dBm {strong:?}, at -75 dBm {weak:?}, at -45 dBm again {again:?} (LQI, RSSI)");
    if has_lqi {
        let (a, b, c) = (strong.0, weak.0, again.0);
        expect(a.is_some() && b.is_some() && b < a && c == a, obs.clone())?;
    }
    if has_rssi {
        let ok = |v: Option<i64>, want: i64| v.is_some_and(|v| (v - want).abs() <= 3);
        expect(ok(strong.1, -45) && ok(weak.1, -75) && ok(again.1, -45), obs.clone())?;
    }
    Ok(obs)
}

fn no_commands(s: &mut Session) -> R<String> {
    s.need_cluster(DIAG)?;
    let mut seen = Vec::new();
    for cmd in [0x00u8, 0x01] {
        match s.cmd(DIAG, cmd, &[])? {
            CmdAnswer::Status(st) => seen.push((cmd, st)),
            CmdAnswer::None => return Err(Stop::Fail(format!("command {cmd:#04x}: no answer"))),
            CmdAnswer::Frame(_) => return Err(Stop::Fail(format!("command {cmd:#04x}: answered with a frame of the cluster"))),
        }
    }
    expect(seen.iter().all(|(_, st)| *st == UNSUP_COMMAND), format!("{seen:?}"))?;
    Ok(format!("Default Response status {:#04x} to commands 0x00 and 0x01", UNSUP_COMMAND))
}

fn rx_counters(s: &mut Session) -> R<String> {
    s.need_cluster(DIAG)?;
    let have = discovered(s)?;
    let attrs: Vec<u16> = [0x0102u16, 0x0108].into_iter().filter(|a| have.iter().any(|h| h.0 == *a)).collect();
    if attrs.is_empty() {
        return Err(Stop::NotApplicable("neither MacRxUcast nor APSRxUcast".into()));
    }
    let before = read(s, &attrs)?;
    const N: i64 = 5;
    for _ in 0..N {
        s.read(ON_OFF, &[0x0000], 1500.0)?;
    }
    let after = read(s, &attrs)?;
    let mut parts = Vec::new();
    let mut ok = true;
    for a in &attrs {
        let (b, c) = (before.get(a).and_then(|x| int(&x.2)), after.get(a).and_then(|x| int(&x.2)));
        let d = match (b, c) {
            (Some(b), Some(c)) => c - b,
            _ => return Err(Stop::Inconclusive(format!("{a:#06x} did not read"))),
        };
        ok &= d >= N + 1;
        parts.push(format!("{a:#06x}: +{d}"));
    }
    let obs = format!("{} after {N} unicast reads", parts.join(", "));
    expect(ok, obs.clone())?;
    Ok(obs)
}

fn resets(s: &mut Session) -> R<String> {
    s.need_cluster(DIAG)?;
    let have = discovered(s)?;
    if !have.iter().any(|a| a.0 == 0x0000) {
        return Err(Stop::NotApplicable("no NumberOfResets".into()));
    }
    let n0 = read(s, &[0x0000])?.get(&0x0000).and_then(|x| int(&x.2));
    s.reboot()?;
    s.run(6000.0)?;
    let n1 = read(s, &[0x0000])?.get(&0x0000).and_then(|x| int(&x.2));
    let obs = format!("NumberOfResets {n0:?} -> {n1:?} over a power cycle");
    expect(matches!((n0, n1), (Some(a), Some(b)) if b == (a + 1) & 0xFFFF), obs.clone())?;
    Ok(obs)
}
