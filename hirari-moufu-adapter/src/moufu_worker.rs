//! Worker-thread run loop and reconnect policy, split out of lib.rs.
use super::*;

pub(crate) fn run_worker(
    address: Arc<Mutex<Option<String>>>,
    latest: Arc<Mutex<Option<ProjectSnapshot>>>,
    stop: Arc<std::sync::atomic::AtomicBool>,
    events: Arc<MoufuEventQueue>,
    requests: mpsc::Receiver<QueuedMoufuRequest>,
) {
    while !stop.load(Ordering::Acquire) {
        let generation = events.generation.load(Ordering::Acquire);
        let configured_address = address.lock().ok().and_then(|address| address.clone());
        let Some(configured_address) = configured_address else {
            thread::sleep(Duration::from_millis(100));
            continue;
        };
        let result = resolve_addresses(&configured_address).and_then(|resolved| {
            connect_and_publish(
                resolved,
                &latest,
                &stop,
                &address,
                &configured_address,
                generation,
                &events,
                &requests,
            )
        });
        let stale_generation = !events.is_current(generation);
        let address_changed = address
            .lock()
            .map(|current| current.as_deref() != Some(configured_address.as_str()))
            .unwrap_or(true);
        if stale_generation || address_changed {
            continue;
        }
        match result {
            Ok(()) => log::info!("Moufu closed the Hirari integration connection"),
            Err(error) => {
                publish_event(
                    &events,
                    generation,
                    MoufuEvent::Disconnected {
                        reason: error.to_string(),
                    },
                );
                log::debug!("Hirari Moufu connection retry: {error}");
            }
        }
        for _ in 0..20 {
            if stop.load(Ordering::Acquire) {
                return;
            }
            if !events.is_current(generation)
                || address
                    .lock()
                    .map(|current| current.as_deref() != Some(configured_address.as_str()))
                    .unwrap_or(true)
            {
                break;
            }
            thread::sleep(Duration::from_millis(100));
        }
    }
}

