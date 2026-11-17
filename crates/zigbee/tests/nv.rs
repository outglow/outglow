use crate::sim::*;
use zigbee::hal::NvItem;
use zigbee::stack::State;

const GROUPS: u16 = 0x0004;
const SCENES: u16 = 0x0005;

fn stored(net: &Net, n: usize, item: NvItem) -> Vec<u8> {
    net.air.borrow().ports[net.nodes[n].id].nv[&(item as u8)].clone()
}

fn put(net: &mut Net, n: usize, item: NvItem, data: Vec<u8>) {
    let id = net.nodes[n].id;
    net.air.borrow_mut().ports[id].nv.insert(item as u8, data);
}

fn reboot(net: &mut Net, n: usize) {
    net.power_cycle(n);
    net.run(500);
}

#[test]
fn a_network_record_of_another_version_or_with_an_impossible_channel_or_address_is_no_network() {
    for change in [(0usize, 2u8), (1, 10), (1, 27), (4, 0xFF)] {
        let mut net = Net::new();
        let n = net.join_node();
        let mut network = stored(&net, n, NvItem::Network);
        if change.0 == 4 {
            network[4] = 0xF8;
            network[5] = 0xFF;
        } else {
            network[change.0] = change.1;
        }
        net.coord.permit_join = false;
        put(&mut net, n, NvItem::Network, network);
        reboot(&mut net, n);
        assert_ne!(net.nodes[n].stack.state(), State::Up, "{change:?}");
    }
    let mut net = Net::new();
    let n = net.join_node();
    let network = stored(&net, n, NvItem::Network);
    put(&mut net, n, NvItem::Network, network[..network.len() - 1].to_vec());
    net.coord.permit_join = false;
    reboot(&mut net, n);
    assert_ne!(net.nodes[n].stack.state(), State::Up, "cut short");
}

fn group_count(net: &mut Net, short: u16) -> u8 {
    let seq = net.coord.zcl(short, GROUPS, 0x01, 0x02, &[0]);
    net.run(50);
    let rsp = net.coord.last_zcl(short, GROUPS, 0x02).unwrap();
    assert_eq!(rsp[1], seq);
    rsp[4]
}

#[test]
fn tables_of_another_version_or_cut_short_are_left_alone() {
    let mut net = Net::new();
    let n = net.join_node();
    let short = net.nodes[n].short();
    net.coord.zcl(short, GROUPS, 0x01, 0x00, &[0x34, 0x12, 0]);
    net.run(11_000);
    let tables = stored(&net, n, NvItem::Tables);
    assert_eq!(tables[..3], [2, 0x34, 0x12]);
    let mut other = tables.clone();
    other[0] = 3;
    put(&mut net, n, NvItem::Tables, other);
    reboot(&mut net, n);
    assert_eq!(group_count(&mut net, short), 0);
    put(&mut net, n, NvItem::Tables, tables[..20].to_vec());
    reboot(&mut net, n);
    assert_eq!(group_count(&mut net, short), 0);
    put(&mut net, n, NvItem::Tables, tables);
    reboot(&mut net, n);
    assert_eq!(group_count(&mut net, short), 1);
}

fn scene_ids(net: &mut Net, short: u16) -> Vec<u8> {
    let seq = net.coord.zcl(short, SCENES, 0x01, 0x06, &[0x34, 0x12]);
    net.run(50);
    let rsp = net.coord.last_zcl(short, SCENES, 0x06).unwrap();
    assert_eq!(rsp[1], seq);
    rsp[8..].to_vec()
}

#[test]
fn scenes_of_the_older_format_are_read_too_and_a_record_cut_short_ends_them() {
    let mut net = Net::new();
    let n = net.join_node();
    let short = net.nodes[n].short();
    net.coord.zcl(short, GROUPS, 0x01, 0x00, &[0x34, 0x12, 0]);
    net.run(11_000);
    let mut old = vec![2];
    for id in [5u8, 6] {
        old.extend_from_slice(&[0x34, 0x12, id, 0x01, 100, 0xFA, 0x00, 2, 0, 0]);
    }
    put(&mut net, n, NvItem::Scenes, old.clone());
    reboot(&mut net, n);
    assert_eq!(scene_ids(&mut net, short), vec![5, 6]);
    old.truncate(old.len() - 1);
    put(&mut net, n, NvItem::Scenes, old);
    reboot(&mut net, n);
    assert_eq!(scene_ids(&mut net, short), vec![5]);
    let mut new = vec![3, 0x34, 0x12, 7, 0x01, 100, 0xFA, 0x00, 2, 0, 0, 2, 0, 0, 0, 0, 0];
    put(&mut net, n, NvItem::Scenes, new.clone());
    reboot(&mut net, n);
    assert_eq!(scene_ids(&mut net, short), Vec::<u8>::new(), "a colour cut short");
    new.extend_from_slice(&[0]);
    put(&mut net, n, NvItem::Scenes, new);
    reboot(&mut net, n);
    assert_eq!(scene_ids(&mut net, short), vec![7]);
    put(&mut net, n, NvItem::Scenes, vec![4, 0x34, 0x12, 9, 0x01, 100, 0xFA, 0x00, 2, 0, 0]);
    reboot(&mut net, n);
    assert_eq!(scene_ids(&mut net, short), Vec::<u8>::new(), "an unknown version");
}
