//! View cards for the AmeUI shell.
use crate::ameui::*;
impl HirariAmeApp {
    pub(crate) fn video_card_view(&self) -> View {
        // Video card (media/video_monitor parity): toggle and load.
        // Frame display fans out through the video canvas; revision-tracked.
        let video_open = self.show_video.clone();
        let video_toggle = ButtonView::new(if self.show_video.get() {
            "HIDE VIDEO"
        } else {
            "SHOW VIDEO"
        })
        .action_id("video.toggle")
        .secondary()
        .on_press(move |_| {
            video_open.set(!video_open.get());
        });
        let video_pending = self.pending_video_load.clone();
        let video_load = ButtonView::new("LOAD VIDEO")
            .action_id("video.load")
            .secondary()
            .on_press(move |_| {
                let dialog = ame_ui::platform::NativeFileDialog::new()
                    .add_filter("Video file", &["mov", "mp4", "mkv", "webm", "avi"]);
                let pending = video_pending.clone();
                dialog.pick_file_async(move |path| {
                    if let Some(path) = path {
                        if let Ok(mut slot) = pending.lock() {
                            *slot = Some(path);
                        }
                    }
                });
            });

        let video_card = View::Card(vec![View::HStack(vec![
            View::Text("VIDEO".to_owned()),
            video_load.into(),
            video_toggle.into(),
        ])]);

        video_card
    }
}
