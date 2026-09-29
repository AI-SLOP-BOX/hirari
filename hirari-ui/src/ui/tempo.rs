use hirari_core_bridge::HirariCore;
use slint::ComponentHandle;
use std::cell::RefCell;
use std::collections::VecDeque;
use std::rc::Rc;
use std::time::Instant;

use crate::slint_ui::{AppWindow, TransportActions};

pub fn install(ui: &AppWindow, core: Rc<HirariCore>) {
    let weak = ui.as_weak();
    ui.global::<TransportActions>().on_set_bpm({
        let weak = weak.clone();
        let core = core.clone();
        move |bpm| {
            let ok = core.set_tempo(bpm);
            if let Some(ui) = weak.upgrade() {
                ui.set_last_action(if ok {
                    format!("TEMPO: {:.2} BPM", bpm.clamp(20.0, 300.0)).into()
                } else {
                    "TEMPO REJECTED".into()
                });
            }
        }
    });
    ui.global::<TransportActions>().on_tap_tempo({
        let weak = weak.clone();
        let core = core.clone();
        let taps = Rc::new(RefCell::new(VecDeque::<Instant>::new()));
        move || {
            let now = Instant::now();
            let mut taps = taps.borrow_mut();
            if taps.back().is_some_and(|previous| {
                let elapsed = now.duration_since(*previous).as_secs_f64();
                !(0.2..=3.0).contains(&elapsed)
            }) {
                taps.clear();
            }
            taps.push_back(now);
            while taps.len() > 5 {
                taps.pop_front();
            }

            if taps.len() >= 2 {
                let intervals: Vec<f64> = taps
                    .make_contiguous()
                    .windows(2)
                    .map(|pair| pair[1].duration_since(pair[0]).as_secs_f64())
                    .collect();
                let average = intervals.iter().sum::<f64>() / intervals.len() as f64;
                let bpm = (60.0 / average).clamp(20.0, 300.0) as f32;
                let ok = core.set_tempo(bpm);
                if let Some(ui) = weak.upgrade() {
                    ui.set_last_action(if ok {
                        format!("TAP TEMPO: {:.2} BPM", bpm).into()
                    } else {
                        "TAP TEMPO REJECTED".into()
                    });
                }
            } else if let Some(ui) = weak.upgrade() {
                ui.set_last_action("TAP TEMPO: TAP AGAIN".into());
            }
        }
    });
    ui.global::<TransportActions>()
        .on_set_time_signature_event({
            let weak = weak.clone();
            let core = core.clone();
            move |beat, numerator, denominator| {
                let valid =
                    (1..=32).contains(&numerator) && matches!(denominator, 1 | 2 | 4 | 8 | 16 | 32);
                let ok = valid
                    && core.set_time_signature_event(
                        beat as f64,
                        numerator as u8,
                        denominator as u8,
                    );
                if let Some(ui) = weak.upgrade() {
                    ui.set_last_action(if ok {
                        format!(
                            "TIME SIGNATURE: {}/{} @ {:.2} BEAT",
                            numerator, denominator, beat
                        )
                        .into()
                    } else {
                        "TIME SIGNATURE REJECTED".into()
                    });
                }
            }
        });
    ui.global::<TransportActions>().on_set_tempo_event({
        let weak = weak.clone();
        let core = core.clone();
        move |beat, bpm, ramp| {
            let ok = core.set_tempo_transition_event(beat as f64, bpm as f64, ramp);
            if let Some(ui) = weak.upgrade() {
                ui.set_last_action(if ok {
                    format!("TEMPO EVENT {:.3} BEAT / {:.2} BPM", beat, bpm).into()
                } else {
                    "TEMPO EVENT REJECTED".into()
                });
            }
        }
    });
    ui.global::<TransportActions>().on_remove_tempo_event({
        let weak = weak.clone();
        let core = core.clone();
        move |beat| {
            let ok = core.remove_tempo_event(beat as f64);
            if let Some(ui) = weak.upgrade() {
                ui.set_last_action(if ok {
                    format!("REMOVED TEMPO EVENT @ {:.2} BEAT", beat).into()
                } else {
                    "TEMPO EVENT REMOVE REJECTED".into()
                });
            }
        }
    });
    ui.global::<TransportActions>().on_set_snap({
        let weak = weak.clone();
        move |value| {
            let division = match value.as_str() {
                "1/1" => 1.0,
                "1/4" => 4.0,
                "1/8" => 8.0,
                "1/8T" => 12.0,
                "1/16" => 16.0,
                "1/16T" => 24.0,
                "1/32" => 32.0,
                "1/64" => 64.0,
                "Adaptive" => 16.0,
                _ => return,
            };
            if let Some(ui) = weak.upgrade() {
                ui.set_snap_division(division);
                ui.set_last_action(format!("SNAP: {}", value).into());
            }
        }
    });
    ui.global::<TransportActions>().on_move_tempo_event({
        let weak = weak.clone();
        let core = core.clone();
        move |from, to| {
            let ok = core.move_tempo_event(from as f64, to as f64);
            if let Some(ui) = weak.upgrade() {
                ui.set_last_action(if ok {
                    format!("MOVED TEMPO EVENT {:.2} → {:.2} BEAT", from, to).into()
                } else {
                    "TEMPO EVENT MOVE REJECTED".into()
                });
            }
        }
    });
}
