//! View cards for the AmeUI shell.
use crate::ameui::*;
impl HirariAmeApp {
    pub(crate) fn stacks_card_view(&self) -> View {
        // TRACK STACKS card (P10): Create/add/gain/collapse/delete stack
        // driven by the engine's track_stacks API.
        let stacks_card = {
            let create_core = self.core.clone();
            let create_selection = self.piano_track.clone();
            let create_selected_id = self.stack_selected_id.clone();
            let create_button = ButtonView::new("CREATE FROM SELECTED")
                .action_id("stacks.create")
                .secondary()
                .on_press(move |_| {
                    let Some(core) = &create_core else { return };
                    let member = create_selection.get();
                    if member == 0 {
                        return;
                    }
                    let json = core.track_stacks_json();
                    let stacks = parse_track_stacks(&json);
                    let next_id = stacks.iter().map(|stack| stack.id).max().unwrap_or(0) + 1;
                    let created_name = format!("Stack {next_id}");
                    let _ = core.upsert_track_stack(next_id, &created_name, &[member], 1.0, false);
                    create_selected_id.set(next_id);
                });
            let add_core = self.core.clone();
            let add_selection = self.piano_track.clone();
            let add_selected_id = self.stack_selected_id.clone();
            let add_button = ButtonView::new("ADD SELECTED")
                .action_id("stacks.add")
                .secondary()
                .on_press(move |_| {
                    let Some(core) = &add_core else { return };
                    let member = add_selection.get();
                    let stack_id = add_selected_id.get();
                    if stack_id == 0 || member == 0 {
                        return;
                    }
                    let _ = core.add_track_stack_member(stack_id, member);
                });
            let gains_core = self.core.clone();
            let gains_id = self.stack_selected_id.clone();
            let gain_slider = SliderView::new(self.stack_gain.get(), 0.0, 2.0)
                .action_id("stacks.gain")
                .on_change(move |value| {
                    let clamp = value.clamp(0.0, 2.0);
                    if let Some(core) = &gains_core {
                        let _ = core.set_track_stack_gain(gains_id.get(), clamp);
                    }
                });
            let collapse_state = self.stack_collapsed.clone();
            let collapse_id = self.stack_selected_id.clone();
            let collapse_core = self.core.clone();
            let collapse_button = CheckboxView::new("COLLAPSE", self.stack_collapsed.get())
                .action_id("stacks.collapse")
                .on_toggle(move |next| {
                    let id = collapse_id.get();
                    if id == 0 {
                        return;
                    }
                    if let Some(core) = &collapse_core {
                        let _ = core.set_track_stack_collapsed(id, next);
                    }
                    collapse_state.set(next);
                });
            let delete_core = self.core.clone();
            let delete_id = self.stack_selected_id.clone();
            let delete_button = ButtonView::new("DELETE")
                .action_id("stacks.delete")
                .secondary()
                .on_press(move |_| {
                    let id = delete_id.get();
                    if id == 0 {
                        return;
                    }
                    let Some(core) = &delete_core else { return };
                    let _ = core.delete_track_stack(id);
                    delete_id.set(0);
                });
            let caption = if self.stack_selected_id.get() == 0 {
                "No stack selected — CREATE FROM SELECTED to make one".to_owned()
            } else {
                format!(
                    "#{} · {}",
                    self.stack_selected_id.get(),
                    self.stack_name.get()
                )
            };
            View::Card(vec![View::HStack(vec![
                View::Text("TRACK STACKS".to_owned()),
                create_button.into(),
                add_button.into(),
                View::Caption(caption),
                gain_slider.into(),
                collapse_button.into(),
                delete_button.into(),
            ])])
        };

        stacks_card
    }
}
