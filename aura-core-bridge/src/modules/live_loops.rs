use crate::{WorkspaceState, EngineCommand, ForensicSeverity, ForensicModule};

pub struct LiveLoopsOrchestrator;

impl LiveLoopsOrchestrator {
    pub fn handle_command(state: &mut WorkspaceState, cmd: EngineCommand) -> bool {
        match cmd {
            EngineCommand::AddLiveLoopCell { col, row, label, node_ids } => {
                if state.live_loops.columns == 0 {
                    return false;
                }
                let cell_idx = row * state.live_loops.columns + col;
                if cell_idx < state.live_loops.cells.len() {
                    state.live_loops.cells[cell_idx] = crate::LiveLoopsCell {
                        // Cell identity is structural: it remains stable across
                        // repeated edits and does not depend on RNG state.
                        id: (cell_idx as u64).saturating_add(1),
                        label,
                        node_ids,
                        is_active: false,
                        color: [
                            64u8.saturating_add(((col * 37) % 160) as u8),
                            96u8.saturating_add(((row * 29) % 128) as u8),
                            192,
                            255,
                        ],
                    };
                    true
                } else {
                    false
                }
            }
            EngineCommand::TriggerLiveLoopCell { col, row, active } => {
                if state.live_loops.columns == 0 {
                    return false;
                }
                let cell_idx = row * state.live_loops.columns + col;
                if let Some(cell) = state.live_loops.cells.get_mut(cell_idx) {
                    cell.is_active = active;
                    
                    // INDUSTRIAL: Update node bypass states based on cell activity
                    for &node_id in &cell.node_ids {
                        if let Some(node) = state.nodes.iter_mut().find(|n| n.id == node_id) {
                            node.is_bypassed = !active;
                        }
                    }
                    
                    crate::aura_log!(
                        ForensicSeverity::Info,
                        ForensicModule::Core,
                        "LIVE LOOPS: Cell ({}, {}) {} - affected {} nodes.",
                        col, row, if active { "ACTIVATED" } else { "DEACTIVATED" }, cell.node_ids.len()
                    );
                }
                true
            }
            _ => false,
        }
    }
}
