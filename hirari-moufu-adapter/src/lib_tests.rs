//! Tests split out of lib.rs.

use super::*;
use moufu_protocol::AppSessionId;
use std::net::TcpListener;
use std::sync::mpsc::channel;

/// Catalog frames the pinned protocol rev does not define. The adapter
/// emits them deliberately as a forward-compatibility probe, and the Hub's
/// read loop drops unparseable frames without closing the session, so the
/// test server must tolerate exactly these and nothing else.
const EXPECTED_UNSUPPORTED_PROBES: [&str; 2] = ["ListEntities", "ListLinks"];

/// Public keys that merely mention a private word must not be redacted,
/// while genuine filesystem locations and opaque plugin state must be.
#[test]
fn redaction_uses_an_exact_key_allow_list() {
    let mut value = serde_json::json!({
        "graph_path": "/nodes/0/2",
        "patch_name": "Intro Verse",
        "graph_node_path": "sine/partials",
        "name": "Vocal",
        "tracks": [{
            "name": "Vocal",
            "path": "/Users/private/voice.wav",
            "frozen_path": "/Users/private/frozen.wav",
            "plugin_state_hex": ["secret"],
            "pluginStateHex": ["secret"],
            "graph_path": "/tracks/0/1",
            "patch_name": "Take 3",
            "volume": 0.8,
        }],
    });
    redact_private_fields(&mut value);
    let track = &value["tracks"][0];
    assert_eq!(track["graph_path"], "/tracks/0/1");
    assert_eq!(track["patch_name"], "Take 3");
    assert_eq!(value["graph_path"], "/nodes/0/2");
    assert_eq!(value["patch_name"], "Intro Verse");
    assert_eq!(value["graph_node_path"], "sine/partials");
    assert!(track.get("path").is_none());
    assert!(track.get("frozen_path").is_none());
    assert!(track.get("plugin_state_hex").is_none());
    assert!(track.get("pluginStateHex").is_none());
    assert_eq!(track["name"], "Vocal");
    assert_eq!(track["volume"], 0.8);
}

fn read_client_message(reader: &mut BufReader<TcpStream>) -> ClientMessage {
    loop {
        let mut line = String::new();
        let bytes = reader.read_line(&mut line).unwrap();
        assert_ne!(bytes, 0, "server closed before a client message arrived");
        match serde_json::from_str(&line) {
            Ok(message) => return message,
            Err(error) => {
                let tag = serde_json::from_str::<serde_json::Value>(&line)
                    .ok()
                    .and_then(|value| {
                        value
                            .get("type")
                            .and_then(serde_json::Value::as_str)
                            .map(str::to_owned)
                    })
                    .unwrap_or_default();
                assert!(
                    EXPECTED_UNSUPPORTED_PROBES.contains(&tag.as_str()),
                    "unexpected client frame {tag:?} ({error}): {line}"
                );
            }
        }
    }
}

#[test]
fn server_frames_are_bounded_and_require_a_complete_newline() {
    let mut reader = BufReader::new(std::io::Cursor::new(b"abc\n"));
    let mut frame = Vec::new();
    assert_eq!(read_bounded_frame(&mut reader, &mut frame, 4).unwrap(), 4);
    assert_eq!(frame, b"abc\n");

    let mut reader = BufReader::new(std::io::Cursor::new(b"oversized\n"));
    let error = read_bounded_frame(&mut reader, &mut Vec::new(), 4).unwrap_err();
    assert_eq!(error.kind(), io::ErrorKind::InvalidData);

    let mut reader = BufReader::new(std::io::Cursor::new(b"partial"));
    let error = read_bounded_frame(&mut reader, &mut Vec::new(), 16).unwrap_err();
    assert_eq!(error.kind(), io::ErrorKind::UnexpectedEof);
}

