use hirari_core_bridge::HirariCore;
use slint::{ComponentHandle, Model, VecModel};
use std::cell::RefCell;
use std::process::Command;
use std::rc::Rc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use crate::slint_ui::{
    clamp_selection_index, clear_plugin_blacklist, load_plugin_favorites, sync_tracks_from_engine,
    ui_error_message, ui_error_with_action, AppWindow, PluginActions, UiErrorKind, Z_Plugin_Entry,
    Z_Track,
};

thread_local! {
    /// The engine handle owned by the UI thread.
    ///
    /// HirariCore holds a cxx::UniquePtr and is deliberately not `Send`, so it
    /// cannot be captured by a closure handed to `invoke_from_event_loop`. The
    /// closures below run *on* the UI thread, so a thread-local gives them the
    /// engine without an unsafe raw pointer or an unjustified `Send` impl.
    static THREAD_CORE: RefCell<Option<Rc<HirariCore>>> = const { RefCell::new(None) };
}

/// Publish the engine handle for deferred work that resumes on the UI thread.
/// Must be called from the UI thread during startup.
pub fn register_thread_core(core: Rc<HirariCore>) {
    THREAD_CORE.with(|slot| *slot.borrow_mut() = Some(core));
}

/// Run `action` with the UI thread's engine, or `None` if it is unavailable
/// (for example when the callback is invoked after shutdown).
pub fn with_thread_core<T>(action: impl FnOnce(&HirariCore) -> T) -> Option<T> {
    THREAD_CORE.with(|slot| slot.borrow().as_ref().map(|core| action(core)))
}

/// Monotonic token identifying the most recent tuning request.
///
/// A worker finishes long after the user may have started another request or
/// closed the window. Only the newest generation is allowed to publish a
/// result, so a slow, superseded subprocess cannot overwrite fresher UI state.
static TUNING_GENERATION: AtomicU64 = AtomicU64::new(0);

/// Result handed back from the tuning worker thread to the UI thread.
#[derive(Clone)]
struct TuningResult {
    generation: u64,
    succeeded: bool,
    output: String,
    scoop: f32,
    vibrato: f32,
    dynamics: f32,
    consonants: f32,
}

/// Shared, thread-safe landing slot for the worker result.
type TuningSlot = Arc<Mutex<Option<TuningResult>>>;

/// The slot the most recent worker will publish into, so the UI-thread
/// collector can reach it from the event loop.
static TUNING_PENDING: Mutex<Option<TuningSlot>> = Mutex::new(None);

/// Consume a finished tuning result on the UI thread.
///
/// Runs inside `invoke_from_event_loop`, so the engine is reached through the
/// thread-local rather than being captured across threads. Results from a
/// superseded generation are discarded, and a closed window upgrades to None so
/// nothing touches a dead UI.
fn collect_tuning_result(weak: &slint::Weak<AppWindow>) {
    let slot = TUNING_PENDING
        .lock()
        .ok()
        .and_then(|mut pending| pending.take());
    let Some(slot) = slot else {
        return;
    };
    let result = slot.lock().ok().and_then(|mut pending| pending.take());
    let Some(result) = result else {
        return;
    };
    if result.generation != TUNING_GENERATION.load(Ordering::Acquire) {
        // A newer request superseded this one; drop the stale result.
        return;
    }
    let Some(ui) = weak.upgrade() else {
        return;
    };
    if !result.succeeded {
        ui.set_openutau_status("TUNING FAILED · CHECK PYTHON / USTX".into());
        return;
    }
    let output = std::path::PathBuf::from(&result.output);
    let source = ui.get_openutau_score_path().to_string();
    // HirariCore is not Send; it is reached through the UI-thread handle.
    if with_thread_core(|core| {
        core.set_openutau_tuning(
            &source,
            &result.output,
            result.scoop,
            result.vibrato,
            result.dynamics,
            result.consonants,
        )
    })
    .is_none()
    {
        ui.set_openutau_status("TUNING FAILED · ENGINE UNAVAILABLE".into());
        return;
    }
    ui.set_openutau_score_path(result.output.as_str().into());
    set_openutau_note_preview(&ui, &output);
    ui.set_openutau_status("HIRARI TUNING APPLIED · PITCH / VIBRATO / DYNAMICS READY".into());
}

