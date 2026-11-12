//! View cards for the AmeUI shell.
use crate::ameui::*;
impl HirariAmeApp {
    pub(crate) fn moufu_card_view(&self) -> View {
        // Moufu card (moufu_link_page parity): Hub address,
        // connect/disconnect, entity share/receive, links list and
        // read-only linked-project preview.
        let moufu_card = {
            let address_state = self.moufu_address.clone();
            let address_change = address_state.clone();
            let address_field = TextFieldView::new(
                self.moufu_address.get(),
                "Moufu Hub address (e.g. 127.0.0.1:9478)",
            )
            .change_action_id("moufu.address_change")
            .submit_action_id("moufu.address_change")
            .on_change(move |value| address_change.set(value))
            .on_submit(|| {});
            let connect_state = self.moufu_address.clone();
            let connect_publisher = self.moufu_publisher.clone();
            let connect_status = self.moufu_status.clone();
            let connect_button = ButtonView::new("CONNECT")
                .action_id("moufu.connect")
                .primary()
                .on_press(move |_| {
                    let address = connect_state.get().trim().to_owned();
                    if address.len() > 1024 {
                        connect_status.set("MOUFU ADDRESS IS TOO LONG".to_owned());
                        return;
                    }
                    match &connect_publisher {
                        Some(publisher) => {
                            let target = (!address.is_empty()).then_some(address.as_str());
                            match publisher.set_address(target) {
                                Ok(()) => {
                                    let _ = save_moufu_address(&address);
                                    connect_status.set(if address.is_empty() {
                                        "MOUFU NOT CONFIGURED".to_owned()
                                    } else {
                                        "MOUFU CONNECTING".to_owned()
                                    });
                                }
                                Err(error) => {
                                    connect_status
                                        .set(format!("MOUFU CONFIGURATION FAILED: {error}"));
                                }
                            }
                        }
                        None => connect_status.set("MOUFU UNAVAILABLE IN THIS RUN MODE".to_owned()),
                    }
                });
            let disconnect_state = self.moufu_address.clone();
            let disconnect_publisher = self.moufu_publisher.clone();
            let disconnect_status = self.moufu_status.clone();
            let disconnect_button =
                ButtonView::new("DISCONNECT")
                    .action_id("moufu.disconnect")
                    .secondary()
                    .on_press(move |_| {
                        disconnect_state.set(String::new());
                        let _ = save_moufu_address("");
                        match &disconnect_publisher {
                            Some(publisher) => match publisher.set_address(None) {
                                Ok(()) => disconnect_status.set("MOUFU NOT CONFIGURED".to_owned()),
                                Err(error) => disconnect_status
                                    .set(format!("MOUFU DISCONNECT FAILED: {error}")),
                            },
                            None => disconnect_status
                                .set("MOUFU UNAVAILABLE IN THIS RUN MODE".to_owned()),
                        }
                    });
            let refresh_publisher = self.moufu_publisher.clone();
            let refresh_status = self.moufu_status.clone();
            let refresh_button = ButtonView::new("REFRESH")
                .action_id("moufu.refresh")
                .secondary()
                .on_press(move |_| match &refresh_publisher {
                    Some(publisher) => {
                        let entities = publisher.list_entities();
                        let links = publisher.list_links();
                        if let Err(error) = entities.and(links) {
                            refresh_status.set(format!("MOUFU REFRESH FAILED: {error}"));
                        }
                    }
                    None => refresh_status.set("MOUFU UNAVAILABLE IN THIS RUN MODE".to_owned()),
                });
            let entity_labels = self.moufu_entity_labels.get();
            let entity_count = entity_labels.len() as u32;
            let entity_index = self
                .moufu_entity_selected
                .get()
                .min(entity_count.saturating_sub(1));
            let entity_name = entity_labels
                .get(entity_index as usize)
                .cloned()
                .unwrap_or_else(|| "no entities".to_owned());
            let cycle_label = format!("ENT {entity_index}/{}", entity_count.max(1));
            let entity_selected_state = self.moufu_entity_selected.clone();
            let entity_cycle_button = ButtonView::new(cycle_label)
                .action_id("moufu.entity_cycle")
                .secondary()
                .on_press(move |_| {
                    if entity_count > 0 {
                        entity_selected_state.set((entity_selected_state.get() + 1) % entity_count);
                    }
                });
            let share_publisher = self.moufu_publisher.clone();
            let share_ids = self.moufu_entity_ids.clone();
            let share_selected = self.moufu_entity_selected.clone();
            let share_status = self.moufu_status.clone();
            let share_button = ButtonView::new("SHARE")
                .action_id("moufu.share")
                .secondary()
                .on_press(move |_| {
                    let ids = share_ids.get();
                    let target = ids.get(share_selected.get() as usize).cloned();
                    let Some(target) = target else { return };
                    match &share_publisher {
                        Some(publisher) => {
                            if let Err(error) =
                                publisher.create_link(HIRARI_PROJECT_ENTITY, &target)
                            {
                                share_status.set(format!("MOUFU SHARE FAILED: {error}"));
                            }
                        }
                        None => share_status.set("MOUFU UNAVAILABLE IN THIS RUN MODE".to_owned()),
                    }
                });
            let receive_publisher = self.moufu_publisher.clone();
            let receive_ids = self.moufu_entity_ids.clone();
            let receive_selected = self.moufu_entity_selected.clone();
            let receive_status = self.moufu_status.clone();
            let receive_button = ButtonView::new("RECEIVE")
                .action_id("moufu.receive")
                .secondary()
                .on_press(move |_| {
                    let ids = receive_ids.get();
                    let source = ids.get(receive_selected.get() as usize).cloned();
                    let Some(source) = source else { return };
                    match &receive_publisher {
                        Some(publisher) => {
                            if let Err(error) =
                                publisher.create_link(&source, HIRARI_PROJECT_ENTITY)
                            {
                                receive_status.set(format!("MOUFU RECEIVE FAILED: {error}"));
                            }
                        }
                        None => receive_status.set("MOUFU UNAVAILABLE IN THIS RUN MODE".to_owned()),
                    }
                });
            let link_labels = self.moufu_link_labels.get();
            let link_count = link_labels.len() as u32;
            let link_index = self
                .moufu_link_selected
                .get()
                .min(link_count.saturating_sub(1));
            let link_name = link_labels
                .get(link_index as usize)
                .cloned()
                .unwrap_or_else(|| "no links".to_owned());
            let link_cycle = format!("LINK {link_index}/{}", link_count.max(1));
            let link_selected_state = self.moufu_link_selected.clone();
            let link_cycle_button = ButtonView::new(link_cycle)
                .action_id("moufu.link_cycle")
                .secondary()
                .on_press(move |_| {
                    if link_count > 0 {
                        link_selected_state.set((link_selected_state.get() + 1) % link_count);
                    }
                });
            let unlink_publisher = self.moufu_publisher.clone();
            let unlink_ids = self.moufu_link_ids.clone();
            let unlink_selected = self.moufu_link_selected.clone();
            let unlink_status = self.moufu_status.clone();
            let unlink_button = ButtonView::new("REMOVE LINK")
                .action_id("moufu.unlink")
                .secondary()
                .on_press(move |_| {
                    let ids = unlink_ids.get();
                    let link_id = ids.get(unlink_selected.get() as usize).cloned();
                    let Some(link_id) = link_id else { return };
                    match &unlink_publisher {
                        Some(publisher) => {
                            if let Err(error) = publisher.destroy_link(&link_id) {
                                unlink_status.set(format!("MOUFU REMOVE LINK FAILED: {error}"));
                            }
                        }
                        None => unlink_status.set("MOUFU UNAVAILABLE IN THIS RUN MODE".to_owned()),
                    }
                });
            View::Card(vec![
                View::Text("MOUFU HUB".to_owned()),
                View::HStack(vec![
                    address_field.into(),
                    connect_button.into(),
                    disconnect_button.into(),
                    refresh_button.into(),
                ]),
                View::Caption(self.moufu_status.get()),
                View::HStack(vec![
                    View::Caption(format!("ENT: {entity_name}")),
                    entity_cycle_button.into(),
                    share_button.into(),
                    receive_button.into(),
                ]),
                View::HStack(vec![
                    View::Caption(format!("LINK: {link_name}")),
                    link_cycle_button.into(),
                    unlink_button.into(),
                ]),
                View::Caption(if self.moufu_remote_source.get().is_empty() {
                    self.moufu_preview.get()
                } else {
                    format!("{}
{}", self.moufu_remote_source.get(), self.moufu_preview.get())
                }),
                View::Caption(
                    "Project names/notes/markers sync both ways; file paths and plugin state stay local. Connect only to a trusted Hub."
                        .to_owned(),
                ),
            ])
        };

        moufu_card
    }
}
