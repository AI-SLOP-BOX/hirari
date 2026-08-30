//! Domain-neutral Audio/VFX parameter bindings.
//!
//! Bindings are evaluated on the shared MasterClock, so a VFX client can use
//! audio automation, MIDI, or a marker-derived value without knowing the
//! internals of the Audio Core.

use crate::production_timeline::{BindingValue, MasterClock, ParameterBinding};
use serde::{Deserialize, Serialize};

pub const VFX_BINDING_API_VERSION: &str = "aura.vfx-bindings.v1";

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
pub struct BindingPoint {
    pub clock: MasterClock,
    pub value: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ParameterBindingCurve {
    pub binding: ParameterBinding,
    pub points: Vec<BindingPoint>,
    #[serde(default = "default_true")]
    pub clamp_output: bool,
}

fn default_true() -> bool {
    true
}

impl ParameterBindingCurve {
    pub fn validate(&self) -> bool {
        !self.binding.source.trim().is_empty()
            && !self.binding.target.trim().is_empty()
            && self.points.len() <= 65_536
            && self
                .points
                .windows(2)
                .all(|pair| pair[0].clock.tick.0 < pair[1].clock.tick.0)
            && self.points.iter().all(|point| point.value.is_finite())
    }

    pub fn evaluate(&self, clock: MasterClock) -> Option<BindingValue> {
        if !self.validate() || self.points.is_empty() || clock.ticks_per_second == 0 {
            return None;
        }
        let first = self.points.first()?;
        let last = self.points.last()?;
        let value = if clock.tick.0 <= first.clock.tick.0 {
            first.value
        } else if clock.tick.0 >= last.clock.tick.0 {
            last.value
        } else {
            let right = self
                .points
                .partition_point(|point| point.clock.tick.0 < clock.tick.0);
            let left_point = self.points.get(right.checked_sub(1)?)?;
            let right_point = self.points.get(right)?;
            let span = (right_point.clock.tick.0 - left_point.clock.tick.0) as f64;
            let progress = (clock.tick.0 - left_point.clock.tick.0) as f64 / span;
            left_point.value + (right_point.value - left_point.value) * progress
        };
        Some(BindingValue {
            clock,
            value: if self.clamp_output {
                value.clamp(0.0, 1.0)
            } else {
                value
            },
        })
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct VfxBindingGraph {
    pub curves: Vec<ParameterBindingCurve>,
}

impl VfxBindingGraph {
    pub fn upsert(&mut self, curve: ParameterBindingCurve) -> bool {
        if !curve.validate() {
            return false;
        }
        if let Some(existing) = self
            .curves
            .iter_mut()
            .find(|item| item.binding == curve.binding)
        {
            *existing = curve;
        } else if self.curves.len() < 4096 {
            self.curves.push(curve);
        } else {
            return false;
        }
        true
    }

    pub fn evaluate_target(&self, target: &str, clock: MasterClock) -> Option<BindingValue> {
        self.curves
            .iter()
            .find(|curve| curve.binding.target == target)?
            .evaluate(clock)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn clock(tick: u128) -> MasterClock {
        MasterClock {
            tick: crate::production_timeline::MasterTick(tick),
            ticks_per_second: 1_000,
        }
    }

    #[test]
    fn audio_automation_interpolates_to_a_vfx_parameter() {
        let mut graph = VfxBindingGraph::default();
        assert!(graph.upsert(ParameterBindingCurve {
            binding: ParameterBinding {
                source: "audio.synth.cutoff".into(),
                target: "vfx.glow.intensity".into(),
                source_unit: "normalized".into(),
                target_unit: "normalized".into()
            },
            points: vec![
                BindingPoint {
                    clock: clock(0),
                    value: 0.0
                },
                BindingPoint {
                    clock: clock(1_000),
                    value: 1.0
                }
            ],
            clamp_output: true,
        }));
        let value = graph
            .evaluate_target("vfx.glow.intensity", clock(250))
            .unwrap();
        assert!((value.value - 0.25).abs() < 1e-9);
    }

    #[test]
    fn invalid_ordered_points_are_rejected() {
        let curve = ParameterBindingCurve {
            binding: ParameterBinding {
                source: "a".into(),
                target: "b".into(),
                source_unit: "x".into(),
                target_unit: "y".into(),
            },
            points: vec![
                BindingPoint {
                    clock: clock(2),
                    value: 0.0,
                },
                BindingPoint {
                    clock: clock(1),
                    value: 1.0,
                },
            ],
            clamp_output: true,
        };
        assert!(!curve.validate());
    }
}
