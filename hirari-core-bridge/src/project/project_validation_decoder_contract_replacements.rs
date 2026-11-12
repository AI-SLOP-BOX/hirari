//! Tests split out of project_validation.rs.

    use super::{ProjectDocument, ProjectRegion};

    /// A minimal well-formed layout: one track, one region on it.
    const ONE_TRACK_ONE_REGION: &str = r#"[{
        "id": 12,
        "name": "Lead \"Vocal\"",
        "type": "Midi",
        "regions": [{"id": 9001, "path": "/tmp/phrase.wav", "start": 0, "len": 64}]
    }]"#;

    fn hydrate(layout: &str) -> ProjectDocument {
        ProjectDocument::from_layout_json("Demo", 128.0, 48_000.0, layout)
            .expect("well-formed layout should hydrate")
    }

    /// These assertions replace `tests/project_decoder_contract.cpp`, whose
    /// `ProjectDecoder` was included by nothing but its own test and whose
    /// JSON schema differed from the document the product actually loads. The
    /// properties that one asserted are restated here against the path that
    /// really runs: `from_layout_json`, which builds the document the loader
    /// hands to the engine, and `ProjectDocument::validate`.
    #[test]
    fn a_layout_with_a_duplicate_track_id_is_refused() {
        let err = ProjectDocument::from_layout_json(
            "Demo",
            128.0,
            48_000.0,
            r#"[{"id": 12, "name": "A"}, {"id": 12, "volume": 0.2}]"#,
        )
        .expect_err("a repeated track id must not hydrate");
        assert!(
            err.to_string().contains("duplicate track id 12"),
            "unexpected rejection: {err}"
        );
    }

    #[test]
    fn a_layout_with_distinct_track_ids_hydrates_every_track() {
        let document = hydrate(
            r#"[{"id": 12, "name": "Lead \"Vocal\"", "type": "Midi", "muted": true},
                {"id": 13, "name": "Bass", "volume": 0.2, "pan": -0.1}]"#,
        );
        assert_eq!(document.tracks.len(), 2);
        assert_eq!(document.tracks[0].id, 12);
        assert_eq!(document.tracks[1].id, 13);
        assert_eq!(document.tracks[0].name, "Lead \"Vocal\"");
        assert_eq!(document.tracks[0].track_type, "Midi");
        assert!(document.tracks[0].muted);
        assert_eq!(document.tracks[1].volume, 0.2);
        assert_eq!(document.tracks[1].pan, -0.1);
        assert!(document.validate().is_ok());
    }

    #[test]
    fn a_region_belonging_to_no_track_is_refused() {
        let mut document = hydrate(ONE_TRACK_ONE_REGION);
        assert_eq!(document.regions.len(), 1);
        let mut orphan = document.regions[0].clone();
        orphan.track_id = 999;
        document.regions = vec![orphan];
        let err = document
            .validate()
            .expect_err("a region on an unknown track must not validate");
        assert!(
            err.to_string().contains("unknown track"),
            "unexpected rejection: {err}"
        );
    }

    #[test]
    fn a_region_id_of_zero_is_refused() {
        let mut document = hydrate(ONE_TRACK_ONE_REGION);
        document.regions[0].id = 0;
        let err = document
            .validate()
            .expect_err("region id zero is the invalid sentinel");
        assert!(
            err.to_string().contains("region id must be non-zero"),
            "unexpected rejection: {err}"
        );
    }

    #[test]
    fn a_region_without_media_is_refused() {
        for (label, mutate) in [
            ("empty path", Box::new(|r: &mut ProjectRegion| r.path.clear())
                as Box<dyn Fn(&mut ProjectRegion)>),
            ("zero length", Box::new(|r: &mut ProjectRegion| r.length = 0)),
        ] {
            let mut document = hydrate(ONE_TRACK_ONE_REGION);
            mutate(&mut document.regions[0]);
            let err = document
                .validate()
                .expect_err("a region with no media must not validate");
            assert!(
                err.to_string().contains("invalid media data"),
                "{label}: unexpected rejection: {err}"
            );
        }
    }

    /// The old contract fed the decoder a region list that was an object
    /// rather than an array, and a `clip_gain` written as a string, and
    /// required both to fail closed. Ignoring either would silently produce a
    /// different project than the file described, so the same two shapes are
    /// refused here at the point the product parses a layout.
    #[test]
    fn a_field_with_the_wrong_json_type_is_refused() {
        let object_instead_of_array = r#"[{
            "id": 21, "name": "Broken",
            "regions": {"id": 1, "path": "/tmp/a.wav", "start": 0, "len": 8}
        }]"#;
        assert!(
            ProjectDocument::from_layout_json("Demo", 128.0, 48_000.0, object_instead_of_array)
                .is_err(),
            "a region list that is an object must not parse"
        );

        let string_instead_of_number = r#"[{
            "id": 22,
            "regions": [{"id": 2, "path": "/tmp/a.wav", "start": 0, "len": 8,
                         "clip_gain": "not-a-number"}]
        }]"#;
        assert!(
            ProjectDocument::from_layout_json("Demo", 128.0, 48_000.0, string_instead_of_number)
                .is_err(),
            "a clip_gain written as a string must not parse"
        );

        let missing_track_id = r#"[{"name": "No id"}]"#;
        assert!(
            ProjectDocument::from_layout_json("Demo", 128.0, 48_000.0, missing_track_id)
                .is_err(),
            "a track without an id must not parse"
        );
    }
