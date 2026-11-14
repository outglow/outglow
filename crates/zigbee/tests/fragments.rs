//! APS fragmentation: long payloads in both directions.

mod sim;

use sim::*;

const BASIC: u16 = 0x0000;

/// A Read Attributes request for `n` attributes of the Basic cluster, the
/// first few real, the rest unknown; and the response it must get.
fn long_read(n: u16) -> (Vec<u8>, Vec<u8>) {
    let mut request = vec![];
    let mut response = vec![];
    for i in 0..n {
        let id: u16 = match i {
            0 => 0x0004,
            1 => 0x0005,
            _ => 0x1000 + i,
        };
        request.extend_from_slice(&id.to_le_bytes());
        response.extend_from_slice(&id.to_le_bytes());
        match i {
            0 => {
                response.extend_from_slice(&[0x00, 0x42, 4]);
                response.extend_from_slice(b"AwoX");
            }
            1 => {
                response.extend_from_slice(&[0x00, 0x42, 15]);
                response.extend_from_slice(b"ESMLFzm_w6_Dimm");
            }
            _ => response.push(0x86),
        }
    }
    (request, response)
}

#[test]
fn a_long_request_arrives_in_fragments_and_the_long_answer_leaves_in_fragments() {
    let mut net = Net::new();
    let n = net.join_node();
    let short = net.nodes[n].short();
    // 60 attribute identifiers: 123 bytes of ZCL, three blocks of 50
    let (request, response) = long_read(60);
    let acks = net.coord.fragment_acks;
    let seq = net.coord.zcl_fragmented(short, BASIC, 0x00, 0x00, &request, 50);
    net.run(500);
    assert_eq!(net.coord.fragment_acks, acks + 3, "every block of the request was acknowledged");
    let rsp = net.coord.last_zcl(short, BASIC, 0x01).expect("the response, reassembled");
    assert_eq!(&rsp[..3], &[0x18, seq, 0x01]);
    assert_eq!(&rsp[3..], &response[..]);
    assert!(rsp.len() > 200, "it took several blocks: {} bytes", rsp.len());
}

#[test]
fn blocks_may_come_twice_and_the_payload_is_still_delivered_once() {
    let mut net = Net::new();
    let n = net.join_node();
    let short = net.nodes[n].short();
    // Add Scene with a long name: a long request whose answer is short
    let mut add = vec![0, 0, 1, 0, 0, 100];
    add.extend_from_slice(&[b'n'; 100]);
    add.extend_from_slice(&[0x08, 0x00, 1, 77]);
    let seq = net.coord.zcl_fragmented(short, 0x0005, 0x01, 0x00, &add, 50);
    net.run(100);
    // every block again, as a sender does whose acknowledgements were lost
    let acks = net.coord.fragment_acks;
    net.coord.repeat_fragments();
    net.run(500);
    assert_eq!(net.coord.fragment_acks, acks + 3, "the repeats are acknowledged again");
    let answers = net.coord.zcl_rx.iter().filter(|(s, c, f)| *s == short && *c == 0x0005 && f[1] == seq).count();
    assert_eq!(answers, 1, "and the request is carried out once");
}

#[test]
fn a_block_that_gets_no_acknowledgement_is_sent_again() {
    let mut net = Net::new();
    let n = net.join_node();
    let short = net.nodes[n].short();
    let (request, response) = long_read(60);
    // the coordinator misses the first block of the response once
    net.coord.drop_fragments = 1;
    let seq = net.coord.zcl_fragmented(short, BASIC, 0x00, 0x00, &request, 50);
    net.run(1_000);
    assert!(net.coord.last_zcl(short, BASIC, 0x01).is_none(), "the transfer is stuck on the lost block");
    net.run(2_000);
    let rsp = net.coord.last_zcl(short, BASIC, 0x01).expect("complete after the retry");
    assert_eq!(&rsp[..3], &[0x18, seq, 0x01]);
    assert_eq!(&rsp[3..], &response[..]);
}

#[test]
fn a_transfer_nobody_acknowledges_is_given_up() {
    let mut net = Net::new();
    let n = net.join_node();
    let short = net.nodes[n].short();
    let (request, _) = long_read(60);
    net.coord.drop_fragments = 100;
    let t = net.now();
    net.coord.zcl_fragmented(short, BASIC, 0x00, 0x00, &request, 50);
    net.run(20_000);
    // first block of the response: sent once and repeated three times, then nothing
    let blocks = net.sent_by(net.nodes[n].id, t).iter().filter(|f| f.len() > 80).count();
    assert_eq!(blocks, 4, "long frames sent");
    assert!(net.coord.last_zcl(short, BASIC, 0x01).is_none());
    // and the stack is free for the next long answer
    net.coord.drop_fragments = 0;
    let seq = net.coord.zcl_fragmented(short, BASIC, 0x00, 0x00, &request, 50);
    net.run(1_000);
    assert_eq!(net.coord.last_zcl(short, BASIC, 0x01).unwrap()[1], seq);
}

