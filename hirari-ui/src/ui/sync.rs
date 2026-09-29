use crate::slint_ui::Z_Track;
use slint::{Model, VecModel};

/// Single write boundary for the track view model.
pub fn replace_track(tracks: &VecModel<Z_Track>, index: usize, track: Z_Track) -> bool {
    if index >= tracks.row_count() {
        return false;
    }
    tracks.set_row_data(index, track);
    true
}

#[cfg(test)]
mod tests {
    use super::replace_track;
    use crate::slint_ui::fallback_template_tracks;
    use slint::{Model, VecModel};

    #[test]
    fn invalid_row_does_not_mutate_the_model() {
        let model = VecModel::from(fallback_template_tracks(&[("Test", 0)]));
        let original_name = model.row_data(0).expect("template track").name;
        let replacement = model.row_data(0).expect("template track");

        assert!(!replace_track(&model, 4, replacement));
        assert_eq!(
            model.row_data(0).expect("template track").name,
            original_name
        );
    }

    #[test]
    fn valid_row_replaces_the_model_value() {
        let model = VecModel::from(fallback_template_tracks(&[("Before", 0)]));
        let mut replacement = model.row_data(0).expect("template track");
        replacement.name = "After".into();

        assert!(replace_track(&model, 0, replacement));
        assert_eq!(model.row_data(0).expect("template track").name, "After");
    }
}
