use std::collections::HashMap;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SidechainTapPointRust {
    PreFX,
    PostFX,
    PostFader,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn link(source: u32, dest: u32, plugin_idx: u32) -> SidechainLinkRust {
        SidechainLinkRust {
            source_track_id: source,
            dest_track_id: dest,
            plugin_idx,
            input_bus: 0,
            enabled: true,
            level: 1.0,
            tap_point: SidechainTapPointRust::PostFX,
        }
    }

    #[test]
    fn audit_rejects_duplicate_routes_and_invalid_levels() {
        let mut graph = SidechainOrchestrator::new();
        graph.register_link(1, link(1, 2, 0)).unwrap();
        graph.active_links.insert(
            2,
            SidechainLinkRust {
                level: f32::NAN,
                ..link(3, 4, 0)
            },
        );
        assert!(!graph.audit_sidechain_manager());
    }

    #[test]
    fn audit_rejects_imported_cycles() {
        let mut graph = SidechainOrchestrator::new();
        graph.active_links.insert(1, link(1, 2, 0));
        graph.active_links.insert(2, link(2, 1, 0));
        assert!(!graph.audit_sidechain_manager());
    }

    #[test]
    fn dynamic_plugin_ports_are_enumerated_and_removed() {
        let mut graph = SidechainOrchestrator::new();
        graph.register_link(20, link(3, 9, 1)).unwrap();
        graph.register_link(10, link(4, 9, 1)).unwrap();
        assert_eq!(
            graph
                .links_for_destination(9, 1)
                .iter()
                .map(|entry| entry.0)
                .collect::<Vec<_>>(),
            vec![10, 20]
        );
        assert_eq!(graph.remove_plugin_links(9, 1), 2);
        assert!(graph.links_for_destination(9, 1).is_empty());
        assert!(graph.audit_sidechain_manager());
    }

    #[test]
    fn supports_multiple_sources_and_multiple_plugin_inputs() {
        let mut graph = SidechainOrchestrator::new();
        graph.register_link(30, link(3, 9, 1)).unwrap();
        graph.register_link(10, link(4, 9, 1)).unwrap();
        let mut second_input = link(5, 9, 1);
        second_input.input_bus = 1;
        graph.register_link(20, second_input).unwrap();
        assert_eq!(
            graph.sources_for_input(9, 1, 0),
            vec![(10, 4, 1.0), (30, 3, 1.0)]
        );
        assert_eq!(graph.sources_for_input(9, 1, 1), vec![(20, 5, 1.0)]);
        assert_eq!(graph.resolve_source_for(9, 1), Some(4));
        assert!(graph.audit_sidechain_manager());
    }

    #[test]
    fn rejects_an_exact_duplicate_route_but_allows_shared_destination() {
        let mut graph = SidechainOrchestrator::new();
        graph.register_link(1, link(3, 9, 1)).unwrap();
        assert!(graph.register_link(2, link(4, 9, 1)).is_ok());
        assert_eq!(
            graph.register_link(3, link(3, 9, 1)),
            Err(SidechainLinkError::DuplicateRoute)
        );
    }
}

#[derive(Clone, Debug)]
pub struct SidechainLinkRust {
    pub source_track_id: u32,
    pub dest_track_id: u32,
    pub plugin_idx: u32,
    /// Zero-based side-chain input exposed by the plug-in.
    pub input_bus: u16,
    pub enabled: bool,
    pub level: f32,
    pub tap_point: SidechainTapPointRust,
}

pub struct SidechainOrchestrator {
    pub active_links: HashMap<u64, SidechainLinkRust>,
}

#[derive(Debug, PartialEq, Eq)]
pub enum SidechainLinkError {
    EmptyId,
    SelfReference,
    DuplicateId,
    DuplicateRoute,
    InvalidParameters,
    CircularReference,
}

impl Default for SidechainOrchestrator {
    fn default() -> Self {
        Self::new()
    }
}

impl SidechainOrchestrator {
    pub fn new() -> Self {
        Self {
            active_links: HashMap::new(),
        }
    }

    /// Registers a link after checking that it can be added to the routing graph.
    pub fn register_link(
        &mut self,
        link_id: u64,
        link: SidechainLinkRust,
    ) -> Result<(), SidechainLinkError> {
        if link_id == 0 || link.source_track_id == 0 || link.dest_track_id == 0 {
            return Err(SidechainLinkError::EmptyId);
        }
        if link.source_track_id == link.dest_track_id {
            return Err(SidechainLinkError::SelfReference);
        }
        if self.active_links.contains_key(&link_id) {
            return Err(SidechainLinkError::DuplicateId);
        }
        if link.input_bus >= 256 || !link.level.is_finite() || !(0.0..=1.0).contains(&link.level) {
            return Err(SidechainLinkError::InvalidParameters);
        }
        if self.active_links.values().any(|existing| {
            existing.source_track_id == link.source_track_id
                && existing.dest_track_id == link.dest_track_id
                && existing.plugin_idx == link.plugin_idx
                && existing.input_bus == link.input_bus
        }) {
            return Err(SidechainLinkError::DuplicateRoute);
        }
        if self.would_create_cycle(link.source_track_id, link.dest_track_id) {
            return Err(SidechainLinkError::CircularReference);
        }

        self.active_links.insert(link_id, link);
        Ok(())
    }

    /// Compatibility convenience for callers that only need success/failure.
    pub fn add_sidechain_link(&mut self, link_id: u64, link: SidechainLinkRust) -> bool {
        self.register_link(link_id, link).is_ok()
    }

    /// Resolves a link ID without panicking for an unknown ID.
    pub fn resolve_link(&self, link_id: u64) -> Option<&SidechainLinkRust> {
        self.active_links.get(&link_id)
    }

