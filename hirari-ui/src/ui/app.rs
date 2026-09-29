//! Slint application bootstrap and callback wiring.

use crate::orchestrator::CoreOrchestrator;
use crate::slint_ui::*;
use hirari_core_bridge::HirariCore;
#[cfg(debug_assertions)]
use slint::Model;
use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::rc::Rc;
use std::sync::atomic::AtomicU64;
use std::sync::{Arc, Mutex};

fn core_markers(core: &HirariCore) -> Vec<Z_Marker> {
    #[derive(serde::Deserialize)]
    struct Marker {
        id: u32,
        label: String,
        beat: f64,
        color: String,
    }
    let parsed = serde_json::from_str::<Vec<Marker>>(&core.markers_json()).unwrap_or_default();
    if parsed.is_empty() {
        return vec![
            Z_Marker {
                id: 1,
                label: "START".into(),
                beat: 0.0,
                color: slint::Color::from_rgb_u8(100, 100, 150),
            },
            Z_Marker {
                id: 2,
                label: "DEVELOPMENT".into(),
                beat: 32.0,
                color: slint::Color::from_rgb_u8(150, 100, 100),
            },
        ];
    }
    parsed
        .into_iter()
        .map(|marker| {
            let hex = marker.color.trim_start_matches('#');
            let rgb = u32::from_str_radix(hex, 16).unwrap_or(0x6472a8);
            Z_Marker {
                id: marker.id as i32,
                label: marker.label.into(),
                beat: marker.beat as f32,
                color: slint::Color::from_rgb_u8((rgb >> 16) as u8, (rgb >> 8) as u8, rgb as u8),
            }
        })
        .collect()
}

fn linked_project_preview(
    source: &str,
    version: u64,
    transient: bool,
    payload: &crate::moufu::PayloadTransport,
) -> (String, String) {
    let state = match payload {
        crate::moufu::PayloadTransport::Inline(value) => value,
        crate::moufu::PayloadTransport::SharedMemory(descriptor) => {
            return (
                source.to_owned(),
                format!(
                    "{} snapshot · v{version} · shared memory payload ({} bytes)",
                    if transient {
                        "Live preview"
                    } else {
                        "Committed"
                    },
                    descriptor.size_bytes
                ),
            );
        }
    };
    let tracks = state
        .as_array()
        .or_else(|| state.get("tracks").and_then(serde_json::Value::as_array));
    let Some(tracks) = tracks else {
        return (
            source.to_owned(),
            format!(
                "{} snapshot · v{version} · payload received, but no track list is available",
                if transient {
                    "Live preview"
                } else {
                    "Committed"
                }
            ),
        );
    };
    let mut lines = vec![format!(
        "{} snapshot · v{version} · {} tracks",
        if transient {
            "Live preview"
        } else {
            "Committed"
        },
        tracks.len()
    )];
    if let Some(object) = state.as_object() {
        let mut details = Vec::new();
        if let Some(bpm) = object.get("tempo_bpm").and_then(serde_json::Value::as_f64) {
            if bpm.is_finite() {
                details.push(format!("{bpm:.2} BPM"));
            }
        }
        for (key, label) in [
            ("midi_notes", "MIDI notes"),
            ("markers", "markers"),
            ("time_signature_events", "meter changes"),
        ] {
            if let Some(items) = object.get(key).and_then(serde_json::Value::as_array) {
                details.push(format!("{} {label}", items.len()));
            }
        }
        if let Some(events) = object
            .get("tempo_events")
            .and_then(serde_json::Value::as_array)
        {
            details.push(format!("{} tempo points", events.len()));
        }
        if !details.is_empty() {
            lines[0].push_str(" · ");
            lines[0].push_str(&details.join(" · "));
        }
    }
    for track in tracks.iter().take(8) {
        let name = track
            .get("name")
            .and_then(serde_json::Value::as_str)
            .map(one_line_remote_preview_text)
            .filter(|name| !name.is_empty())
            .unwrap_or_else(|| "Unnamed track".to_owned());
        let region_count = track
            .get("regions")
            .and_then(serde_json::Value::as_array)
            .map_or(0, Vec::len);
        let input_channels = track
            .get("recording_input_channels")
            .and_then(serde_json::Value::as_array)
            .map(|channels| {
                channels
                    .iter()
                    .filter_map(serde_json::Value::as_u64)
                    .map(|channel| channel.to_string())
                    .collect::<Vec<_>>()
                    .join("/")
            })
            .filter(|channels| !channels.is_empty());
        if let Some(input_channels) = input_channels {
            lines.push(format!(
                "{name} · input {input_channels} · {region_count} regions"
            ));
        } else {
            lines.push(format!("{name} · {region_count} regions"));
        }
    }
    if tracks.len() > 8 {
        lines.push(format!("…and {} more", tracks.len() - 8));
    }
    (source.to_owned(), lines.join("\n"))
}