fn launch_openutau(source: &std::path::Path) -> Result<(), String> {
    hirari_core_bridge::openutau::validate_source(&source.to_string_lossy())
        .map_err(|error| format!("INVALID SCORE · {error}"))?;
    let app = hirari_core_bridge::openutau::application_path();
    if !app.is_dir() {
        return Err(format!("OPENUTAU NOT FOUND · {}", app.display()));
    }
    Command::new("open")
        .arg("-a")
        .arg(&app)
        .arg(source)
        .spawn()
        .map(|_| ())
        .map_err(|error| format!("OPENUTAU LAUNCH FAILED · {error}"))
}

fn set_openutau_note_preview(ui: &AppWindow, source: &std::path::Path) {
    let notes = hirari_core_bridge::openutau::note_preview(&source.to_string_lossy());
    ui.set_openutau_notes(slint::ModelRc::new(VecModel::from(
        notes
            .into_iter()
            .map(Into::into)
            .collect::<Vec<slint::SharedString>>(),
    )));
}

pub fn apply_plugin_catalog_json_to_ui(catalog_json: &str, ui: &AppWindow) -> usize {
    let favorites = load_plugin_favorites();
    let value = serde_json::from_str::<serde_json::Value>(catalog_json).unwrap_or_default();
    let mut entries = vec![
        Z_Plugin_Entry {
            id: "hirari.internal.limiter".into(),
            name: "Hirari Limiter".into(),
            format: "INTERNAL".into(),
            path: "Hirari/Limiter".into(),
            capability: "native realtime dynamics".into(),
            binary_hash: "builtin".into(),
            binary_bytes: 0,
            installed: true,
            sandboxed: false,
            favorite: favorites.contains("hirari.internal.limiter"),
            visible: true,
        },
        Z_Plugin_Entry {
            id: "hirari.internal.compressor".into(),
            name: "Hirari Compressor".into(),
            format: "INTERNAL".into(),
            path: "Hirari/Compressor".into(),
            capability: "native realtime dynamics · assistant-ready".into(),
            binary_hash: "builtin".into(),
            binary_bytes: 0,
            installed: true,
            sandboxed: false,
            favorite: favorites.contains("hirari.internal.compressor"),
            visible: true,
        },
    ];
    entries.extend(
        value
            .get("plugins")
            .and_then(serde_json::Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|plugin| {
                Some(Z_Plugin_Entry {
                    id: plugin.get("id")?.as_str()?.into(),
                    name: plugin.get("name")?.as_str()?.into(),
                    format: plugin.get("format")?.as_str()?.to_ascii_uppercase().into(),
                    path: plugin.get("path")?.as_str()?.into(),
                    capability: plugin.get("capability")?.as_str()?.into(),
                    binary_hash: plugin.get("binary_hash")?.as_str()?.into(),
                    binary_bytes: plugin.get("binary_bytes")?.as_u64()?.min(i32::MAX as u64) as i32,
                    installed: plugin.get("installed")?.as_bool()?,
                    sandboxed: plugin.get("sandboxed")?.as_bool()?,
                    favorite: favorites.contains(plugin.get("id")?.as_str()?),
                    visible: true,
                })
            })
            .collect::<Vec<_>>(),
    );
    let count = entries.len();
    ui.set_plugin_catalog(slint::ModelRc::new(slint::VecModel::from(entries)));
    crate::ui::plugin::filter_plugin_catalog(ui, ui.get_plugin_catalog_query().as_str());
    ui.set_plugin_catalog_status(format!("{} PLUGINS · CLAP/VST3/AU", count).into());
    count
}