    pub fn remove_link(&mut self, link_id: u64) -> bool {
        self.active_links.remove(&link_id).is_some()
    }

    /// Removes every side-chain endpoint owned by a plugin instance. This is
    /// used when a plugin is deleted or bypassed so stale dynamic ports cannot
    /// remain in the routing graph.
    pub fn remove_plugin_links(&mut self, dest_track_id: u32, plugin_idx: u32) -> usize {
        let before = self.active_links.len();
        self.active_links.retain(|_, link| {
            !(link.dest_track_id == dest_track_id && link.plugin_idx == plugin_idx)
        });
        before - self.active_links.len()
    }

    /// Returns all sources feeding a plugin in deterministic link-ID order.
    pub fn links_for_destination(
        &self,
        dest_track_id: u32,
        plugin_idx: u32,
    ) -> Vec<(u64, u32, u16, bool, f32, SidechainTapPointRust)> {
        let mut links: Vec<_> = self
            .active_links
            .iter()
            .filter(|(_, link)| {
                link.dest_track_id == dest_track_id && link.plugin_idx == plugin_idx
            })
            .map(|(id, link)| {
                (
                    *id,
                    link.source_track_id,
                    link.input_bus,
                    link.enabled,
                    link.level,
                    link.tap_point,
                )
            })
            .collect();
        links.sort_by_key(|entry| entry.0);
        links
    }

    pub fn sources_for_input(
        &self,
        dest_track_id: u32,
        plugin_idx: u32,
        input_bus: u16,
    ) -> Vec<(u64, u32, f32)> {
        let mut sources: Vec<_> = self
            .active_links
            .iter()
            .filter(|(_, link)| {
                link.enabled
                    && link.dest_track_id == dest_track_id
                    && link.plugin_idx == plugin_idx
                    && link.input_bus == input_bus
            })
            .map(|(id, link)| (*id, link.source_track_id, link.level))
            .collect();
        sources.sort_by_key(|entry| entry.0);
        sources
    }

    pub fn set_enabled(&mut self, link_id: u64, enabled: bool) -> bool {
        let Some(link) = self.active_links.get_mut(&link_id) else {
            return false;
        };
        link.enabled = enabled;
        true
    }

    pub fn update_level(&mut self, link_id: u64, level: f32) -> bool {
        if !level.is_finite() || !(0.0..=1.0).contains(&level) {
            return false;
        }
        let Some(link) = self.active_links.get_mut(&link_id) else {
            return false;
        };
        link.level = level;
        true
    }

    pub fn set_tap_point(&mut self, link_id: u64, tap_point: SidechainTapPointRust) -> bool {
        let Some(link) = self.active_links.get_mut(&link_id) else {
            return false;
        };
        link.tap_point = tap_point;
        true
    }

    /// Resolves the source track for a destination/plugin pair.
    pub fn resolve_source_for(&self, dest_track_id: u32, plugin_idx: u32) -> Option<u32> {
        if dest_track_id == 0 {
            return None;
        }
        self.active_links
            .iter()
            .filter(|(_, link)| {
                link.enabled && link.dest_track_id == dest_track_id && link.plugin_idx == plugin_idx
            })
            .min_by_key(|(id, _)| *id)
            .map(|(_, link)| link.source_track_id)
    }

    /// Validates the already registered graph. Registration keeps this graph valid;
    /// this method remains the orchestration entry point for existing callers.
    pub fn resolve_sidechain_links(&mut self) {
        self.active_links.retain(|_, link| {
            link.source_track_id != 0
                && link.dest_track_id != 0
                && link.source_track_id != link.dest_track_id
        });
    }

    fn would_create_cycle(&self, source: u32, destination: u32) -> bool {
        let mut current = destination;
        let mut visited = std::collections::HashSet::new();
        while visited.insert(current) {
            if current == source {
                return true;
            }
            let Some(next) = self
                .active_links
                .values()
                .find(|link| link.source_track_id == current)
                .map(|link| link.dest_track_id)
            else {
                return false;
            };
            current = next;
        }
        false
    }

    /// INDUSTRIAL: Performs a forensic audit of the project-wide routing state.
    pub fn audit_sidechain_manager(&self) -> bool {
        if self.active_links.len() > 65_536 {
            return false;
        }
        let mut routes = std::collections::HashSet::new();
        for (link_id, link) in &self.active_links {
            if *link_id == 0
                || link.source_track_id == 0
                || link.dest_track_id == 0
                || link.source_track_id == link.dest_track_id
                || !link.level.is_finite()
                || !(0.0..=1.0).contains(&link.level)
                || link.input_bus >= 256
                || !matches!(
                    link.tap_point,
                    SidechainTapPointRust::PreFX
                        | SidechainTapPointRust::PostFX
                        | SidechainTapPointRust::PostFader
                )
            {
                return false;
            }
            if !routes.insert((
                link.source_track_id,
                link.dest_track_id,
                link.plugin_idx,
                link.input_bus,
            )) {
                return false;
            }
        }

        // Validate the complete graph, not only the edge most recently added.
        // This catches imported/legacy snapshots that bypass register_link().
        for link in self.active_links.values() {
            if self.would_create_cycle_from_snapshot(link.source_track_id, link.dest_track_id) {
                return false;
            }
        }
        true
    }

    fn would_create_cycle_from_snapshot(&self, source: u32, destination: u32) -> bool {
        let mut current = destination;
        let mut visited = std::collections::HashSet::new();
        while visited.insert(current) {
            if current == source {
                return true;
            }
            let Some(next) = self
                .active_links
                .values()
                .filter(|link| link.source_track_id == current)
                .map(|link| link.dest_track_id)
                .next()
            else {
                return false;
            };
            current = next;
        }
        true
    }
}
