//! Slint application bootstrap and callback wiring.

use crate::orchestrator::CoreOrchestrator;
use crate::slint_ui::*;
use aura_core_bridge::AuraCore;
#[cfg(debug_assertions)]
use slint::Model;
use std::cell::RefCell;
use std::rc::Rc;
use std::sync::atomic::AtomicU64;
use std::sync::{Arc, Mutex};

fn core_markers(core: &AuraCore) -> Vec<Z_Marker> {
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

pub fn run() {
    let ui = AppWindow::new().unwrap();
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
    let core = match AuraCore::new() {
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
            ui.run().unwrap();
            return;
        }
    };
    crate::ui::transport::install(&ui, core.clone());
    let orchestrator = Rc::new(CoreOrchestrator::new(core.clone()));
    let _last_preview_at = Arc::new(AtomicU64::new(0));
    let tone_test_started_ms = Arc::new(AtomicU64::new(0));
    let tone_test_baseline_callbacks = Arc::new(AtomicU64::new(0));
    let render_started_ms = Arc::new(AtomicU64::new(0));
    let render_output_path = Arc::new(Mutex::new(default_render_output_path()));
    let operation_gate = crate::ui::operation_gate::OperationGate::default();
    let render_lease = Arc::new(Mutex::new(None));

    // --- 1. INITIAL PROJECT STATE ---

    fn gen_fx() -> slint::ModelRc<Z_Fx> {
        slint::ModelRc::new(slint::VecModel::from(vec![
            Z_Fx {
                name: "AURA EQ".into(),
                active: true,
                has_ui: true,
            },
            Z_Fx {
                name: "DYN-REPAIR".into(),
                active: true,
                has_ui: true,
            },
            Z_Fx {
                name: "SATURATOR".into(),
                active: false,
                has_ui: true,
            },
        ]))
    }

    fn gen_auto_lanes() -> slint::ModelRc<Z_AutomationLane> {
        slint::ModelRc::new(slint::VecModel::from(vec![Z_AutomationLane {
            name: "Volume".into(),
            color: slint::Color::from_rgb_u8(255, 143, 0),
            active: true,
            points: slint::ModelRc::default(),
        }]))
    }

    let tracks_vec = Rc::new(slint::VecModel::from(vec![
        Z_Track {
            id: 1,
            name: "ORCHESTRA".into(),
            r#type: "FOLD".into(),
            color: slint::Color::from_rgb_u8(34, 197, 94),
            volume: 1.0,
            pan: 0.0,
            solo: false,
            mute: false,
            armed: false,
            expanded: true,
            show_automation: false,
            send_lvl: 0.0,
            is_stereo: true,
            phase_invert: false,
            auto_rw: 0,
            notes: "Main Orchestral Bus".into(),
            width: 1.0,
            delay_ms: 0.0,
            filter_lp: 1.0,
            filter_hp: 0.02,
            icon: "🏛️".into(),
            pan_law: "0dB".into(),
            midi_ch: 1,
            group_id: 0,
            input: "None".into(),
            output: "Main Out".into(),
            monitor: false,
            piano_roll_notes: slint::ModelRc::default(),
            fx: gen_fx(),
            clips: slint::ModelRc::default(),
            auto_lanes: slint::ModelRc::default(),
            is_folder: true,
            parent_id: 0,
            folded: false,
            panner_mode: 0,
            pan3d_x: 0.0,
            pan3d_y: 0.1,
            pan3d_z: 0.0,
            saturate_active: false,
            artic_map: "".into(),
            correlation: 0.0,
            frozen: false,
            frozen_sample_rate: 0,
        },
        Z_Track {
            id: 2,
            name: "VIOLINS I".into(),
            r#type: "AUDIO".into(),
            color: slint::Color::from_rgb_u8(255, 143, 0),
            volume: 0.85,
            pan: 0.0,
            solo: false,
            mute: false,
            armed: true,
            expanded: false,
            show_automation: true,
            send_lvl: 0.15,
            is_stereo: true,
            phase_invert: false,
            auto_rw: 1,
            notes: "Spitfire Symphonic Strings".into(),
            width: 0.6,
            delay_ms: 0.0,
            filter_lp: 1.0,
            filter_hp: 0.1,
            icon: "🎻".into(),
            pan_law: "-3dB".into(),
            midi_ch: 1,
            group_id: 1,
            input: "IN 1/2".into(),
            output: "ORCHESTRA".into(),
            monitor: true,
            piano_roll_notes: slint::ModelRc::default(),
            fx: gen_fx(),
            clips: slint::ModelRc::new(slint::VecModel::from(vec![Z_Clip {
                id: 201,
                name: "Legato_A".into(),
                start_beat: 0.0,
                length_beats: 16.0,
                color: slint::Color::from_rgb_u8(255, 143, 0),
                points: slint::ModelRc::default(),
                fade_in: 0.25,
                fade_out: 0.25,
                gain: 0.9,
                reverse: false,
                warp_ratio: 1.0,
                pitch_semitones: 0.0,
                loop_count: 1,
                trim_start: 0.0,
                trim_end: 1.0,
                layer: 0,
                selected: false,
                missing: false,
            }])),
            auto_lanes: gen_auto_lanes(),
            is_folder: false,
            parent_id: 1,
            folded: false,
            panner_mode: 1,
            pan3d_x: -0.5,
            pan3d_y: 0.8,
            pan3d_z: 0.2,
            saturate_active: false,
            artic_map: "Violins 1 Pro".into(),
            correlation: 0.0,
            frozen: false,
            frozen_sample_rate: 0,
        },
    ]));

    // Preview workflow: allow a project path as the first CLI argument so a
    // clean build can be smoke-tested without requiring a file-dialog plugin.
    let startup_project = std::env::args()
        .nth(1)
        .filter(|path| !path.trim().is_empty());
    if let Some(path) = startup_project.as_deref() {
        if core.load_project(path) && !core.get_project_layout_json().trim().is_empty() {
            // The startup project is also the default target for Save.
            // The shared path cell is initialized below before callbacks run.
            // A project supplied on the command line is already an explicit
            // session choice, so do not cover it with the New Project surface.
            ui.set_show_genesis(false);
        }
    }

    // Synchronize UI tracks with the actual engine state on startup.
    sync_tracks_from_engine(&tracks_vec, &core);
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
    let last_saved_project_path: Rc<RefCell<Option<String>>> = Rc::new(RefCell::new(
        startup_project.or_else(load_saved_project_path),
    ));
    crate::ui::project::install(
        &ui,
        core.clone(),
        tracks_vec.clone(),
        markers_vec.clone(),
        last_saved_project_path.clone(),
        orchestrator.clone(),
        operation_gate.clone(),
    );
    crate::ui::automation::install(&ui, core.clone(), tracks_vec.clone());
    crate::ui::tempo::install(&ui, core.clone());
    crate::ui::audio_settings::install(&ui, core.clone());
    crate::ui::recording::install(
        &ui,
        core.clone(),
        tracks_vec.clone(),
        last_saved_project_path.clone(),
    );
    crate::ui::render::install(
        &ui,
        core.clone(),
        render_started_ms.clone(),
        render_output_path.clone(),
        operation_gate.clone(),
        render_lease.clone(),
    );
    crate::ui::mixer::install(&ui, core.clone(), tracks_vec.clone());
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
    ui.set_auto_save_status(if last_saved_project_path.borrow().is_some() {
        "Auto-save: Ready".into()
    } else {
        "Auto-save: New Project".into()
    });

    crate::ui::command_router::install(
        &ui,
        core.clone(),
        tracks_vec.clone(),
        last_saved_project_path.clone(),
        tone_test_started_ms.clone(),
        tone_test_baseline_callbacks.clone(),
        render_started_ms.clone(),
    );

    crate::ui::telemetry_loop::install_telemetry_loop(
        &ui,
        core.clone(),
        tracks_vec.clone(),
        last_saved_project_path.clone(),
        tone_test_started_ms.clone(),
        tone_test_baseline_callbacks.clone(),
        render_started_ms.clone(),
        render_output_path.clone(),
        render_lease.clone(),
    );

    // Opt-in main-thread smoke path for the real Slint -> Core callback.
    // AppKit requires the event-loop-backed UI to be created on the process
    // main thread, so this is intentionally exercised by the application
    // binary rather than a normal worker-thread unit test.
    #[cfg(debug_assertions)]
    if std::env::var("AURA_UI_SMOKE").as_deref() == Ok("plugin-core") {
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
            "AURA_UI_SMOKE audio_config={} plugin_core={} action={}",
            audio_config_action,
            applied,
            ui.get_last_action()
        );
        return;
    }

    #[cfg(debug_assertions)]
    if std::env::var("AURA_UI_SMOKE").as_deref() == Ok("production-workflow") {
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
    if std::env::var("AURA_UI_SMOKE").as_deref() == Ok("template-navigation") {
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
                "AURA_UI_SMOKE template_navigation failed tracks={} action={} genesis={} workspace={} bot_view={} sel_idx={}",
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
            "AURA_UI_SMOKE template_navigation tracks={} action={}",
            track_count, last_action
        );
        return;
    }

    // Release-bundle health check. This deliberately avoids the debug-only
    // smoke paths and proves that the packaged UI, resources, and Core bridge
    // can initialize and shut down without entering the event loop.
    if std::env::var("AURA_HEADLESS").as_deref() == Ok("1") {
        let layout_valid =
            serde_json::from_str::<serde_json::Value>(&core.get_project_layout_json())
                .is_ok_and(|value| value.is_array());
        let bundle_root = std::env::current_exe()
            .ok()
            .and_then(|path| path.parent().map(std::path::Path::to_path_buf))
            .and_then(|macos| macos.parent().map(std::path::Path::to_path_buf));
        let resources_present = bundle_root.as_ref().is_some_and(|root| {
            root.join("Resources").is_dir()
                && root.join("Resources/aura-resources.manifest").is_file()
        });
        let health = core.runtime_health_snapshot();
        let audio_generation = core.audio_config_generation();
        let sample_rate = core.get_sample_rate();
        let native_engine_ready = audio_generation > 0
            && sample_rate.is_finite()
            && (8_000.0..=384_000.0).contains(&sample_rate);
        let strict_audio = std::env::var("AURA_REQUIRE_HARDWARE_DEVICE").as_deref() == Ok("1");
        if !layout_valid
            || !resources_present
            || !native_engine_ready
            || (strict_audio && !health.audio_device_ready)
        {
            eprintln!(
                "AURA_HEADLESS_INIT_FAILED layout_valid={} resources_present={} native_engine_ready={} audio_generation={} sample_rate={} audio_device_ready={} audio_driver={} strict_audio={}",
                layout_valid, resources_present, native_engine_ready, audio_generation,
                sample_rate, health.audio_device_ready, health.audio_driver_status, strict_audio
            );
            std::process::exit(1);
        }
        println!(
            "AURA_HEADLESS_READY native_engine=ready bridge=ready project_layout=valid resources=ready audio_device_ready={} audio_driver={} audio_generation={} sample_rate={} runtime_health={}",
            health.audio_device_ready,
            health.audio_driver_status,
            audio_generation,
            sample_rate,
            health.status_text()
        );
        return;
    }

    ui.run().unwrap();
}