#[test]
fn a_transfer_of_thirty_two_blocks_completes() {
    let mut net = Net::new();
    let n = net.join_node();
    let short = net.nodes[n].short();
    // 110 attribute identifiers: 223 bytes of ZCL in 32 blocks of 7, the most blocks a transfer may have
    let (request, _) = long_read(110);
    let acks = net.coord.fragment_acks;
    let seq = net.coord.zcl_fragmented(short, BASIC, 0x00, 0x00, &request, 7);
    net.run(3_000);
    assert_eq!(net.coord.fragment_acks, acks + 32);
    let rsp = net.coord.last_zcl(short, BASIC, 0x01).expect("the request was put together and answered");
    assert_eq!(&rsp[..3], &[0x18, seq, 0x01]);
    // one block more is no transfer this bulb takes: nothing is acknowledged
    let acks = net.coord.fragment_acks;
    let (request, _) = long_read(113);
    net.coord.zcl_fragmented(short, BASIC, 0x00, 0x00, &request, 7);
    net.run(1_000);
    assert_eq!(net.coord.fragment_acks, acks);
}

#[test]
fn a_delivered_transfer_is_remembered_only_for_as_long_as_its_sender_may_repeat_it() {
    let mut net = Net::new();
    let n = net.join_node();
    let short = net.nodes[n].short();
    let mut add = vec![0, 0, 1, 0, 0, 100];
    add.extend_from_slice(&[b'n'; 100]);
    add.extend_from_slice(&[0x08, 0x00, 1, 77]);
    let seq = net.coord.zcl_fragmented(short, 0x0005, 0x01, 0x00, &add, 50);
    net.run(100);
    let answers = |net: &Net| net.coord.zcl_rx.iter().filter(|(s, c, f)| *s == short && *c == 0x0005 && f[1] == seq).count();
    assert_eq!(answers(&net), 1);
    // repeats within the sender's retries (three, 1.6 s apart) are acknowledged, not delivered
    net.run(4_800);
    net.coord.repeat_fragments();
    net.run(500);
    assert_eq!(answers(&net), 1);
    // Later the same APS counter is a new transfer (the counter is a byte
    // and comes round): it is put together and delivered.
    net.run(2_000);
    net.coord.repeat_fragments();
    net.run(500);
    assert_eq!(answers(&net), 2);
}

/// APS frames the bulb sent to the coordinator since `since`: the frames as they went into the NWK frame.
fn aps_sent(net: &Net, n: usize, since: u32) -> Vec<Vec<u8>> {
    net.nwk_sent_by(net.nodes[n].id, since)
        .into_iter()
        .filter(|(_, h, _)| h.frame_type == zigbee::nwk::FRAME_DATA && h.dst == 0x0000)
        .map(|e| e.2)
        .collect()
}

#[test]
fn the_acknowledgement_of_a_block_says_which_kind_of_block_it_was() {
    let mut net = Net::new();
    let n = net.join_node();
    let short = net.nodes[n].short();
    let (request, _) = long_read(60);
    let t = net.now();
    net.coord.zcl_fragmented(short, BASIC, 0x00, 0x00, &request, 50);
    net.run(500);
    // acknowledgements: extended header with the fragmentation value of the
    // block (first: 1, later: 2; R22 2.2.5.2.3), the block number, the bit field
    let acks: Vec<Vec<u8>> = aps_sent(&net, n, t).into_iter().filter(|a| a[0] & 3 == 2).map(|a| a[8..].to_vec()).collect();
    assert_eq!(acks, vec![vec![0x01, 0, 0xFF], vec![0x02, 1, 0xFF], vec![0x02, 2, 0xFF]]);
}

#[test]
fn a_long_answer_to_an_encrypted_request_is_encrypted_block_by_block() {
    let mut net = Net::new();
    let n = net.join_node();
    let short = net.nodes[n].short();
    // Read Attributes under the link key; the answer has 70 bytes, too
    // many for one frame once security has taken its share
    let (request, response) = long_read(22);
    let t = net.now();
    let before = net.coord.aps_encrypted_rx;
    let seq = net.coord.zcl_encrypted(short, BASIC, 0x00, 0x00, &request);
    net.run(1_000);
    let rsp = net.coord.last_zcl(short, BASIC, 0x01).expect("the response, decrypted and put together");
    assert_eq!(&rsp[..3], &[0x18, seq, 0x01]);
    assert_eq!(&rsp[3..], &response[..]);
    // the acknowledgement of the request and two blocks: all under the link key, nothing in the clear
    let sent = aps_sent(&net, n, t);
    assert_eq!(sent.iter().map(|a| a[0]).collect::<Vec<u8>>(), vec![0x22, 0xE0, 0xE0]);
    assert_eq!(net.coord.aps_encrypted_rx, before + 3);
    assert!(sent.iter().all(|a| !a.windows(4).any(|w| w == b"AwoX")), "the manufacturer name is not readable");
    // the same for an answer that needs more than two blocks
    let (request, response) = long_read(30);
    let seq = net.coord.zcl_encrypted(short, BASIC, 0x00, 0x00, &request);
    net.run(1_000);
    let t = net.now();
    let rsp = net.coord.last_zcl(short, BASIC, 0x01).unwrap();
    assert_eq!((rsp[1], &rsp[3..]), (seq, &response[..]));
    // an answer that fits is one encrypted frame, as before
    let seq = net.coord.zcl_encrypted(short, BASIC, 0x00, 0x00, &[0x04, 0x00]);
    net.run(500);
    assert_eq!(net.coord.last_zcl(short, BASIC, 0x01).unwrap()[1], seq);
    assert_eq!(aps_sent(&net, n, t).iter().map(|a| a[0]).collect::<Vec<u8>>(), vec![0x22, 0x20]);
}
