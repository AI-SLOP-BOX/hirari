pub const DEFAULT_PLUGIN_COLLECTION_ID: &str = "default";

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub enum PluginBlockReason {
    Unsupported32Bit,
    ScanCrash,
    LoadFailure(String),
    InvalidBinary,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct PluginInspection {
    pub id: String,
    pub name: String,
    pub vendor: String,
    pub category: String,
    pub format: String,
    pub version: String,
    pub path: String,
    pub bitness: u8,
    pub supports_f64: bool,
    pub asio_guard: bool,
    pub sidechain_inputs: u16,
    pub latency_samples: u32,
    pub hidden: bool,
    pub block_reason: Option<PluginBlockReason>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct PluginManagerRegistry {
    pub plugins: Vec<PluginInspection>,
}

impl PluginManagerRegistry {
    /// Commit one isolated scan result. A failed or unsupported binary remains
    /// visible on the blocklist instead of disappearing from diagnostics.
    pub fn record_scan(&mut self, mut plugin: PluginInspection, scan_succeeded: bool) -> bool {
        plugin.id = plugin.id.trim().to_owned();
        plugin.name = plugin.name.trim().to_owned();
        if plugin.bitness == 32 {
            plugin.block_reason = Some(PluginBlockReason::Unsupported32Bit);
        } else if !scan_succeeded && plugin.block_reason.is_none() {
            plugin.block_reason = Some(PluginBlockReason::ScanCrash);
        } else if scan_succeeded {
            plugin.block_reason = None;
        }
        if !plugin.validate() {
            return false;
        }
        if let Some(existing) = self.plugins.iter_mut().find(|item| item.id == plugin.id) {
            *existing = plugin;
        } else if self.plugins.len() < 65_536 {
            self.plugins.push(plugin);
        } else {
            return false;
        }
        self.plugins.sort_by_key(|item| item.id.clone());
        true
    }

    pub fn set_hidden(&mut self, id: &str, hidden: bool) -> bool {
        let Some(plugin) = self.plugins.iter_mut().find(|plugin| plugin.id == id) else {
            return false;
        };
        plugin.hidden = hidden;
        true
    }

    /// Reactivation is accepted only after an isolated rescan succeeds.
    /// Cubase-compatible 32-bit entries can never be reactivated.
    pub fn reactivate(&mut self, id: &str, rescan_succeeded: bool) -> bool {
        let Some(plugin) = self.plugins.iter_mut().find(|plugin| plugin.id == id) else {
            return false;
        };
        if plugin.bitness != 64 || !rescan_succeeded {
            return false;
        }
        plugin.block_reason = None;
        true
    }

    pub fn available(
        &self,
        used_in_project: Option<&BTreeSet<String>>,
        require_f64: bool,
    ) -> Vec<&PluginInspection> {
        let mut result = self
            .plugins
            .iter()
            .filter(|plugin| {
                !plugin.hidden
                    && plugin.block_reason.is_none()
                    && (!require_f64 || plugin.supports_f64)
                    && used_in_project.is_none_or(|ids| ids.contains(&plugin.id))
            })
            .collect::<Vec<_>>();
        result.sort_by_key(|plugin| {
            (
                plugin.vendor.to_ascii_lowercase(),
                plugin.name.to_ascii_lowercase(),
            )
        });
        result
    }

    pub fn blocklist(&self) -> Vec<&PluginInspection> {
        let mut result = self
            .plugins
            .iter()
            .filter(|plugin| plugin.block_reason.is_some())
            .collect::<Vec<_>>();
        result.sort_by_key(|plugin| plugin.name.to_ascii_lowercase());
        result
    }

    pub fn diagnostic_report(&self, system: &str) -> Result<String, String> {
        if !self.validate()
            || system.trim().is_empty()
            || system.len() > 1024
            || system.contains('\0')
        {
            return Err("invalid plug-in report data".into());
        }
        let mut report = format!(
            "Aura Plug-in Report\nSystem: {}\nPlug-ins: {}\n",
            system.trim(),
            self.plugins.len()
        );
        for plugin in &self.plugins {
            report.push_str(&format!(
                "{} | {} | {} | {}-bit | latency={} | sidechains={} | hidden={} | status={:?}\n",
                plugin.name,
                plugin.vendor,
                plugin.format,
                plugin.bitness,
                plugin.latency_samples,
                plugin.sidechain_inputs,
                plugin.hidden,
                plugin.block_reason
            ));
        }
        Ok(report)
    }

    pub fn validate(&self) -> bool {
        self.plugins.len() <= 65_536
            && self.plugins.iter().all(PluginInspection::validate)
            && self.plugins.windows(2).all(|pair| pair[0].id < pair[1].id)
    }
}

impl PluginInspection {
    fn validate(&self) -> bool {
        let text = |value: &str, max: usize| {
            !value.trim().is_empty() && value.len() <= max && !value.contains('\0')
        };
        text(&self.id, 256)
            && text(&self.name, 256)
            && text(&self.vendor, 256)
            && text(&self.category, 128)
            && text(&self.format, 32)
            && text(&self.version, 128)
            && text(&self.path, 4096)
            && matches!(self.bitness, 32 | 64)
            && self
                .block_reason
                .as_ref()
                .is_none_or(|reason| match reason {
                    PluginBlockReason::LoadFailure(message) => text(message, 1024),
                    _ => true,
                })
            && (self.bitness != 32
                || self.block_reason == Some(PluginBlockReason::Unsupported32Bit))
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct PluginCollectionEntry {
    pub plugin_id: String,
    pub folder: Vec<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct PluginCollection {
    pub id: String,
    pub name: String,
    pub entries: Vec<PluginCollectionEntry>,
    pub immutable: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct PluginCollectionManager {
    pub collections: Vec<PluginCollection>,
    pub active_id: String,
    pub available_plugin_ids: BTreeSet<String>,
    next_id: u64,
}

impl PluginCollectionManager {
    pub fn new<I, S>(available_ids: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        let available_plugin_ids: BTreeSet<_> = available_ids
            .into_iter()
            .map(Into::into)
            .filter(|id| valid_collection_token(id))
            .collect();
        let entries = available_plugin_ids
            .iter()
            .map(|plugin_id| PluginCollectionEntry {
                plugin_id: plugin_id.clone(),
                folder: Vec::new(),
            })
            .collect();
        Self {
            collections: vec![PluginCollection {
                id: DEFAULT_PLUGIN_COLLECTION_ID.into(),
                name: "Default".into(),
                entries,
                immutable: true,
            }],
            active_id: DEFAULT_PLUGIN_COLLECTION_ID.into(),
            available_plugin_ids,
            next_id: 1,
        }
    }

    /// A full rescan recreates Default while preserving unavailable references
    /// in user collections so projects can recover when a plug-in is restored.
    pub fn rescan<I, S>(&mut self, available_ids: I)
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.available_plugin_ids = available_ids
            .into_iter()
            .map(Into::into)
            .filter(|id| valid_collection_token(id))
            .collect();
        if let Some(default) = self
            .collections
            .iter_mut()
            .find(|collection| collection.id == DEFAULT_PLUGIN_COLLECTION_ID)
        {
            default.entries = self
                .available_plugin_ids
                .iter()
                .map(|plugin_id| PluginCollectionEntry {
                    plugin_id: plugin_id.clone(),
                    folder: Vec::new(),
                })
                .collect();
            default.name = "Default".into();
            default.immutable = true;
        }
    }

    pub fn create(&mut self, name: &str, include_all: bool) -> Option<String> {
        let name = name.trim();
        if !valid_collection_name(name)
            || self.collections.len() >= 256
            || self
                .collections
                .iter()
                .any(|collection| collection.name.eq_ignore_ascii_case(name))
        {
            return None;
        }
        let id = format!("user-{}", self.next_id);
        self.next_id = self.next_id.checked_add(1)?;
        let entries = if include_all {
            self.available_plugin_ids
                .iter()
                .map(|plugin_id| PluginCollectionEntry {
                    plugin_id: plugin_id.clone(),
                    folder: Vec::new(),
                })
                .collect()
        } else {
            Vec::new()
        };
        self.collections.push(PluginCollection {
            id: id.clone(),
            name: name.into(),
            entries,
            immutable: false,
        });
        Some(id)
    }

    pub fn copy_collection(&mut self, source_id: &str, name: &str) -> Option<String> {
        let entries = self
            .collections
            .iter()
            .find(|collection| collection.id == source_id)?
            .entries
            .clone();
        let id = self.create(name, false)?;
        self.collections
            .iter_mut()
            .find(|collection| collection.id == id)?
            .entries = entries;
        Some(id)
    }

    pub fn activate(&mut self, id: &str) -> bool {
        if !self
            .collections
            .iter()
            .any(|collection| collection.id == id)
        {
            return false;
        }
        self.active_id = id.into();
        true
    }

    pub fn rename(&mut self, id: &str, name: &str) -> bool {
        let name = name.trim();
        if !valid_collection_name(name)
            || self
                .collections
                .iter()
                .any(|collection| collection.id != id && collection.name.eq_ignore_ascii_case(name))
        {
            return false;
        }
        let Some(collection) = self
            .collections
            .iter_mut()
            .find(|collection| collection.id == id && !collection.immutable)
        else {
            return false;
        };
        collection.name = name.into();
        true
    }

    pub fn delete(&mut self, id: &str) -> bool {
        let Some(index) = self
            .collections
            .iter()
            .position(|collection| collection.id == id && !collection.immutable)
        else {
            return false;
        };
        self.collections.remove(index);
        if self.active_id == id {
            self.active_id = DEFAULT_PLUGIN_COLLECTION_ID.into();
        }
        true
    }

    pub fn add_plugin(&mut self, collection_id: &str, plugin_id: &str, folder: &[String]) -> bool {
        if !self.available_plugin_ids.contains(plugin_id) || !valid_folder(folder) {
            return false;
        }
        let Some(collection) = self
            .collections
            .iter_mut()
            .find(|collection| collection.id == collection_id && !collection.immutable)
        else {
            return false;
        };
        if collection.entries.len() >= 65_536
            || collection
                .entries
                .iter()
                .any(|entry| entry.plugin_id == plugin_id)
        {
            return false;
        }
        collection.entries.push(PluginCollectionEntry {
            plugin_id: plugin_id.into(),
            folder: folder.to_vec(),
        });
        true
    }

    pub fn remove_plugin(&mut self, collection_id: &str, plugin_id: &str) -> bool {
        let Some(collection) = self
            .collections
            .iter_mut()
            .find(|collection| collection.id == collection_id && !collection.immutable)
        else {
            return false;
        };
        let before = collection.entries.len();
        collection
            .entries
            .retain(|entry| entry.plugin_id != plugin_id);
        before != collection.entries.len()
    }

    pub fn remove_unavailable_from_user_collections(&mut self) -> usize {
        let mut removed = 0;
        for collection in self
            .collections
            .iter_mut()
            .filter(|collection| !collection.immutable)
        {
            let before = collection.entries.len();
            collection
                .entries
                .retain(|entry| self.available_plugin_ids.contains(&entry.plugin_id));
            removed += before - collection.entries.len();
        }
        removed
    }

    pub fn active_entries(&self, include_unavailable: bool) -> Vec<&PluginCollectionEntry> {
        let Some(collection) = self
            .collections
            .iter()
            .find(|collection| collection.id == self.active_id)
        else {
            return Vec::new();
        };
        collection
            .entries
            .iter()
            .filter(|entry| {
                include_unavailable || self.available_plugin_ids.contains(&entry.plugin_id)
            })
            .collect()
    }

    pub fn validate(&self) -> bool {
        self.next_id > 0
            && self.collections.len() <= 256
            && self
                .collections
                .iter()
                .any(|collection| collection.id == self.active_id)
            && self
                .collections
                .iter()
                .filter(|collection| {
                    collection.id == DEFAULT_PLUGIN_COLLECTION_ID
                        && collection.immutable
                        && collection.name == "Default"
                })
                .count()
                == 1
            && self
                .collections
                .iter()
                .enumerate()
                .all(|(index, collection)| {
                    valid_collection_token(&collection.id)
                        && valid_collection_name(&collection.name)
                        && collection.entries.len() <= 65_536
                        && self.collections[..index].iter().all(|previous| {
                            previous.id != collection.id
                                && !previous.name.eq_ignore_ascii_case(&collection.name)
                        })
                        && collection
                            .entries
                            .iter()
                            .enumerate()
                            .all(|(entry_index, entry)| {
                                valid_collection_token(&entry.plugin_id)
                                    && valid_folder(&entry.folder)
                                    && collection.entries[..entry_index]
                                        .iter()
                                        .all(|previous| previous.plugin_id != entry.plugin_id)
                            })
                })
            && self
                .collections
                .iter()
                .find(|collection| collection.id == DEFAULT_PLUGIN_COLLECTION_ID)
                .is_some_and(|default| {
                    default
                        .entries
                        .iter()
                        .map(|entry| &entry.plugin_id)
                        .eq(self.available_plugin_ids.iter())
                })
    }
}