/// Remote project names are user supplied and must not be able to inject
/// extra preview rows, reorder text with bidi controls, or flood the UI.
fn one_line_remote_preview_text(value: &str) -> String {
    let mut output = String::new();
    let mut previous_was_space = true;
    let mut truncated = false;
    for character in value.chars() {
        let bidi_control = matches!(
            character,
            '\u{061c}'
                | '\u{200e}'
                | '\u{200f}'
                | '\u{202a}'..='\u{202e}'
                | '\u{2066}'..='\u{2069}'
                | '\u{feff}'
        );
        if bidi_control {
            continue;
        }
        let character = if character.is_control() || character.is_whitespace() {
            ' '
        } else {
            character
        };
        if character == ' ' {
            if previous_was_space {
                continue;
            }
            previous_was_space = true;
        } else {
            previous_was_space = false;
        }
        if output.chars().count() >= 120 {
            truncated = true;
            break;
        }
        output.push(character);
    }
    let output = output.trim_end().to_owned();
    if truncated {
        format!("{output}…")
    } else {
        output
    }
}

fn accept_moufu_version(
    versions: &mut HashMap<String, (u64, bool)>,
    source: &str,
    version: u64,
    transient: bool,
) -> bool {
    if versions
        .get(source)
        .is_some_and(|(current_version, current_transient)| {
            version < *current_version
                || (version == *current_version && (!*current_transient || transient))
        })
    {
        return false;
    }
    versions.insert(source.to_owned(), (version, transient));
    true
}