pub(crate) fn connect_and_publish(
    addresses: Vec<SocketAddr>,
    latest: &Arc<Mutex<Option<ProjectSnapshot>>>,
    stop: &std::sync::atomic::AtomicBool,
    configured_address: &Arc<Mutex<Option<String>>>,
    connected_address: &str,
    generation: u64,
    events: &MoufuEventQueue,
    requests: &mpsc::Receiver<QueuedMoufuRequest>,
) -> io::Result<()> {
    let mut connected = None;
    let mut last_connect_error = None;
    for address in addresses {
        if stop.load(Ordering::Acquire)
            || !events.is_current(generation)
            || configured_address
                .lock()
                .map(|current| current.as_deref() != Some(connected_address))
                .unwrap_or(true)
        {
            return Ok(());
        }
        match TcpStream::connect_timeout(&address, Duration::from_secs(2)) {
            Ok(stream) => {
                connected = Some((address, stream));
                break;
            }
            Err(error) => last_connect_error = Some(error),
        }
    }
    let (address, stream) = connected.ok_or_else(|| {
        last_connect_error.unwrap_or_else(|| {
            io::Error::new(
                io::ErrorKind::AddrNotAvailable,
                "no Moufu address was attempted",
            )
        })
    })?;
    stream.set_nodelay(true)?;
    stream.set_write_timeout(Some(Duration::from_secs(2)))?;
    let mut writer = stream.try_clone()?;
    let reader = stream.try_clone()?;
    let _shutdown_guard = StreamShutdown(stream);
    let messages = spawn_server_reader(reader);

    write_message(
        &mut writer,
        &ClientMessage::Handshake {
            app_name: "Hirari".to_owned(),
            app_version: env!("CARGO_PKG_VERSION").to_owned(),
            capabilities: AppCapabilities {
                supports_live_link: true,
                supports_push_edits: false,
                supports_delta_sync: false,
                supports_link_restoration: true,
                supported_data_types: vec![HIRARI_PROJECT_DATA_TYPE.to_owned()],
            },
        },
    )?;

    let deadline = Instant::now() + Duration::from_secs(5);
    let mut acknowledged = false;
    while Instant::now() < deadline {
        if stop.load(Ordering::Acquire) {
            return Ok(());
        }
        if configured_address
            .lock()
            .map(|current| current.as_deref() != Some(connected_address))
            .unwrap_or(true)
            || !events.is_current(generation)
        {
            return Ok(());
        }
        write_queued_requests(&mut writer, requests, generation)?;
        match receive_message(&messages, Duration::from_millis(100))? {
            None => continue,
            Some(WireServerMessage::Protocol(ServerMessage::HandshakeAck {
                server_version,
                ..
            })) => {
                publish_event(events, generation, MoufuEvent::Connected { server_version });
                acknowledged = true;
                break;
            }
            Some(WireServerMessage::Protocol(ServerMessage::Error { code, message })) => {
                return Err(io::Error::other(format!(
                    "Moufu handshake rejected ({code}): {message}"
                )));
            }
            Some(_) => {}
        }
    }
    if !acknowledged {
        return Err(io::Error::new(
            io::ErrorKind::TimedOut,
            "Moufu handshake timed out",
        ));
    }

    log::info!("Hirari connected to Moufu at {address}");
    let mut metadata = std::collections::HashMap::new();
    metadata.insert("contract".to_owned(), HIRARI_PROJECT_CONTRACT.to_owned());
    metadata.insert(
        "edit-direction".to_owned(),
        "hirari-to-linked-app".to_owned(),
    );
    write_message(
        &mut writer,
        &ClientMessage::RegisterEntity(EntityDescriptor {
            id: EntityId::from(HIRARI_PROJECT_ENTITY),
            name: "Current Hirari Project".to_owned(),
            data_type: HIRARI_PROJECT_DATA_TYPE.to_owned(),
            source_app: "Hirari".to_owned(),
            metadata,
        }),
    )?;

    // The pinned wire crate predates Moufu's read-only catalog messages. Send
    // their stable tagged JSON forms after the handshake so older Hubs can
    // reject them as ordinary requests without aborting connection setup.
    write_message(&mut writer, &serde_json::json!({"type": "ListEntities"}))?;
    write_message(&mut writer, &serde_json::json!({"type": "ListLinks"}))?;

    let mut last_sent_version = 0;
    let mut publish_attempts: u32 = 0;
    let mut next_publish_at = Instant::now();
    let mut last_ping = Instant::now();
    let mut last_pong = Instant::now();
    let mut inbound_links = HashMap::new();
    let mut outbound_links = HashSet::new();
    loop {
        if stop.load(Ordering::Acquire) {
            return Ok(());
        }
        if configured_address
            .lock()
            .map(|current| current.as_deref() != Some(connected_address))
            .unwrap_or(true)
            || !events.is_current(generation)
        {
            return Ok(());
        }
        write_queued_requests(&mut writer, requests, generation)?;
        // While a publish is backing off, skip re-reading the same snapshot so a
        // permanently invalid or oversize payload cannot spin this loop.
        let publish_backoff_active = Instant::now() < next_publish_at;
        let snapshot = if outbound_links.is_empty() || publish_backoff_active {
            None
        } else {
            latest
                .lock()
                .map_err(|_| io::Error::other("publisher state poisoned"))?
                .as_ref()
                .filter(|snapshot| snapshot.version > last_sent_version)
                .cloned()
        };
        if let Some(snapshot) = snapshot {
            let mut payload = match serde_json::from_str::<Value>(&snapshot.layout_json) {
                Ok(payload) => payload,
                Err(error) => {
                    // The marker is deliberately NOT advanced: a snapshot whose
                    // JSON is temporarily unreadable (for example a layout
                    // captured mid-write) must be retried, not silently lost.
                    log::warn!(
                        "Hirari could not publish invalid project layout JSON to Moufu \
                         (version {}, attempt {}/{}): {error}",
                        snapshot.version,
                        publish_attempts + 1,
                        MAX_PUBLISH_ATTEMPTS
                    );
                    defer_publish(&mut publish_attempts, &mut next_publish_at, Instant::now());
                    if publish_attempts >= MAX_PUBLISH_ATTEMPTS {
                        log::error!(
                            "Hirari gave up publishing project layout version {} after {} failed \
                             attempts; later versions will be published instead",
                            snapshot.version,
                            publish_attempts
                        );
                        last_sent_version = snapshot.version;
                        reset_publish_backoff(&mut publish_attempts, &mut next_publish_at);
                    }
                    continue;
                }
            };
            apply_recording_input_channels(&mut payload, &snapshot.recording_input_channels);
            redact_private_fields(&mut payload);
            let event = ChangeEvent {
                entity_id: EntityId::from(HIRARI_PROJECT_ENTITY),
                version: snapshot.version,
                is_transient: snapshot.is_transient,
                timestamp: chrono::Utc::now(),
                payload: PayloadTransport::Inline(payload),
                delta: None,
            };
            match write_change_event(&mut writer, events, generation, event)? {
                PublishOutcome::Sent => {
                    last_sent_version = snapshot.version;
                    reset_publish_backoff(&mut publish_attempts, &mut next_publish_at);
                }
                PublishOutcome::Rejected(reason) => {
                    // Oversize payloads are surfaced to the user as a
                    // ServerNotice by write_change_event and must stay visible
                    // here, but they are not sent, so the marker stays put and
                    // the bounded backoff below keeps a permanently oversized
                    // project from spinning the publish loop forever.
                    log::warn!(
                        "Hirari could not publish project layout version {} to Moufu \
                         (attempt {}/{}): {reason}",
                        snapshot.version,
                        publish_attempts + 1,
                        MAX_PUBLISH_ATTEMPTS
                    );
                    defer_publish(&mut publish_attempts, &mut next_publish_at, Instant::now());
                    if publish_attempts >= MAX_PUBLISH_ATTEMPTS {
                        log::error!(
                            "Hirari gave up publishing project layout version {} after {} failed \
                             attempts; later versions will be published instead",
                            snapshot.version,
                            publish_attempts
                        );
                        last_sent_version = snapshot.version;
                        reset_publish_backoff(&mut publish_attempts, &mut next_publish_at);
                    }
                }
            }
        } else {
            reset_publish_backoff(&mut publish_attempts, &mut next_publish_at);
        }

        if last_ping.elapsed() >= Duration::from_secs(3) {
            write_message(&mut writer, &ClientMessage::Ping)?;
            last_ping = Instant::now();
        }
        if last_pong.elapsed() >= Duration::from_secs(12) {
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "Moufu heartbeat timed out",
            ));
        }

        match receive_message(&messages, Duration::from_millis(100))? {
            None => continue,
            Some(WireServerMessage::LinkDestroyed { link_id }) => {
                inbound_links.remove(&link_id);
                outbound_links.remove(&link_id);
                publish_event(events, generation, MoufuEvent::LinkDestroyed { link_id });
            }
            Some(WireServerMessage::EntityList(entities)) => {
                publish_event(events, generation, MoufuEvent::EntityList(entities));
            }
            Some(WireServerMessage::LinkList(links)) => {
                publish_event(events, generation, MoufuEvent::LinkList(links));
            }
            Some(WireServerMessage::Protocol(ServerMessage::Pong)) => last_pong = Instant::now(),
            Some(WireServerMessage::Protocol(ServerMessage::LinkStatusChanged(link))) => {
                log_link_status(&link);
                if link.source_entity.0 == HIRARI_PROJECT_ENTITY {
                    if link.status == LinkStatus::Active
                        && link.target_entity.0 != HIRARI_PROJECT_ENTITY
                    {
                        outbound_links.insert(link.link_id);
                    } else {
                        outbound_links.remove(&link.link_id);
                    }
                }
                // A restored or newly created source -> Hirari link may have
                // been established before this connection came back. Request
                // the source's current snapshot so consumers don't have to
                // wait for its next edit before learning its state.
                if link.target_entity.0 == HIRARI_PROJECT_ENTITY {
                    let was_source_linked = inbound_links
                        .values()
                        .any(|source| source == &link.source_entity);
                    if link.status == LinkStatus::Active
                        && link.source_entity.0 != HIRARI_PROJECT_ENTITY
                    {
                        inbound_links.insert(link.link_id, link.source_entity.clone());
                    } else {
                        inbound_links.remove(&link.link_id);
                    }
                    let is_source_linked = inbound_links
                        .values()
                        .any(|source| source == &link.source_entity);
                    if !was_source_linked && is_source_linked {
                        write_message(
                            &mut writer,
                            &ClientMessage::PullEntity {
                                entity_id: link.source_entity.clone(),
                            },
                        )?;
                    }
                }
                publish_event(events, generation, MoufuEvent::LinkStatusChanged(link));
            }
            Some(WireServerMessage::Protocol(ServerMessage::EntityUpdated(event))) => {
                if !inbound_links
                    .values()
                    .any(|source| source == &event.entity_id)
                {
                    log::debug!(
                        "Ignored Moufu update for unlinked entity {}",
                        event.entity_id
                    );
                    continue;
                }
                log::debug!(
                    "Moufu delivered linked entity {} at version {}",
                    event.entity_id,
                    event.version
                );
                publish_event(events, generation, MoufuEvent::EntityUpdated(event));
            }
            Some(WireServerMessage::Protocol(ServerMessage::EntityState {
                entity_id,
                version,
                payload,
            })) => {
                if !inbound_links.values().any(|source| source == &entity_id) {
                    log::debug!("Ignored Moufu state for unlinked entity {}", entity_id);
                    continue;
                }
                log::debug!("Moufu returned entity {} at version {}", entity_id, version);
                publish_event(
                    events,
                    generation,
                    MoufuEvent::EntityState {
                        entity_id,
                        version,
                        payload,
                    },
                );
            }
            Some(WireServerMessage::Protocol(ServerMessage::Error { code, message })) => {
                log::warn!("Moufu rejected a Hirari request ({code}): {message}");
                publish_event(
                    events,
                    generation,
                    MoufuEvent::ServerNotice { code, message },
                );
            }
            Some(WireServerMessage::Protocol(ServerMessage::HandshakeAck { .. })) => {}
        }
    }
}
