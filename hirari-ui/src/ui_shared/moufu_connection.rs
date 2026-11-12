//! Moufu Hub address persistence. Neutral artifact: the AmeUI card and any tooling read and write
//! the same `moufu-connection.json` in `hirari-ui/` under the platform config directory.

use std::fs;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

fn ui_config_dir() -> Option<PathBuf> {
    std::env::var_os("HIRARI_CONFIG_DIR")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("XDG_CONFIG_HOME").map(PathBuf::from))
        .or_else(|| {
            std::env::var_os("APPDATA").map(PathBuf::from).or_else(|| {
                std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".config"))
            })
        })
}

#[derive(serde::Deserialize, serde::Serialize)]
struct MoufuConnectionSettings {
    address: String,
}

fn moufu_settings_path() -> Option<PathBuf> {
    Some(
        ui_config_dir()?
            .join("hirari-ui")
            .join("moufu-connection.json"),
    )
}

pub(crate) fn load_moufu_address() -> Option<String> {
    let contents = fs::read_to_string(moufu_settings_path()?).ok()?;
    let settings = serde_json::from_str::<MoufuConnectionSettings>(&contents).ok()?;
    let address = settings.address.trim();
    (!address.is_empty()).then(|| address.to_owned())
}

pub(crate) fn save_moufu_address(address: &str) -> std::io::Result<()> {
    let path = moufu_settings_path().ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::NotFound,
            "Hirari configuration directory is unavailable",
        )
    })?;
    let parent = path.parent().ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "invalid Moufu settings path",
        )
    })?;
    fs::create_dir_all(parent)?;
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let temporary = parent.join(format!(
        "moufu-connection-{}-{nonce}.tmp",
        std::process::id()
    ));
    let serialized = serde_json::to_vec(&MoufuConnectionSettings {
        address: address.trim().to_owned(),
    })
    .map_err(std::io::Error::other)?;
    fs::write(&temporary, serialized)?;
    fs::rename(&temporary, &path)?;
    Ok(())
}
