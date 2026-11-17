use crate::sim::*;
use zigbee::nwk;

#[test]
fn an_unknown_nwk_command_is_answered_with_its_identifier_and_none_goes_where_there_is_no_route() {
    let mut net = Net::new();
    let n = net.join_node();
    let short = net.nodes[n].short();
    net.coord.nwk_cmd_rx.clear();
    net.coord.nwk_command(short, 1, &[0x2E, 0x00]);
    net.run(200);
    assert!(net.coord.nwk_cmd_rx.iter().any(|(s, p)| *s == short && p[..] == [nwk::CMD_NETWORK_STATUS, nwk::STATUS_UNKNOWN_COMMAND, 0x2E]), "{:x?}", net.coord.nwk_cmd_rx);
    let t = net.now();
    let mut h = net.coord.header(nwk::FRAME_CMD, short, 1);
    h.src = 0x5151;
    h.src_ext = None;
    net.coord.nwk_send(h, &[0x2F], None);
    net.run(200);
    let sent = net.nwk_sent_by(net.nodes[n].id, t);
    assert!(!sent.iter().any(|(_, h, p)| h.dst == 0x5151 && p.first() == Some(&nwk::CMD_NETWORK_STATUS)), "no route to tell");
}
