use super::*;

#[test]
fn server_receive_returns_ssp_state_numbers_without_changing_wire_protocol() {
    use moshcatty::pb::UserInstruction;

    let key = [0x42u8; 16];
    let mut server = ServerTransport::new(Ocb::new(&key).unwrap());
    let mut client = Transport::new_client(Ocb::new(&key).unwrap());
    let payload =
        UserInstruction::encode_message(&[UserInstruction::keystroke(b"echo test\r".to_vec())]);
    let expected_new = client.set_pending(payload.clone());
    client.force_next_send();

    let mut accepted = None;
    for datagram in client.tick() {
        let outcome = server.receive(&datagram).unwrap();
        if outcome.state.is_some() {
            accepted = outcome.state;
        }
    }

    let accepted = accepted.expect("client state should complete");
    assert_eq!(accepted.old_num, 0);
    assert_eq!(accepted.new_num, expected_new);
    assert_eq!(accepted.diff, payload);
}

#[test]
fn received_state_is_explicit_server_api() {
    let state = ReceivedState {
        old_num: 7,
        new_num: 11,
        ack_num: 5,
        throwaway_num: 4,
        diff: b"user-stream-diff".to_vec(),
    };
    assert_eq!(state.old_num, 7);
    assert_eq!(state.new_num, 11);
    assert_eq!(state.diff, b"user-stream-diff");
}

#[test]
fn terminal_query_extension_is_ignored_by_stock_host_decoder() {
    let encoded = encode_host_message(
        &[],
        &[TerminalQuery {
            id: 7,
            bytes: b"\x1b]10;?\x07".to_vec(),
        }],
    );

    assert_eq!(
        encoded,
        vec![
            0x0A, 0x0E, 0xA2, 0x01, 0x0B, 0x08, 0x07, 0x12, 0x07, 0x1B, 0x5D, 0x31, 0x30, 0x3B,
            0x3F, 0x07,
        ]
    );
    let stock = HostInstruction::decode_message(&encoded).unwrap();
    assert_eq!(stock.len(), 1);
    assert!(stock[0].hoststring.is_empty());
    assert_eq!(stock[0].width, 0);
    assert_eq!(stock[0].height, 0);
    assert_eq!(stock[0].echo_ack_num, -1);
}

#[test]
fn agent_host_extension_is_ignored_by_stock_host_decoder() {
    let encoded = encode_host_message_with_agent(
        &[],
        &[],
        &[
            AgentHostRecord::Ready {
                supported: true,
                error: None,
            },
            AgentHostRecord::Request {
                id: 4,
                connection_id: 8,
                frame: vec![0, 0, 0, 1, 6],
            },
        ],
    );

    let stock = HostInstruction::decode_message(&encoded).unwrap();
    assert_eq!(stock.len(), 2);
    assert!(stock.iter().all(|instruction| {
        instruction.hoststring.is_empty()
            && instruction.width == 0
            && instruction.height == 0
            && instruction.echo_ack_num == -1
    }));
    assert!(encoded.windows(2).any(|tag| tag == [0xAA, 0x01]));
}