pub fn run() {
    #[cfg(feature = "slint-wgpu")]
    {
        let inspect_renderer = std::env::var("HIRARI_UI_RENDERER").ok();
        let result = if inspect_renderer.as_deref() == Some("femtovg") {
            slint::BackendSelector::new()
                .renderer_name("femtovg".into())
                .select()
        } else {
            slint::BackendSelector::new()
                .require_wgpu_29(slint::wgpu_29::WGPUConfiguration::default())
                .select()
        };
        if let Err(error) = result {
            // Keep the application inspectable on machines where the native
            // WGPU adapter is unavailable. Production builds still prefer
            // WGPU; FemtoVG is an explicit UI inspection escape hatch.
            eprintln!("Hirari renderer initialization failed: {error}; trying Slint fallback");
            if let Err(fallback) = slint::BackendSelector::new().select() {
                eprintln!("Hirari Slint fallback initialization failed: {fallback}");
                return;
            }
        }
    }
    let ui = match AppWindow::new() {
        Ok(ui) => ui,
        Err(error) => {
            eprintln!("Hirari UI initialization failed: {error}");
            return;
        }
    };
    let recent_projects = Rc::new(slint::VecModel::from(
        crate::slint_ui::load_recent_projects(),
    ));
    ui.set_recent_projects(slint::ModelRc::from(
        recent_projects as Rc<dyn slint::Model<Data = Z_Recent_Project>>,
    ));
    // Persist the focused beginner surface versus the full advanced surface.
    // The existing production_mode property is the shared presentation gate.
    ui.set_production_mode(load_beginner_mode());
    let (onboarding_completed, onboarding_step) = load_onboarding_progress();
    ui.set_beginner_guide_step(i32::from(onboarding_step.min(6)));
    ui.set_beginner_guide_open(!onboarding_completed);
    ui.set_focus_mode(load_focus_mode());
    let workspace_preset = load_workspace_preset();
    ui.set_workspace_preset(workspace_preset.clone().into());
    match workspace_preset.as_str() {
        "mix" => {
            ui.set_bot_view(0);
            ui.set_mx_open(true);
        }
        "vocal" => {
            ui.set_bot_view(8);
            ui.set_mx_open(true);
        }
        "sound_design" => {
            ui.set_bot_view(3);
            ui.set_mx_open(true);
        }
        _ => {
            ui.set_bot_view(0);
            ui.set_mx_open(false);
        }
    }
    ui.on_experience_mode_changed(|beginner_mode| {
        store_beginner_mode(beginner_mode);
    });
    ui.on_onboarding_progress_changed(|completed, step| {
        store_onboarding_progress(completed, step.clamp(0, 6) as u8);
    });
    ui.on_focus_mode_changed(|focus_mode| {
        store_focus_mode(focus_mode);
    });
    ui.on_workspace_preset_changed(|preset| {
        store_workspace_preset(preset.as_str());
    });
    ui.on_plugin_favorite_changed(|plugin_id, favorite| {
        store_plugin_favorite(plugin_id.as_str(), favorite);
    });
    let core = match HirariCore::new() {
        Ok(core) => Rc::new(core),
        Err(error) => {
            // Initialization failures must remain visible in the UI instead
            // of terminating the process before the user can recover.
            ui.set_last_action(
                ui_error_message(
                    UiErrorKind::Engine,
                    &format!("initialization failed: {error}"),
                )
                .into(),
            );
            if let Err(error) = ui.run() {
                eprintln!("Hirari UI could not enter its event loop: {error}");
            }
            return;
        }
    };
    let configured_moufu_address = std::env::var("HIRARI_MOUFU_ADDR")
        .ok()
        .or_else(crate::slint_ui::load_moufu_address)
        .unwrap_or_default();
    let moufu_publisher = crate::moufu::MoufuPublisher::start_from_env();
    if let Some(publisher) = &moufu_publisher {
        let configured = configured_moufu_address.trim();
        let _ = publisher.set_address((!configured.is_empty()).then_some(configured));
        ui.global::<MoufuActions>().set_address(configured.into());
        ui.global::<MoufuActions>()
            .set_status(if configured.is_empty() {
                "MOUFU NOT CONFIGURED".into()
            } else {
                "MOUFU CONNECTING".into()
            });
        let weak = ui.as_weak();
        let publisher_for_configure = publisher.clone();
        ui.global::<MoufuActions>().on_configure(move |address| {
            let address = address.trim().to_owned();
            if address.len() > 1024 {
                if let Some(ui) = weak.upgrade() {
                    ui.set_last_action("MOUFU ADDRESS IS TOO LONG".into());
                }
                return;
            }
            let should_connect = !address.is_empty();
            let configured = should_connect.then_some(address.as_str());
            if let Err(error) = publisher_for_configure.set_address(configured) {
                if let Some(ui) = weak.upgrade() {
                    ui.set_last_action(format!("MOUFU CONFIGURATION FAILED: {error}").into());
                }
                return;
            }
            let settings_error = crate::slint_ui::save_moufu_address(&address).err();
            if let Some(ui) = weak.upgrade() {
                ui.global::<MoufuActions>().set_address(address.into());
                ui.global::<MoufuActions>().set_status(if should_connect {
                    "MOUFU CONNECTING".into()
                } else {
                    "MOUFU NOT CONFIGURED".into()
                });
                ui.set_last_action(if let Some(error) = settings_error {
                    format!("MOUFU CONFIGURED BUT COULD NOT SAVE SETTINGS: {error}").into()
                } else if should_connect {
                    "MOUFU CONNECTION REQUESTED".into()
                } else {
                    "MOUFU DISCONNECTED".into()
                });
            }
        });
        let weak = ui.as_weak();
        let publisher_for_refresh = publisher.clone();
        ui.global::<MoufuActions>().on_refresh(move || {
            let entities = publisher_for_refresh.list_entities();
            let links = publisher_for_refresh.list_links();
            if let Err(error) = entities.and(links) {
                if let Some(ui) = weak.upgrade() {
                    ui.set_last_action(format!("MOUFU REFRESH FAILED: {error}").into());
                }
            }
        });
        let weak = ui.as_weak();
        let publisher_for_share = publisher.clone();
        ui.global::<MoufuActions>().on_share(move |target| {
            if let Err(error) = publisher_for_share
                .create_link(crate::moufu::HIRARI_PROJECT_ENTITY, target.as_str())
            {
                if let Some(ui) = weak.upgrade() {
                    ui.set_last_action(format!("MOUFU SHARE FAILED: {error}").into());
                }
            } else if let Some(ui) = weak.upgrade() {
                ui.set_last_action("MOUFU SHARE LINK REQUESTED".into());
            }
        });
        let weak = ui.as_weak();
        let publisher_for_receive = publisher.clone();
        ui.global::<MoufuActions>().on_receive(move |source| {
            if let Err(error) = publisher_for_receive
                .create_link(source.as_str(), crate::moufu::HIRARI_PROJECT_ENTITY)
            {
                if let Some(ui) = weak.upgrade() {
                    ui.set_last_action(format!("MOUFU RECEIVE FAILED: {error}").into());
                }
            } else if let Some(ui) = weak.upgrade() {
                ui.set_last_action("MOUFU RECEIVE LINK REQUESTED".into());
            }
        });
        let weak = ui.as_weak();
        let publisher_for_unlink = publisher.clone();
        ui.global::<MoufuActions>().on_unlink(move |link_id| {
            if let Err(error) = publisher_for_unlink.destroy_link(link_id.as_str()) {
                if let Some(ui) = weak.upgrade() {
                    ui.set_last_action(format!("MOUFU REMOVE LINK FAILED: {error}").into());
                }
            } else if let Some(ui) = weak.upgrade() {
                ui.set_last_action("MOUFU REMOVE LINK REQUESTED".into());
            }
        });
    } else {
        ui.global::<MoufuActions>()
            .set_status("MOUFU UNAVAILABLE IN THIS RUN MODE".into());
    }
    crate::ui::transport::install(&ui, core.clone());
    let orchestrator = Rc::new(CoreOrchestrator::new(core.clone()));
    let _last_preview_at = Arc::new(AtomicU64::new(0));
    let tone_test_started_ms = Arc::new(AtomicU64::new(0));
    let tone_test_baseline_callbacks = Arc::new(AtomicU64::new(0));
    let render_started_ms = Arc::new(AtomicU64::new(0));
    let render_output_path = Arc::new(Mutex::new(default_render_output_path()));
    let operation_gate = crate::ui::operation_gate::OperationGate::default();
    let project_save_queue = crate::ui::project_save_queue::ProjectSaveQueue::start();
    let dawproject_import_queue =
        crate::ui::dawproject_import_queue::DawProjectImportQueue::start();
    let render_lease = Arc::new(Mutex::new(None));
    let stem_batch = Arc::new(Mutex::new(None));

    // Start with no UI-authored content. The native project snapshot below is
    // the only source of track, region, and insert state shown to the user.
    let tracks_vec = Rc::new(slint::VecModel::from(Vec::<Z_Track>::new()));

    // Preview workflow: allow a project path as the first CLI argument so a
    // clean build can be smoke-tested without requiring a file-dialog plugin.
    let startup_project = std::env::args()
        .nth(1)
        .filter(|path| !path.trim().is_empty());
    let startup_recovery_path = if startup_project.is_none() {
        latest_session_recovery_path()
    } else {
        None
    };
    let session_recovery_path = Rc::new(RefCell::new(
        startup_recovery_path
            .clone()
            .or_else(new_session_recovery_path),
    ));
    ui.set_unsaved_recovery_available(startup_recovery_path.is_some());
    let mut loaded_startup_path = None;
    if let Some(path) = startup_project.as_deref() {
        if core.load_project(path) && hydrate_project_models(path, &tracks_vec, &core) {
            loaded_startup_path = Some(path.to_owned());
            // The startup project is also the default target for Save.
            // The shared path cell is initialized below before callbacks run.
            // A project supplied on the command line is already an explicit
            // session choice, so do not cover it with the New Project surface.
            ui.set_show_genesis(false);
        } else {
            core.new_project();
            crate::ui::track_model::reset_project_scoped_track_overlays(&tracks_vec);
            ui.set_last_action(format!("STARTUP PROJECT COULD NOT BE LOADED: {path}").into());
        }
    } else {
        // A new session owns a real starter track in Core. Never fabricate UI
        // rows that later disappear when telemetry catches up.
        core.new_project();
    }

    // Synchronize UI tracks with the actual engine state on startup.
    sync_tracks_from_engine(&tracks_vec, &core);
    if let Some(publisher) = &moufu_publisher {
        crate::moufu::publish_layout(publisher, &core, &tracks_vec, false);
    }
    ui.set_master_output_gain(core.master_gain());

    ui.set_tracks(slint::ModelRc::from(
        tracks_vec.clone() as Rc<dyn slint::Model<Data = Z_Track>>
    ));
    let markers_vec = Rc::new(slint::VecModel::from(core_markers(&core)));
    ui.set_markers(slint::ModelRc::from(
        markers_vec.clone() as Rc<dyn slint::Model<Data = Z_Marker>>
    ));

    let sample_catalog = Rc::new(RefCell::new(Vec::<Z_Sample_Entry>::new()));
    let samples_vec = Rc::new(slint::VecModel::from(sample_catalog.borrow().clone()));
    ui.set_sample_entries(slint::ModelRc::from(
        samples_vec.clone() as Rc<dyn slint::Model<Data = Z_Sample_Entry>>
    ));

    // --- 3. HARDENED ACTION BINDINGS ---
    let last_saved_project_path = Rc::new(RefCell::new(loaded_startup_path));
    let persisted_snapshot = Rc::new(std::cell::Cell::new(
        last_saved_project_path.borrow().as_ref().and_then(|_| {
            crate::ui::project_state::save_fingerprint(
                &core,
                &tracks_vec,
                ui.get_sequencer_patterns_json().as_str(),
            )
        }),
    ));
    let recording_target: Rc<RefCell<Option<Vec<(u32, Vec<u16>)>>>> = Rc::new(RefCell::new(None));
    let midi_recording_request: crate::ui::midi_input::SharedMidiRecordingRequest =
        Rc::new(RefCell::new(None));
    let peak_reset_generation = Rc::new(std::cell::Cell::new(0u64));
    crate::ui::project::install(
        &ui,
        core.clone(),
        tracks_vec.clone(),
        markers_vec.clone(),
        last_saved_project_path.clone(),
        persisted_snapshot.clone(),
        orchestrator.clone(),
        operation_gate.clone(),
        project_save_queue.clone(),
        moufu_publisher.clone(),
        session_recovery_path.clone(),
    );
    let _moufu_live_link_timer = moufu_publisher.as_ref().map(|publisher| {
        let timer = slint::Timer::default();
        let core = core.clone();
        let tracks = tracks_vec.clone();
        let publisher = publisher.clone();
        let ui_weak = ui.as_weak();
        let mut has_outbound_link = false;
        let mut outbound_link_ids = HashSet::<String>::new();
        let mut last_layout_revision = None;
        let mut remote_versions = HashMap::<String, (u64, bool)>::new();
        timer.start(
            slint::TimerMode::Repeated,
            std::time::Duration::from_millis(250),
            move || {
                if has_outbound_link {
                    let layout_revision = core.project_layout_revision();
                    if last_layout_revision != Some(layout_revision) {
                        last_layout_revision = Some(layout_revision);
                        crate::moufu::publish_layout(&publisher, &core, &tracks, true);
                    }
                }
                for event in publisher.drain_events() {
                    match &event {
                        crate::moufu::MoufuEvent::ConnectionConfigured { enabled } => {
                            has_outbound_link = false;
                            outbound_link_ids.clear();
                            last_layout_revision = None;
                            remote_versions.clear();
                            if let Some(ui) = ui_weak.upgrade() {
                                let empty = || {
                                    slint::ModelRc::new(slint::VecModel::from(Vec::<
                                        slint::SharedString,
                                    >::new(
                                    )))
                                };
                                ui.global::<MoufuActions>().set_entity_options(empty());
                                ui.global::<MoufuActions>().set_entity_ids(empty());
                                ui.global::<MoufuActions>().set_link_options(empty());
                                ui.global::<MoufuActions>().set_link_ids(empty());
                                ui.global::<MoufuActions>().set_selected_entity_index(-1);
                                ui.global::<MoufuActions>().set_selected_link_index(-1);
                                ui.global::<MoufuActions>().set_remote_source("".into());
                                ui.global::<MoufuActions>()
                                    .set_remote_preview("No linked project state received.".into());
                                ui.global::<MoufuActions>().set_status(if *enabled {
                                    "MOUFU CONNECTING".into()
                                } else {
                                    "MOUFU NOT CONFIGURED".into()
                                });
                            }
                        }
                        crate::moufu::MoufuEvent::Connected { server_version } => {
                            log::info!("Connected to Moufu server {server_version}");
                            if let Some(ui) = ui_weak.upgrade() {
                                ui.global::<MoufuActions>()
                                    .set_status("MOUFU CONNECTED".into());
                            }
                            let _ = publisher.list_entities();
                            let _ = publisher.list_links();
                        }
                        crate::moufu::MoufuEvent::Disconnected { reason } => {
                            log::debug!("Moufu reconnect scheduled: {reason}");
                            has_outbound_link = false;
                            outbound_link_ids.clear();
                            last_layout_revision = None;
                            remote_versions.clear();
                            if let Some(ui) = ui_weak.upgrade() {
                                ui.global::<MoufuActions>()
                                    .set_status("MOUFU RECONNECTING".into());
                                ui.set_last_action(
                                    format!("MOUFU CONNECTION INTERRUPTED; RETRYING: {reason}")
                                        .into(),
                                );
                            }
                        }
                        crate::moufu::MoufuEvent::EntityList(entities) => {
                            let project_entities = entities
                                .iter()
                                .filter(|entity| {
                                    entity.data_type == crate::moufu::HIRARI_PROJECT_DATA_TYPE
                                        && entity.id.0 != crate::moufu::HIRARI_PROJECT_ENTITY
                                })
                                .collect::<Vec<_>>();
                            if let Some(ui) = ui_weak.upgrade() {
                                ui.global::<MoufuActions>().set_entity_options(
                                    slint::ModelRc::new(slint::VecModel::from(
                                        project_entities
                                            .iter()
                                            .map(|entity| {
                                                format!(
                                                    "{} · {} · {}",
                                                    entity.source_app, entity.name, entity.id.0
                                                )
                                                .into()
                                            })
                                            .collect::<Vec<slint::SharedString>>(),
                                    )),
                                );
                                ui.global::<MoufuActions>()
                                    .set_entity_ids(slint::ModelRc::new(slint::VecModel::from(
                                        project_entities
                                            .iter()
                                            .map(|entity| entity.id.0.clone().into())
                                            .collect::<Vec<slint::SharedString>>(),
                                    )));
                                ui.global::<MoufuActions>().set_selected_entity_index(
                                    if project_entities.is_empty() { -1 } else { 0 },
                                );
                                ui.global::<MoufuActions>().set_status(
                                    format!(
                                        "MOUFU CONNECTED · {} PROJECT ENTITIES",
                                        project_entities.len()
                                    )
                                    .into(),
                                );
                            }
                        }
                        crate::moufu::MoufuEvent::LinkList(links) => {
                            let next_outbound_link_ids = links
                                .iter()
                                .filter(|link| {
                                    link.source_entity.0 == crate::moufu::HIRARI_PROJECT_ENTITY
                                        && link.target_entity.0
                                            != crate::moufu::HIRARI_PROJECT_ENTITY
                                        && link.status == crate::moufu::LinkStatus::Active
                                })
                                .map(|link| link.link_id.to_string())
                                .collect::<HashSet<_>>();
                            let next_has_outbound_link = !next_outbound_link_ids.is_empty();
                            let has_new_outbound_link = next_outbound_link_ids
                                .iter()
                                .any(|link_id| !outbound_link_ids.contains(link_id));
                            if has_new_outbound_link {
                                // A newly linked peer needs the current state
                                // even when the project itself has not changed.
                                // Advancing the Hub entity version makes the
                                // existing snapshot acceptable to Moufu.
                                if !publisher.republish_latest_layout() {
                                    crate::moufu::publish_layout(&publisher, &core, &tracks, true);
                                }
                                last_layout_revision = Some(core.project_layout_revision());
                            }
                            if !next_has_outbound_link {
                                last_layout_revision = None;
                            }
                            has_outbound_link = next_has_outbound_link;
                            outbound_link_ids = next_outbound_link_ids;
                            let relevant = links
                                .iter()
                                .filter(|link| {
                                    link.source_entity.0 == crate::moufu::HIRARI_PROJECT_ENTITY
                                        || link.target_entity.0
                                            == crate::moufu::HIRARI_PROJECT_ENTITY
                                })
                                .collect::<Vec<_>>();
                            if let Some(ui) = ui_weak.upgrade() {
                                ui.global::<MoufuActions>()
                                    .set_link_options(slint::ModelRc::new(slint::VecModel::from(
                                        relevant
                                            .iter()
                                            .map(|link| {
                                                format!(
                                                    "{} → {} · {:?}",
                                                    link.source_entity.0,
                                                    link.target_entity.0,
                                                    link.status
                                                )
                                                .into()
                                            })
                                            .collect::<Vec<slint::SharedString>>(),
                                    )));
                                ui.global::<MoufuActions>()
                                    .set_link_ids(slint::ModelRc::new(slint::VecModel::from(
                                        relevant
                                            .iter()
                                            .map(|link| link.link_id.to_string().into())
                                            .collect::<Vec<slint::SharedString>>(),
                                    )));
                                ui.global::<MoufuActions>().set_selected_link_index(
                                    if relevant.is_empty() { -1 } else { 0 },
                                );
                                let inbound_sources = links
                                    .iter()
                                    .filter(|link| {
                                        link.target_entity.0 == crate::moufu::HIRARI_PROJECT_ENTITY
                                            && link.status == crate::moufu::LinkStatus::Active
                                    })
                                    .map(|link| link.source_entity.0.clone())
                                    .collect::<HashSet<_>>();
                                remote_versions
                                    .retain(|source, _| inbound_sources.contains(source));
                                let remote_source =
                                    ui.global::<MoufuActions>().get_remote_source().to_string();
                                if !remote_source.is_empty()
                                    && !inbound_sources.contains(&remote_source)
                                {
                                    ui.global::<MoufuActions>().set_remote_source("".into());
                                    ui.global::<MoufuActions>().set_remote_preview(
                                        "No active inbound project link.".into(),
                                    );
                                }
                                ui.global::<MoufuActions>().set_status(
                                    format!("MOUFU CONNECTED · {} PROJECT LINKS", relevant.len())
                                        .into(),
                                );
                            }
                        }
                        crate::moufu::MoufuEvent::EntityUpdated(change) => {
                            if !accept_moufu_version(
                                &mut remote_versions,
                                &change.entity_id.0,
                                change.version,
                                change.is_transient,
                            ) {
                                continue;
                            }
                            let (source, preview) = linked_project_preview(
                                &change.entity_id.0,
                                change.version,
                                change.is_transient,
                                &change.payload,
                            );
                            if let Some(ui) = ui_weak.upgrade() {
                                ui.global::<MoufuActions>().set_remote_source(source.into());
                                ui.global::<MoufuActions>()
                                    .set_remote_preview(preview.into());
                            }
                        }
                        crate::moufu::MoufuEvent::EntityState {
                            entity_id,
                            version,
                            payload,
                        } => {
                            if !accept_moufu_version(
                                &mut remote_versions,
                                &entity_id.0,
                                *version,
                                false,
                            ) {
                                continue;
                            }
                            let (source, preview) =
                                linked_project_preview(&entity_id.0, *version, false, payload);
                            if let Some(ui) = ui_weak.upgrade() {
                                ui.global::<MoufuActions>().set_remote_source(source.into());
                                ui.global::<MoufuActions>()
                                    .set_remote_preview(preview.into());
                            }
                        }
                        crate::moufu::MoufuEvent::LinkStatusChanged(_)
                        | crate::moufu::MoufuEvent::LinkDestroyed { .. } => {
                            let _ = publisher.list_links();
                        }
                        _ => {}
                    }
                    if let (Some(message), Some(ui)) = (event.status_message(), ui_weak.upgrade()) {
                        ui.set_last_action(message.into());
                    }
                    if let (crate::moufu::MoufuEvent::ServerNotice { code, message }, Some(ui)) =
                        (&event, ui_weak.upgrade())
                    {
                        ui.set_last_action(format!("MOUFU {code}: {message}").into());
                    }
                }
            },
        );
        timer
    });
    crate::ui::automation::install(&ui, core.clone(), tracks_vec.clone());
    crate::ui::tempo::install(&ui, core.clone());
    crate::ui::audio_settings::install(&ui, core.clone(), tracks_vec.clone());
    let _midi_input_timer = crate::ui::midi_input::install(
        &ui,
        core.clone(),
        tracks_vec.clone(),
        midi_recording_request.clone(),
    );
    crate::ui::recording::install(
        &ui,
        core.clone(),
        tracks_vec.clone(),
        last_saved_project_path.clone(),
        recording_target.clone(),
        midi_recording_request.clone(),
    );
    crate::ui::render::install(
        &ui,
        core.clone(),
        render_started_ms.clone(),
        render_output_path.clone(),
        operation_gate.clone(),
        render_lease.clone(),
        stem_batch.clone(),
    );
    crate::ui::mixer::install(
        &ui,
        core.clone(),
        tracks_vec.clone(),
        peak_reset_generation.clone(),
        moufu_publisher.clone(),
    );
    crate::ui::stack::install(&ui, core.clone());
    crate::ui::plugin::install(&ui, core.clone(), tracks_vec.clone());
    crate::ui::arrange::install(&ui, core.clone(), tracks_vec.clone());
    crate::ui::timeline::install(&ui, core.clone(), markers_vec.clone(), tracks_vec.clone());
    crate::ui::editor::install(&ui, core.clone(), tracks_vec.clone());
    crate::ui::midi::install(&ui, core.clone(), tracks_vec.clone());
    crate::ui::browser::install(
        &ui,
        core.clone(),
        samples_vec.clone(),
        sample_catalog.clone(),
    );
    crate::ui::misc::install(&ui, core.clone(), tracks_vec.clone());
    ui.set_project_save_status(if last_saved_project_path.borrow().is_some() {
        "Saved".into()
    } else {
        "Unsaved project".into()
    });

    crate::ui::command_router::install(
        &ui,
        core.clone(),
        tracks_vec.clone(),
        last_saved_project_path.clone(),
        persisted_snapshot.clone(),
        session_recovery_path.clone(),
        tone_test_started_ms.clone(),
        tone_test_baseline_callbacks.clone(),
        operation_gate.clone(),
        project_save_queue.clone(),
        dawproject_import_queue.clone(),
    );

    let gpu_plot_store = crate::ui::gpu_canvas::shared_store();
    #[cfg(feature = "slint-wgpu")]
    if std::env::var("HIRARI_DISABLE_GPU_CANVAS").as_deref() != Ok("1") {
        if let Err(error) = crate::ui::slint_wgpu::install(&ui, gpu_plot_store.clone()) {
            ui.set_last_action(format!("GPU canvas unavailable: {error}").into());
        }
    }
    crate::ui::telemetry_loop::install_telemetry_loop(
        &ui,
        core.clone(),
        tracks_vec.clone(),
        last_saved_project_path.clone(),
        persisted_snapshot.clone(),
        session_recovery_path,
        operation_gate.clone(),
        project_save_queue,
        dawproject_import_queue,
        markers_vec.clone(),
        tone_test_started_ms.clone(),
        tone_test_baseline_callbacks.clone(),
        render_started_ms.clone(),
        render_output_path.clone(),
        render_lease.clone(),
        stem_batch.clone(),
        recording_target,
        peak_reset_generation,
        gpu_plot_store,
    );

    // Opt-in main-thread smoke path for the real Slint -> Core callback.
    // AppKit requires the event-loop-backed UI to be created on the process
    // main thread, so this is intentionally exercised by the application
    // binary rather than a normal worker-thread unit test.
    #[cfg(debug_assertions)]
    if std::env::var("HIRARI_UI_SMOKE").as_deref() == Ok("plugin-core") {
        ui.global::<AudioSettingsActions>()
            .invoke_apply_config(48_000, 256);
        let audio_config_action = ui.get_last_action().to_string();
        let track_id = core.add_track(0);
        let applied = core.add_plugin(track_id, 0);
        if applied {
            ui.global::<PluginActions>()
                .invoke_plugin_param_changed(track_id as i32, 0, 0, 50.0);
        }
        println!(
            "HIRARI_UI_SMOKE audio_config={} plugin_core={} action={}",
            audio_config_action,
            applied,
            ui.get_last_action()
        );
        return;
    }

    #[cfg(debug_assertions)]
    if std::env::var("HIRARI_UI_SMOKE").as_deref() == Ok("production-workflow") {
        if !crate::ui::smoke::run_production_workflow(&ui, &core) {
            std::process::exit(1);
        }
        return;
    }

    // Regression smoke for the template transition exercised by the desktop
    // app.  The initial peak models are intentionally empty here, matching a
    // freshly opened window; template loading must not render an out-of-range
    // peak lookup while the telemetry loop catches up.
    #[cfg(debug_assertions)]
    if std::env::var("HIRARI_UI_SMOKE").as_deref() == Ok("template-navigation") {
        ui.global::<CommandActions>()
            .invoke_palette_exec("INIT_ELECTRONIC".into());
        let track_count = ui.get_tracks().row_count();
        let last_action = ui.get_last_action().to_string();
        let visible_project = !ui.get_show_genesis()
            && ui.get_workspace_preset().as_str() == "arrange"
            && ui.get_bot_view() == 0
            && ui.get_sel_idx() == 0
            && ui.get_project_surface_ready();
        if track_count != 5
            || !last_action.contains("TEMPLATE READY: ELECTRONIC")
            || !visible_project
        {
            eprintln!(
                "HIRARI_UI_SMOKE template_navigation failed tracks={} action={} genesis={} workspace={} bot_view={} sel_idx={}",
                track_count,
                last_action,
                ui.get_show_genesis(),
                ui.get_workspace_preset(),
                ui.get_bot_view(),
                ui.get_sel_idx(),
            );
            std::process::exit(1);
        }
        println!(
            "HIRARI_UI_SMOKE template_navigation tracks={} action={}",
            track_count, last_action
        );
        return;
    }

    // Release-bundle health check. This deliberately avoids the debug-only
    // smoke paths and proves that the packaged UI, resources, and Core bridge
    // can initialize and shut down without entering the event loop.
    if std::env::var("HIRARI_HEADLESS").as_deref() == Ok("1") {
        let layout_valid =
            serde_json::from_str::<serde_json::Value>(&core.get_project_layout_json())
                .is_ok_and(|value| value.is_array());
        let bundle_root = std::env::current_exe()
            .ok()
            .and_then(|path| path.parent().map(std::path::Path::to_path_buf))
            .and_then(|macos| macos.parent().map(std::path::Path::to_path_buf));
        let resources_present = bundle_root.as_ref().is_some_and(|root| {
            root.join("Resources").is_dir()
                && root.join("Resources/hirari-resources.manifest").is_file()
        });
        let health = core.runtime_health_snapshot();
        let audio_generation = core.audio_config_generation();
        let sample_rate = core.get_sample_rate();
        let native_engine_ready = audio_generation > 0
            && sample_rate.is_finite()
            && (8_000.0..=384_000.0).contains(&sample_rate);
        let strict_audio = std::env::var("HIRARI_REQUIRE_HARDWARE_DEVICE").as_deref() == Ok("1");
        if !layout_valid
            || !resources_present
            || !native_engine_ready
            || (strict_audio && !health.audio_device_ready)
        {
            eprintln!(
                "HIRARI_HEADLESS_INIT_FAILED layout_valid={} resources_present={} native_engine_ready={} audio_generation={} sample_rate={} audio_device_ready={} audio_driver={} strict_audio={}",
                layout_valid, resources_present, native_engine_ready, audio_generation,
                sample_rate, health.audio_device_ready, health.audio_driver_status, strict_audio
            );
            std::process::exit(1);
        }
        println!(
            "HIRARI_HEADLESS_READY native_engine=ready bridge=ready project_layout=valid resources=ready audio_device_ready={} audio_driver={} audio_generation={} sample_rate={} runtime_health={}",
            health.audio_device_ready,
            health.audio_driver_status,
            audio_generation,
            sample_rate,
            health.status_text()
        );
        return;
    }

    if let Err(error) = ui.run() {
        eprintln!("Hirari UI event loop failed: {error}");
    }
    if let Some(publisher) = &moufu_publisher {
        publisher.shutdown();
    }
}
