impl AuraCore {
    pub fn take_mix_snapshot_json(&self, name: &str, states_json: &str) -> String {
        self.take_mix_snapshot_with_plugins_json(name, states_json, "[]")
    }

    pub fn take_mix_snapshot_with_plugins_json(
        &self,
        name: &str,
        states_json: &str,
        plugins_json: &str,
    ) -> String {
        self.take_mix_snapshot_with_plugins_and_routing_json(name, states_json, plugins_json, "[]")
    }

    pub fn take_mix_snapshot_with_plugins_and_routing_json(
        &self,
        name: &str,
        states_json: &str,
        plugins_json: &str,
        routing_json: &str,
    ) -> String {
        let Ok(states) = serde_json::from_str::<std::collections::HashMap<u32, f32>>(states_json)
        else {
            return "{\"ok\":false,\"code\":\"invalid_snapshot_state\"}".into();
        };
        let Ok(plugins) =
            serde_json::from_str::<Vec<crate::snapshots::PluginSnapshotState>>(plugins_json)
        else {
            return "{\"ok\":false,\"code\":\"invalid_snapshot_plugins\"}".into();
        };
        let Ok(routing) = serde_json::from_str::<serde_json::Value>(routing_json) else {
            return "{\"ok\":false,\"code\":\"invalid_snapshot_routing\"}".into();
        };
        let Ok(mut snapshots) = self.mix_snapshots.lock() else {
            return "{\"ok\":false,\"code\":\"snapshot_lock_failed\"}".into();
        };
        let before = snapshots.snapshots.len();
        snapshots.take_snapshot_with_plugins_and_routing(
            name,
            states,
            plugins,
            routing.to_string(),
        );
        let accepted = snapshots.snapshots.len() >= before
            && snapshots
                .snapshots
                .iter()
                .any(|snapshot| snapshot.name == name);
        serde_json::json!({"ok": accepted, "operation": "take_mix_snapshot", "name": name, "count": snapshots.snapshots.len()}).to_string()
    }

    pub fn diff_mix_snapshots_json(&self, first: usize, second: usize) -> String {
        let Ok(snapshots) = self.mix_snapshots.lock() else {
            return "{\"ok\":false,\"code\":\"snapshot_lock_failed\"}".into();
        };
        if first >= snapshots.snapshots.len() || second >= snapshots.snapshots.len() {
            return "{\"ok\":false,\"code\":\"snapshot_not_found\"}".into();
        }
        serde_json::json!({"ok": true, "operation": "diff_mix_snapshots", "first": first, "second": second, "diff": snapshots.diff_snapshots(first, second)}).to_string()
    }

    pub fn recall_mix_snapshot_json(&self, index: usize) -> String {
        let Ok(snapshots) = self.mix_snapshots.lock() else {
            return "{\"ok\":false,\"code\":\"snapshot_lock_failed\"}".into();
        };
        let Some(snapshot) = snapshots.snapshots.get(index) else {
            return "{\"ok\":false,\"code\":\"snapshot_not_found\"}".into();
        };
        let routing = serde_json::from_str::<serde_json::Value>(&snapshot.routing_state)
            .unwrap_or_else(|_| serde_json::json!([]));
        serde_json::json!({"ok": true, "operation": "recall_mix_snapshot", "index": index, "name": snapshot.name, "states": snapshot.parameter_states, "plugin_states": snapshot.plugin_states, "routing": routing}).to_string()
    }

    /// Apply a captured scene to native track parameters. Capture uses four
    /// stable slots per track: `track_id*4 + {0:volume,1:pan,2:mute,3:solo}`;
    /// unknown IDs are ignored for forward compatibility.
    pub fn apply_mix_snapshot_json(&self, index: usize) -> String {
        let (states, plugin_states, routing_state) = {
            let Ok(snapshots) = self.mix_snapshots.lock() else {
                return "{\"ok\":false,\"code\":\"snapshot_lock_failed\"}".into();
            };
            let Some(snapshot) = snapshots.snapshots.get(index) else {
                return "{\"ok\":false,\"code\":\"snapshot_not_found\"}".into();
            };
            (
                snapshot.parameter_states.clone(),
                snapshot.plugin_states.clone(),
                snapshot.routing_state.clone(),
            )
        };
        let Some(engine) = self.engine.as_ref() else {
            return "{\"ok\":false,\"code\":\"engine_unavailable\"}".into();
        };
        let mut applied = 0usize;
        for (parameter_id, value) in states {
            let track_id = parameter_id / 2;
            let slot = parameter_id % 4;
            let accepted = if slot == 0 {
                value.is_finite()
                    && (0.0..=2.0).contains(&value)
                    && engine.set_track_volume(track_id, value)
            } else if slot == 1 {
                value.is_finite()
                    && (-1.0..=1.0).contains(&value)
                    && engine.set_track_pan(track_id, value)
            } else if slot == 2 {
                (value == 0.0 || value == 1.0) && engine.set_track_mute(track_id, value > 0.5)
            } else {
                (value == 0.0 || value == 1.0) && engine.set_track_solo(track_id, value > 0.5)
            };
            if accepted {
                applied += 1;
            }
        }
        for plugin in plugin_states {
            if !engine.set_plugin_bypass(plugin.track_id, plugin.plugin_index, plugin.bypassed) {
                continue;
            }
            applied += 1;
            for (parameter_id, value) in plugin.parameters.into_iter().enumerate() {
                if value.is_finite()
                    && engine.set_plugin_parameter_without_undo(
                        plugin.track_id,
                        plugin.plugin_index,
                        parameter_id as u32,
                        value,
                    )
                {
                    applied += 1;
                }
            }
        }
        if let Ok(routes) = serde_json::from_str::<Vec<crate::project_contracts::AudioRouteContract>>(
            &routing_state,
        ) {
            for route in routes {
                if route.source_id != route.destination_id
                    && route.gain.is_finite()
                    && (0.0..=2.0).contains(&route.gain)
                    && engine.set_route_gain(
                        route.source_id,
                        route.destination_id,
                        route.gain,
                        true,
                    )
                {
                    applied += 1;
                }
            }
        }
        serde_json::json!({"ok": true, "operation": "apply_mix_snapshot", "index": index, "applied": applied}).to_string()
    }
}
