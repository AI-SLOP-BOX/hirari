//! View cards for the AmeUI shell.
use crate::ameui::*;
impl HirariAmeApp {
    pub(crate) fn diagnostics_card_view(&self) -> View {
        // Diagnostics card (diagnostics page parity): engine, audio
        // drops/overruns, plugin sandbox health and range overflow.
        // Aboutattribution is intentionally not ported.
        let diagnostics_card = if let Some(core) = &self.core {
            let health = core.runtime_health_snapshot();
            let state = core.runtime_health_status_text();
            let overruns: u32 = core
                .sandbox_snapshots()
                .iter()
                .map(|snapshot| snapshot.mailbox_overruns)
                .sum();
            let drops = core.get_dropped_input_blocks();
            let failures = health.sandbox_failures;
            let watchdog = self.watchdog_trips.get();
            let recover_core = self.core.clone();
            let recover_status = self.status.clone();
            let recover_button = ButtonView::new("RECOVER")
                .action_id("diagnostics.recover")
                .secondary()
                .on_press(move |_| {
                    if let Some(core) = &recover_core {
                        let recovered = core.recover_sandboxed_plugins();
                        recover_status.set(format!(
                            "RECOVERY REQUESTED · {recovered} PLUGIN(S) RECOVERED"
                        ));
                    }
                });
            let cpu = health.dsp_load.clamp(0.0, 4.0) * 25.0;
            let range_label = if health.audio_range_overflow {
                "RANGE OVERFLOW"
            } else {
                "RANGE OK"
            };
            let mut rows = vec![
                View::Text(format!("SYSTEM · {state}")),
                View::HStack(vec![
                    View::Caption(format!("CPU {cpu:.0}%")),
                    View::Caption(format!("DROP {drops}")),
                    View::Caption(format!("OVERRUN {overruns}")),
                    View::Caption(format!("FAIL {failures}")),
                    View::Caption(format!("WATCHDOG {watchdog}")),
                    View::Caption(range_label.to_owned()),
                    recover_button.into(),
                ]),
            ];
            if health.audio_silent_fallback
                || matches!(health.audio_driver_status, "start-failed" | "unavailable")
            {
                rows.push(View::Caption(
                    "No audio path — check device, sample rate, buffer, cable.".to_owned(),
                ));
            }
            View::Card(rows)
        } else {
            View::Card(vec![View::Text("SYSTEM · no engine".to_owned())])
        };

        diagnostics_card
    }
}