pub fn insert_catalog_plugin(
    alias: &str,
    core: &HirariCore,
    ui: &AppWindow,
    tracks: &Rc<VecModel<Z_Track>>,
) {
    let selected = clamp_selection_index(ui.get_sel_idx(), tracks.row_count());
    let Some(track) = tracks.row_data(selected) else {
        ui.set_last_action("PLUGIN INSERT: NO TARGET TRACK".into());
        return;
    };
    let result = if alias == "hirari.internal.compressor" || alias == "Hirari/Compressor" {
        core.add_plugin_diagnostic_json(track.id.max(0) as u32, 1)
    } else if alias == "hirari.internal.limiter" || alias == "Hirari/Limiter" {
        core.add_plugin_diagnostic_json(track.id.max(0) as u32, 0)
    } else {
        core.add_named_sandboxed_plugin_diagnostic_json(track.id.max(0) as u32, alias)
    };
    let value = serde_json::from_str::<serde_json::Value>(&result).unwrap_or_default();
    let ok = value
        .get("result")
        .and_then(|v| v.get("ok"))
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false);
    if ok {
        sync_tracks_from_engine(tracks, core);
        ui.set_last_action(
            if alias.starts_with("hirari.internal.") || alias.starts_with("Hirari/") {
                "PLUGIN INSERTED · NATIVE DSP READY".into()
            } else {
                "PLUGIN INSERTED · SANDBOX READY".into()
            },
        );
    } else {
        let code = value
            .get("code")
            .and_then(serde_json::Value::as_str)
            .or_else(|| {
                value
                    .get("result")
                    .and_then(|v| v.get("code"))
                    .and_then(serde_json::Value::as_str)
            })
            .unwrap_or("plugin_host_unavailable");
        ui.set_last_action(format!("PLUGIN INSERT FAILED · {code}").into());
    }
}

fn copy_plugin_bundle(source: &std::path::Path, target: &std::path::Path) -> std::io::Result<()> {
    if source.is_dir() {
        std::fs::create_dir_all(target)?;
        for entry in std::fs::read_dir(source)? {
            let entry = entry?;
            copy_plugin_bundle(&entry.path(), &target.join(entry.file_name()))?;
        }
        Ok(())
    } else {
        std::fs::copy(source, target).map(|_| ())
    }
}

fn install_target_path(
    destination_dir: &std::path::Path,
    source: &std::path::Path,
) -> Option<(std::path::PathBuf, std::path::PathBuf)> {
    let name = source.file_name()?.to_owned();
    let target = destination_dir.join(&name);
    let nonce = format!(
        "{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .ok()?
            .as_nanos()
    );
    let temporary =
        destination_dir.join(format!(".hirari-plugin-{nonce}-{}", name.to_string_lossy()));
    Some((temporary, target))
}

