use super::{read_pcm_wav, usage};
use aura_core_bridge::piano_visualizer::{PianoNote, PianoVisualizer, PianoVisualizerConfig};

pub(super) fn try_handle(raw_args: &[String]) -> bool {
    if raw_args.first().map(String::as_str) == Some("render")
        && raw_args.get(1).map(String::as_str) == Some("piano")
    {
        if !(5..=7).contains(&raw_args.len()) {
            usage();
        }
        let notes_text = match std::fs::read_to_string(&raw_args[2]) {
            Ok(text) => text,
            Err(error) => {
                eprintln!(
                    "{{\"ok\":false,\"error\":{}}}",
                    serde_json::to_string(&error.to_string()).unwrap()
                );
                std::process::exit(1);
            }
        };
        let notes: Vec<PianoNote> = match serde_json::from_str(&notes_text) {
            Ok(notes) => notes,
            Err(error) => {
                eprintln!(
                    "{{\"ok\":false,\"error\":{}}}",
                    serde_json::to_string(&format!("invalid notes JSON: {error}")).unwrap()
                );
                std::process::exit(1);
            }
        };
        let audio = match read_pcm_wav(&raw_args[3]) {
            Ok(audio) => audio,
            Err(error) => {
                eprintln!(
                    "{{\"ok\":false,\"error\":{}}}",
                    serde_json::to_string(&error).unwrap()
                );
                std::process::exit(1);
            }
        };
        let sample_rate = raw_args
            .get(5)
            .and_then(|value| value.parse().ok())
            .unwrap_or(48_000);
        let channels = raw_args
            .get(6)
            .and_then(|value| value.parse().ok())
            .unwrap_or(2);
        let config = PianoVisualizerConfig::default();
        match PianoVisualizer::render_to_mp4(
            &notes,
            &audio,
            sample_rate,
            channels,
            &config,
            std::path::Path::new(&raw_args[4]),
        ) {
            Ok(()) => println!(
                "{{\"ok\":true,\"operation\":\"piano_visualizer\",\"output_path\":{}}}",
                serde_json::to_string(&raw_args[4]).unwrap()
            ),
            Err(error) => {
                eprintln!(
                    "{{\"ok\":false,\"error\":{}}}",
                    serde_json::to_string(&error.to_string()).unwrap()
                );
                std::process::exit(1);
            }
        }
        return true;
    }
    false
}
