impl HirariCore {
    pub fn poll_events_into(&self, out: &mut Vec<ffi::BridgeEvent>) {
        if let Some(e) = self.engine.as_ref() {
            let host = crate::engine_event_ops::host_from_core(ffi::get_unified_engine(e));

            // --- INDUSTRIAL: Zero-Allocation Collection ---
            // Reuses the passed vector to avoid reallocations.
            out.clear();
            let mut ev = ffi::BridgeEvent {
                timestamp: 0,
                event_type: 0,
                track_id: 0,
                value: 0.0,
                label: [0; 128],
            };

            for _ in 0..128 {
                // Support higher burst volume (Point 1)
                if crate::engine_event_ops::pop_event(host, &mut ev) {
                    out.push(ev);
                } else {
                    break;
                }
            }
        }
    }
}