#[test]
fn publisher_registers_hirari_and_sends_live_project_snapshot() {
    // The whole handshake chain is processed on real threads and the timers
    // below bound them, not correctness. The internal Linux CI runs under
    // xvfb alongside a full build fleet and routinely takes several
    // seconds longer than an idle developer machine.
    let deadline = Duration::from_secs(15);
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let (observed_tx, observed_rx) = channel();
    let server = thread::spawn(move || {
        let (mut socket, _) = listener.accept().unwrap();
        socket.set_read_timeout(Some(deadline)).unwrap();
        let mut reader = BufReader::new(socket.try_clone().unwrap());
        let handshake = read_client_message(&mut reader);
        observed_tx.send(handshake.clone()).unwrap();
        assert!(matches!(
            handshake,
            ClientMessage::Handshake { app_name, .. } if app_name == "Hirari"
        ));

        writeln!(socket).unwrap();
        let ack = ServerMessage::HandshakeAck {
            session_id: AppSessionId::new_v4(),
            server_version: "test".to_owned(),
        };
        writeln!(socket, "{}", serde_json::to_string(&ack).unwrap()).unwrap();

        let registration = read_client_message(&mut reader);
        observed_tx.send(registration.clone()).unwrap();
        assert!(matches!(
            registration,
            ClientMessage::RegisterEntity(EntityDescriptor { source_app, .. })
                if source_app == "Hirari"
        ));

        let source_id = EntityId::from("moufu://source/companion");
        let link = LinkInfo {
            link_id: moufu_protocol::LinkId::new(),
            source_app: "Companion".to_owned(),
            source_entity: source_id.clone(),
            target_app: "Hirari".to_owned(),
            target_entity: EntityId::from(HIRARI_PROJECT_ENTITY),
            data_type: HIRARI_PROJECT_DATA_TYPE.to_owned(),
            status: LinkStatus::Active,
            last_synced_version: 0,
            created_at: chrono::Utc::now(),
        };
        let link_status = ServerMessage::LinkStatusChanged(link);
        writeln!(socket, "{}", serde_json::to_string(&link_status).unwrap()).unwrap();

        // The adapter is a pull-only app (it advertises
        // supports_push_edits: false), and it only publishes a project
        // snapshot once a link points *out* of Hirari
        // (moufu-source -> hirari-target is the inbound direction and
        // never populates `outbound_links`). A real session therefore has
        // both directions registered; model the outbound one so the
        // publish path is reachable.
        let outbound_link = ServerMessage::LinkStatusChanged(LinkInfo {
            link_id: moufu_protocol::LinkId::new(),
            source_app: "Hirari".to_owned(),
            source_entity: EntityId::from(HIRARI_PROJECT_ENTITY),
            target_app: "Companion".to_owned(),
            target_entity: source_id.clone(),
            data_type: HIRARI_PROJECT_DATA_TYPE.to_owned(),
            status: LinkStatus::Active,
            last_synced_version: 0,
            created_at: chrono::Utc::now(),
        });
        writeln!(socket, "{}", serde_json::to_string(&outbound_link).unwrap()).unwrap();

        let mut received_pull = false;
        let mut received_live_snapshot = false;
        while !received_pull || !received_live_snapshot {
            let message = read_client_message(&mut reader);
            match message {
                ClientMessage::PullEntity { entity_id } => {
                    assert_eq!(entity_id, source_id);
                    received_pull = true;
                }
                ClientMessage::NotifyChange(change) if change.is_transient => {
                    observed_tx
                        .send(ClientMessage::NotifyChange(change.clone()))
                        .unwrap();
                    assert!(matches!(
                        change.payload,
                        PayloadTransport::Inline(ref value)
                            if value["title"] == "Night Drive"
                                && value["tracks"][0].get("path").is_none()
                                && value["tracks"][0].get("frozen_path").is_none()
                                && value["tracks"][0].get("plugin_state_hex").is_none()
                    ));
                    received_live_snapshot = true;
                }
                // The real Hub answers every heartbeat with a bare Pong
                // (moufu-core/src/server.rs, ClientMessage::Ping arm).
                // Without this the adapter's 3s ping reaches the strict
                // match below and fails an otherwise healthy session.
                ClientMessage::Ping => {
                    writeln!(
                        socket,
                        "{}",
                        serde_json::to_string(&ServerMessage::Pong).unwrap()
                    )
                    .unwrap();
                }
                other => panic!("unexpected client message: {other:?}"),
            }
        }

        let state = ServerMessage::EntityState {
            entity_id: source_id.clone(),
            // Keep the state sample below the update's version: at the
            // same version the adapter's queue keeps the committed
            // snapshot and drops the transient update, which made this
            // test observation-dependent on scheduling.
            version: 41,
            payload: PayloadTransport::Inline(serde_json::json!({"title": "Companion"})),
        };
        writeln!(socket, "{}", serde_json::to_string(&state).unwrap()).unwrap();
        let update = ServerMessage::EntityUpdated(ChangeEvent {
            entity_id: source_id,
            version: 42,
            is_transient: true,
            timestamp: chrono::Utc::now(),
            payload: PayloadTransport::Inline(serde_json::json!({"title": "Companion"})),
            delta: None,
        });
        writeln!(socket, "{}", serde_json::to_string(&update).unwrap()).unwrap();

        // The adapter pings every 3s and the socket is long-lived; on a
        // loaded host (CI under xvfb) a Ping can land inside this slot
        // before the layout commit arrives. Mirror the main loop: answer
        // it so the wait does not depend on wall-clock cadence.
        let commit = loop {
            match read_client_message(&mut reader) {
                ClientMessage::Ping => {
                    writeln!(
                        socket,
                        "{}",
                        serde_json::to_string(&ServerMessage::Pong).unwrap()
                    )
                    .unwrap();
                }
                // Tolerate noise that the strict match below cannot see:
                // a duplicate pull echo can race a reconnect window on a
                // busy host. The commit itself is still strictly asserted.
                ClientMessage::PullEntity { .. } => continue,
                message => break message,
            }
        };
        observed_tx.send(commit.clone()).unwrap();
        assert!(
            matches!(
                commit,
                ClientMessage::NotifyChange(ChangeEvent {
                    is_transient: false,
                    payload: PayloadTransport::Inline(ref value),
                    ..
                }) if value["title"] == "Night Drive"
            ),
            "commit message mismatch: {commit:?}"
        );
    });

    let publisher = MoufuPublisher::connect_to(&address.to_string()).unwrap();
    publisher.publish_layout(
            r#"{"title":"Night Drive","tracks":[{"name":"Vocal","path":"/Users/private/voice.wav","frozen_path":"/Users/private/frozen.wav","plugin_state_hex":["secret"],"volume":0.8}]}"#,
            true,
        );

    let handshake = observed_rx.recv_timeout(deadline).unwrap();
    assert!(matches!(
        handshake,
        ClientMessage::Handshake { app_name, .. } if app_name == "Hirari"
    ));
    let registration = observed_rx.recv_timeout(deadline).unwrap();
    assert!(matches!(
        registration,
        ClientMessage::RegisterEntity(EntityDescriptor {
            id: EntityId(ref id),
            data_type,
            ..
        }) if id == HIRARI_PROJECT_ENTITY && data_type == HIRARI_PROJECT_DATA_TYPE
    ));
    let mut received_connected = false;
    let mut received_update = false;
    let event_deadline = Instant::now() + deadline;
    while Instant::now() < event_deadline && !received_update {
        if let Some(event) = publisher.recv_event_timeout(Duration::from_millis(100)) {
            match event {
                MoufuEvent::Connected { server_version } => {
                    received_connected = server_version == "test";
                }
                MoufuEvent::EntityUpdated(event) => {
                    received_update = event.version == 42
                        && event.entity_id == EntityId::from("moufu://source/companion")
                        && matches!(event.payload, PayloadTransport::Inline(ref payload) if payload["title"] == "Companion");
                }
                _ => {}
            }
        }
    }
    assert!(received_connected);
    assert!(received_update);
    let event = observed_rx.recv_timeout(deadline).unwrap();
    assert!(matches!(
        event,
        ClientMessage::NotifyChange(ChangeEvent {
            is_transient: true,
            payload: PayloadTransport::Inline(ref payload),
            ..
        }) if payload["title"] == "Night Drive"
            && payload["tracks"][0]["name"] == "Vocal"
            && payload["tracks"][0]["volume"] == 0.8
            && payload["tracks"][0].get("path").is_none()
            && payload["tracks"][0].get("frozen_path").is_none()
            && payload["tracks"][0].get("plugin_state_hex").is_none()
    ));
    publisher.publish_layout(
            r#"{"title":"Night Drive","tracks":[{"name":"Vocal","path":"/Users/private/voice.wav","frozen_path":"/Users/private/frozen.wav","plugin_state_hex":["secret"],"volume":0.8}]}"#,
            false,
        );
    let committed = observed_rx.recv_timeout(deadline).unwrap();
    assert!(matches!(
        committed,
        ClientMessage::NotifyChange(ChangeEvent {
            is_transient: false,
            ..
        })
    ));

    publisher.shutdown();
    server.join().unwrap();
}