fn install_selected_plugin(ui: &AppWindow) {
    let Some(source) = rfd::FileDialog::new()
        .add_filter("Hirari Plugin", &["clap", "vst3", "component"])
        .pick_file()
    else {
        ui.set_last_action("PLUGIN INSTALL CANCELLED".into());
        return;
    };
    let Some(format) = source
        .extension()
        .and_then(|value| value.to_str())
        .map(|value| value.to_ascii_lowercase())
    else {
        ui.set_last_action("PLUGIN INSTALL REJECTED · UNKNOWN FORMAT".into());
        return;
    };
    let Some(home) = std::env::var_os("HOME").map(std::path::PathBuf::from) else {
        ui.set_last_action("PLUGIN INSTALL FAILED · HOME DIRECTORY UNAVAILABLE".into());
        return;
    };
    let destination_dir = match format.as_str() {
        "clap" => home.join("Library/Audio/Plug-Ins/CLAP"),
        "vst3" => home.join("Library/Audio/Plug-Ins/VST3"),
        "component" => home.join("Library/Audio/Plug-Ins/Components"),
        _ => {
            ui.set_last_action("PLUGIN INSTALL REJECTED · UNSUPPORTED FORMAT".into());
            return;
        }
    };
    let Ok(metadata) = std::fs::symlink_metadata(&source) else {
        ui.set_last_action("PLUGIN INSTALL REJECTED · SOURCE NOT FOUND".into());
        return;
    };
    if metadata.file_type().is_symlink() || (!metadata.is_dir() && !metadata.is_file()) {
        ui.set_last_action("PLUGIN INSTALL REJECTED · SYMLINK OR INVALID SOURCE".into());
        return;
    }
    let Some((temporary, target)) = install_target_path(&destination_dir, &source) else {
        ui.set_last_action("PLUGIN INSTALL REJECTED · INVALID FILE NAME".into());
        return;
    };
    if target.exists() {
        ui.set_last_action("PLUGIN INSTALL REJECTED · EXISTING FILE NOT OVERWRITTEN".into());
        return;
    }
    // The recursive bundle copy and the final rename can take seconds for a
    // large plugin, which froze the window while it ran. Everything above is
    // cheap validation and stays on the UI thread; only the filesystem work
    // moves to a worker, which reports back through a Weak handle so a closed
    // window is simply ignored.
    let weak = ui.as_weak();
    ui.set_last_action("PLUGIN INSTALLING…".into());
    let worker_source = source.clone();
    let worker_temporary = temporary.clone();
    let worker_target = target.clone();
    let worker_destination = destination_dir.clone();
    let spawn = std::thread::Builder::new()
        .name("hirari-plugin-install".to_owned())
        .spawn(move || {
            let _permit = crate::ui::worker_budget::WorkerPermit::acquire();
            let outcome = std::fs::create_dir_all(&worker_destination)
                .and_then(|()| copy_plugin_bundle(&worker_source, &worker_temporary))
                .and_then(|()| std::fs::rename(&worker_temporary, &worker_target));
            let _ = slint::invoke_from_event_loop(move || {
                let Some(ui) = weak.upgrade() else {
                    return;
                };
                if let Err(error) = outcome {
                    let _ = std::fs::remove_dir_all(&worker_temporary);
                    let _ = std::fs::remove_file(&worker_temporary);
                    ui.set_last_action(format!("PLUGIN INSTALL FAILED · {error}").into());
                    return;
                }
                ui.global::<PluginActions>().invoke_rescan_plugins();
                ui.set_bot_view(16);
                ui.set_mx_open(true);
                ui.set_last_action("PLUGIN INSTALLED · REFRESHING CATALOG…".into());
            });
        });
    if spawn.is_err() {
        ui.set_last_action("PLUGIN INSTALL FAILED · COULD NOT START WORKER".into());
    }
}

