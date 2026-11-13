//! Wire protocol helpers — private-field redaction, address resolution and
//! frame decode/encode for the Moufu MQTT-like transport, split out of lib.rs.
use super::*;

pub(crate) fn redact_private_fields(value: &mut Value) {
    match value {
        Value::Object(fields) => {
            fields.retain(|key, _| !is_private_project_field(key));
            for child in fields.values_mut() {
                redact_private_fields(child);
            }
        }
        Value::Array(items) => {
            for item in items {
                redact_private_fields(item);
            }
        }
        _ => {}
    }
}

/// Normalized (alphanumeric, lowercased) key names that must never leave the
/// machine in a published project snapshot.
///
/// This is deliberately an explicit allow-list rather than a substring test:
/// `normalized.contains("path")` also destroyed legitimate public data such as
/// `graph_path` and turned `patch_name` into `patchname`-shaped noise. Only
/// names whose *entire* normalized form is listed here are stripped.
const PRIVATE_PROJECT_FIELD_NAMES: [&str; 22] = [
    // Absolute filesystem locations.
    "path",
    "filepath",
    "absolutepath",
    "fullpath",
    "directory",
    "dirpath",
    "folder",
    "folderpath",
    "frozenpath",
    "frozenfilepath",
    "sourcepath",
    "targetpath",
    "projectpath",
    "userpath",
    "filepathhint",
    "pathhint",
    // Opaque serialized plugin / GUI state.
    "pluginstate",
    "pluginstatehex",
    "pluginstateblob",
    "pluginstatestring",
    "stateblob",
    "guistate",
];

pub(crate) fn is_private_project_field(key: &str) -> bool {
    // Normalize separators so old snake_case keys and future camelCase keys
    // receive the same treatment (e.g. plugin_state_hex/pluginStateHex), then
    // require an exact allow-list match so public keys that merely mention
    // "path" (graph_path) or "name" (patch_name) survive redaction.
    let mut normalized = String::with_capacity(key.len());
    for character in key.chars() {
        if character.is_ascii_alphanumeric() {
            normalized.extend(character.to_lowercase());
        }
    }
    PRIVATE_PROJECT_FIELD_NAMES
        .iter()
        .any(|name| *name == normalized)
}

pub(crate) fn resolve_addresses(address: &str) -> io::Result<Vec<SocketAddr>> {
    let mut addresses = address.to_socket_addrs()?.collect::<Vec<_>>();
    addresses.dedup();
    if addresses.is_empty() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "Moufu address resolved empty",
        ));
    }
    Ok(addresses)
}

pub(crate) fn write_message<T: Serialize>(stream: &mut TcpStream, message: &T) -> io::Result<()> {
    serde_json::to_writer(&mut *stream, message).map_err(io::Error::other)?;
    stream.write_all(b"\n")?;
    stream.flush()
}

pub(crate) struct StreamShutdown(pub(crate) TcpStream);

impl Drop for StreamShutdown {
    fn drop(&mut self) {
        let _ = self.0.shutdown(std::net::Shutdown::Both);
    }
}

pub(crate) enum WireServerMessage {
    Protocol(ServerMessage),
    LinkDestroyed { link_id: LinkId },
    EntityList(Vec<EntityDescriptor>),
    LinkList(Vec<LinkInfo>),
}

pub(crate) fn decode_server_message(frame: &[u8]) -> Result<WireServerMessage, serde_json::Error> {
    let value = serde_json::from_slice::<Value>(frame)?;
    if value.get("type").and_then(Value::as_str) == Some("LinkDestroyed") {
        let link_id = value
            .get("data")
            .and_then(|data| data.get("link_id"))
            .cloned()
            .map(serde_json::from_value::<LinkId>)
            .transpose()?
            .ok_or_else(|| {
                serde_json::Error::io(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "Moufu LinkDestroyed message has no link_id",
                ))
            })?;
        return Ok(WireServerMessage::LinkDestroyed { link_id });
    }
    if value.get("type").and_then(Value::as_str) == Some("EntityList") {
        let entities = value
            .get("data")
            .cloned()
            .map(serde_json::from_value::<Vec<EntityDescriptor>>)
            .transpose()?
            .ok_or_else(|| {
                serde_json::Error::io(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "Moufu EntityList message has no data",
                ))
            })?;
        return Ok(WireServerMessage::EntityList(entities));
    }
    if value.get("type").and_then(Value::as_str) == Some("LinkList") {
        let links = value
            .get("data")
            .cloned()
            .map(serde_json::from_value::<Vec<LinkInfo>>)
            .transpose()?
            .ok_or_else(|| {
                serde_json::Error::io(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "Moufu LinkList message has no data",
                ))
            })?;
        return Ok(WireServerMessage::LinkList(links));
    }
    serde_json::from_value::<ServerMessage>(value).map(WireServerMessage::Protocol)
}

pub(crate) fn spawn_server_reader(
    reader: TcpStream,
) -> mpsc::Receiver<io::Result<WireServerMessage>> {
    let (tx, rx) = mpsc::sync_channel(MAX_QUEUED_SERVER_MESSAGES);
    let _ = thread::Builder::new()
        .name("hirari-moufu-reader".to_owned())
        .spawn(move || {
            let mut reader = BufReader::new(reader);
            let mut line = Vec::with_capacity(4096);
            loop {
                let message =
                    match read_bounded_frame(&mut reader, &mut line, MAX_SERVER_MESSAGE_BYTES) {
                        Ok(0) => Err(io::Error::new(
                            io::ErrorKind::UnexpectedEof,
                            "Moufu disconnected",
                        )),
                        Ok(_) if line.iter().all(u8::is_ascii_whitespace) => {
                            line.clear();
                            continue;
                        }
                        Ok(_) => decode_server_message(&line)
                            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error)),
                        Err(error) => Err(error),
                    };
                line.clear();
                let should_stop = message.is_err();
                if tx.send(message).is_err() || should_stop {
                    return;
                }
            }
        });
    rx
}

pub(crate) fn read_bounded_frame<R: BufRead>(
    reader: &mut R,
    frame: &mut Vec<u8>,
    max_bytes: usize,
) -> io::Result<usize> {
    loop {
        let available = reader.fill_buf()?;
        if available.is_empty() {
            return if frame.is_empty() {
                Ok(0)
            } else {
                Err(io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    "Moufu closed in the middle of a message",
                ))
            };
        }
        let take = available
            .iter()
            .position(|byte| *byte == b'\n')
            .map_or(available.len(), |newline| newline + 1);
        if frame.len().saturating_add(take) > max_bytes {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "Moufu message exceeds the configured size limit",
            ));
        }
        let complete = available[take - 1] == b'\n';
        frame.extend_from_slice(&available[..take]);
        reader.consume(take);
        if complete {
            return Ok(frame.len());
        }
    }
}

pub(crate) fn receive_message(
    messages: &mpsc::Receiver<io::Result<WireServerMessage>>,
    wait: Duration,
) -> io::Result<Option<WireServerMessage>> {
    match messages.recv_timeout(wait) {
        Ok(message) => message.map(Some),
        Err(RecvTimeoutError::Timeout) => Ok(None),
        Err(RecvTimeoutError::Disconnected) => Err(io::Error::new(
            io::ErrorKind::UnexpectedEof,
            "Moufu reader stopped",
        )),
    }
}