pub fn handle_command(
    command: &str,
    core: &HirariCore,
    ui: &AppWindow,
    tracks: &Rc<VecModel<Z_Track>>,
) -> bool {
    if crate::ui::plugin::invoke_extension_from_palette(command, core, ui) {
        return true;
    }
    if let Some(path) = command.strip_prefix("INSERT PLUGIN PATH ") {
        let path = path.trim();
        if path.is_empty() {
            ui.set_last_action("PLUGIN INSERT FAILED · MISSING PATH".into());
            return true;
        }
        let selected = clamp_selection_index(ui.get_sel_idx(), tracks.row_count());
        let Some(track) = tracks.row_data(selected) else {
            ui.set_last_action("PLUGIN INSERT FAILED · NO TARGET TRACK".into());
            return true;
        };
        let result = core.add_sandboxed_plugin_diagnostic_json(track.id.max(0) as u32, path);
        let value = serde_json::from_str::<serde_json::Value>(&result).unwrap_or_default();
        let ok = value.get("ok").and_then(serde_json::Value::as_bool) == Some(true);
        if ok {
            sync_tracks_from_engine(tracks, core);
            ui.set_last_action("PLUGIN INSERTED · PATH ADMITTED BY SANDBOX".into());
        } else {
            let code = value
                .get("code")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("plugin_admission_failed");
            ui.set_last_action(format!("PLUGIN INSERT FAILED · {code}").into());
        }
        return true;
    }
    match command {
        command if command.strip_prefix("INSERT PLUGIN ").is_some() => {
            let alias = command
                .strip_prefix("INSERT PLUGIN ")
                .unwrap_or_default()
                .trim();
            if alias.is_empty() {
                ui.set_last_action("PLUGIN INSERT FAILED · MISSING ID".into());
            } else {
                insert_catalog_plugin(alias, core, ui, tracks);
            }
            true
        }
        "INSTALL PLUGIN" | "ADD PLUGIN FILE" => {
            install_selected_plugin(ui);
            true
        }
        "REMOVE ACTIVE PLUGIN" => {
            let selected = clamp_selection_index(ui.get_sel_idx(), tracks.row_count());
            let Some(track) = tracks.row_data(selected) else {
                ui.set_last_action(
                    ui_error_message(UiErrorKind::Plugin, "remove failed: no track selected")
                        .into(),
                );
                return true;
            };
            let plugin_index = ui.get_fx_active_id();
            if plugin_index < 0 {
                ui.set_last_action(
                    ui_error_message(UiErrorKind::Plugin, "remove failed: no plugin selected")
                        .into(),
                );
                return true;
            }
            let removed = core.remove_plugin(track.id.max(0) as u32, plugin_index as u32);
            if removed {
                sync_tracks_from_engine(tracks, core);
                ui.set_fx_active_id(-1);
                ui.set_last_action(format!("REMOVED PLUGIN {}", plugin_index).into());
            } else {
                ui.set_last_action(
                    ui_error_message(UiErrorKind::Plugin, "plugin removal rejected by Core").into(),
                );
            }
            true
        }
        "CLEAR PLUGIN BLACKLIST" | "RESET PLUGIN BLACKLIST" => {
            ui.set_last_action(if clear_plugin_blacklist() {
                "PLUGIN BLACKLIST CLEARED · RESCAN READY".into()
            } else {
                ui_error_message(UiErrorKind::Plugin, "could not clear plugin blacklist").into()
            });
            true
        }
        "RETRY PLUGINS" | "RECOVER PLUGINS" => {
            let recovered = core.recover_sandboxed_plugins();
            let remaining = core
                .sandbox_statuses()
                .iter()
                .filter(|status| !status.alive || status.failure != 0 || status.is_quarantined())
                .count();
            sync_tracks_from_engine(tracks, core);
            ui.set_last_action(
                if remaining == 0 {
                    format!("PLUGIN SANDBOX: {} RECOVERED", recovered)
                } else {
                    format!(
                        "PLUGIN SANDBOX: {} RECOVERED · {} FAILURE(S) REMAIN",
                        recovered, remaining
                    )
                }
                .into(),
            );
            true
        }
        "scan_plugins" | "SCAN PLUGINS" => {
            ui.set_bot_view(16);
            ui.set_mx_open(true);
            ui.global::<PluginActions>().invoke_rescan_plugins();
            ui.set_pr_open(false);
            true
        }
        "LOAD VITAL" | "INSERT VITAL" | "LOAD SURGE XT" | "INSERT SURGE XT" => {
            let alias = if command.contains("VITAL") {
                "Vital"
            } else {
                "Surge XT"
            };
            let selected = clamp_selection_index(ui.get_sel_idx(), tracks.row_count());
            let Some(track) = tracks.row_data(selected) else {
                ui.set_last_action(format!("{alias}: NO TARGET TRACK").into());
                return true;
            };
            let result =
                core.add_named_sandboxed_plugin_diagnostic_json(track.id.max(0) as u32, alias);
            let ok = serde_json::from_str::<serde_json::Value>(&result)
                .ok()
                .and_then(|value| value.get("result").cloned().or(Some(value)))
                .and_then(|value| value.get("ok").cloned())
                .and_then(|value| value.as_bool())
                == Some(true);
            if ok {
                sync_tracks_from_engine(tracks, core);
                ui.set_last_action(format!("{alias}: LOADED IN HIRARI SANDBOX").into());
            } else {
                ui.set_last_action(format!("{alias}: LOAD FAILED · {result}").into());
            }
            true
        }
        "OPENUTAU IMPORT" | "IMPORT OPENUTAU" => {
            let Some(source) = rfd::FileDialog::new()
                .add_filter("OpenUtau score", &["ustx", "ust"])
                .pick_file()
            else {
                ui.set_last_action("OPENUTAU IMPORT CANCELLED".into());
                return true;
            };
            let Some(render) = rfd::FileDialog::new()
                .add_filter("Rendered vocal", &["wav", "aif", "aiff"])
                .pick_file()
            else {
                ui.set_last_action("OPENUTAU IMPORT CANCELLED: RENDER IS REQUIRED".into());
                return true;
            };
            let diagnostic = core.openutau_import_diagnostic_json(
                &source.to_string_lossy(),
                &render.to_string_lossy(),
            );
            let parsed = serde_json::from_str::<serde_json::Value>(&diagnostic).ok();
            if parsed
                .as_ref()
                .and_then(|value| value.get("ok"))
                .and_then(serde_json::Value::as_bool)
                != Some(true)
            {
                ui.set_last_action(format!("OPENUTAU IMPORT REJECTED: {diagnostic}").into());
                return true;
            }
            let vocal_track_id = core.add_track(4);
            if vocal_track_id == 0
                || !core.add_region(vocal_track_id, &render.to_string_lossy(), 0.0)
            {
                ui.set_last_action("OPENUTAU IMPORT FAILED: COULD NOT CREATE VOCAL REGION".into());
                return true;
            }
            sync_tracks_from_engine(tracks, core);
            ui.set_last_action(
                format!(
                    "OPENUTAU VOCAL IMPORTED: {} · SOURCE KEPT: {}",
                    render
                        .file_name()
                        .and_then(|name| name.to_str())
                        .unwrap_or("render"),
                    source
                        .file_name()
                        .and_then(|name| name.to_str())
                        .unwrap_or("score"),
                )
                .into(),
            );
            true
        }
        "OPENUTAU EDIT" | "OPENUTAU SCORE" => {
            let Some(source) = rfd::FileDialog::new()
                .add_filter("OpenUtau score", &["ustx", "ust"])
                .pick_file()
            else {
                ui.set_last_action("OPENUTAU EDIT CANCELLED".into());
                return true;
            };
            ui.set_openutau_score_path(source.to_string_lossy().into_owned().into());
            set_openutau_note_preview(ui, &source);
            ui.set_openutau_status("OPENUTAU SCORE LOADED · READY TO EDIT".into());
            ui.set_bot_view(26);
            ui.set_mx_open(true);
            ui.set_pr_open(false);
            true
        }
        "OPENUTAU LAUNCH" => {
            let source = ui.get_openutau_score_path().to_string();
            if source.trim().is_empty() {
                ui.set_openutau_status("OPEN A USTX/UST SCORE FIRST".into());
                return true;
            }
            ui.set_openutau_status(
                launch_openutau(std::path::Path::new(&source))
                    .map(|()| "OPENUTAU EDITOR LAUNCHED · EDIT THEN RENDER".to_owned())
                    .unwrap_or_else(|error| error)
                    .into(),
            );
            true
        }
        "OPENUTAU RENDER" => {
            let source = ui.get_openutau_score_path().to_string();
            if source.trim().is_empty() {
                ui.set_openutau_status("OPEN A USTX/UST SCORE FIRST".into());
                return true;
            }
            ui.set_openutau_status(
                launch_openutau(std::path::Path::new(&source))
                    .map(|()| "OPENUTAU READY · EXPORT WAV/AIFF, THEN IMPORT".to_owned())
                    .unwrap_or_else(|error| error)
                    .into(),
            );
            true
        }
        "OPENUTAU APPLY TUNING" => {
            let source = ui.get_openutau_score_path().to_string();
            if source.trim().is_empty() {
                ui.set_openutau_status("OPEN A USTX/UST SCORE FIRST".into());
                return true;
            }
            let source_path = std::path::Path::new(&source);
            let Some(stem) = source_path.file_stem().and_then(|v| v.to_str()) else {
                ui.set_openutau_status("TUNING FAILED · INVALID SCORE PATH".into());
                return true;
            };
            let output = source_path.with_file_name(format!("{stem}_hirari_tuned.ustx"));
            let script = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../scripts/tune_openutau_chorus.py");

            // Read every slider once, on the UI thread, so the worker needs no
            // access to the UI and the values cannot drift mid-run.
            let scoop = ui.get_openutau_scoop();
            let vibrato = ui.get_openutau_vibrato();
            let dynamics = ui.get_openutau_dynamics();
            let consonants = ui.get_openutau_consonants();
            let vibrato_rate = ui.get_openutau_vibrato_rate();

            ui.set_openutau_status("APPLYING OPENUTAU TUNING…".into());

            // Invalidate any in-flight request; only the newest generation may
            // publish a result.
            let generation = TUNING_GENERATION.fetch_add(1, Ordering::AcqRel) + 1;
            let slot: TuningSlot = Arc::new(Mutex::new(None));
            let worker_slot = Arc::clone(&slot);
            let worker_source = source.clone();
            let worker_output = output.to_string_lossy().into_owned();
            let worker_generation = generation;
            let weak = ui.as_weak();
            if let Ok(mut pending) = TUNING_PENDING.lock() {
                *pending = Some(Arc::clone(&slot));
            }

            // Running python3 synchronously froze the whole window until the
            // interpreter exited, so the subprocess now runs on a worker and
            // only the cheap result hand-off touches the UI thread.
            let spawn = std::thread::Builder::new()
                .name("hirari-openutau-tuning".to_owned())
                .spawn(move || {
                    let _permit = crate::ui::worker_budget::WorkerPermit::acquire();
                    let status = Command::new("python3")
                        .arg(&script)
                        .arg(&worker_source)
                        .arg(&worker_output)
                        .arg(format!("{:.2}", scoop * 16.0))
                        .arg(format!("{:.2}", vibrato * 18.0 + 4.0))
                        .arg(format!("{:.2}", vibrato * 34.0 + 16.0))
                        .arg(format!("{:.2}", dynamics))
                        .arg(format!("{:.2}", consonants))
                        .arg(format!("{:.2}", 220.0 - vibrato_rate * 160.0))
                        .output();
                    let succeeded = status.as_ref().is_ok_and(|value| value.status.success());
                    let result = TuningResult {
                        generation: worker_generation,
                        succeeded,
                        output: worker_output,
                        scoop,
                        vibrato,
                        dynamics,
                        consonants,
                    };
                    if let Ok(mut pending) = worker_slot.lock() {
                        *pending = Some(result);
                    }
                    drop(worker_slot);
                    // Wake the event loop so the UI thread can collect the slot.
                    let _ = slint::invoke_from_event_loop(move || {
                        collect_tuning_result(&weak);
                    });
                });

            if spawn.is_err() {
                ui.set_openutau_status("TUNING FAILED · COULD NOT START WORKER".into());
                return true;
            }
            return true;
        }
        "OPENUTAU IMPORT RENDER" => {
            let source = ui.get_openutau_score_path().to_string();
            let render = ui.get_openutau_render_path().to_string();
            let diagnostic = core.openutau_import_diagnostic_json(&source, &render);
            let ok = serde_json::from_str::<serde_json::Value>(&diagnostic)
                .ok()
                .and_then(|value| value.get("ok").and_then(|v| v.as_bool()))
                == Some(true);
            if !ok {
                ui.set_openutau_status(format!("IMPORT REJECTED · {diagnostic}").into());
                return true;
            }
            let vocal_track_id = core.add_track(4);
            if vocal_track_id == 0 || !core.add_region(vocal_track_id, &render, 0.0) {
                ui.set_openutau_status("IMPORT FAILED · VOCAL TRACK NOT CREATED".into());
                return true;
            }
            sync_tracks_from_engine(tracks, core);
            ui.set_openutau_status("VOCAL RENDER IMPORTED INTO HIRARI".into());
            ui.set_last_action("OPENUTAU VOCAL TRACK UPDATED".into());
            true
        }
        "ADD LIMITER" => {
            let selected = clamp_selection_index(ui.get_sel_idx(), tracks.row_count());
            let Some(track) = tracks.row_data(selected) else {
                ui.set_last_action(
                    ui_error_message(UiErrorKind::Plugin, "add limiter failed: no track").into(),
                );
                return true;
            };
            let track_id = track.id.max(0) as u32;
            if !core.add_plugin(track_id, 0) {
                ui.set_last_action(
                    ui_error_with_action(
                        UiErrorKind::Plugin,
                        "リミッタープラグインを読み込めません",
                        "プラグインを再スキャンするか、プラグイン設定を確認してください",
                    )
                    .into(),
                );
                return true;
            }
            sync_tracks_from_engine(tracks, core);
            ui.set_last_action("ADDED: Hirari Limiter".into());
            true
        }
        _ => false,
    }
}
