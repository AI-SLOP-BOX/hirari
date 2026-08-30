use aura_core_bridge::stable_api::{
    AudioPluginSpec, AudioProcessRequest, CoreApiV1, PluginParameterValue,
};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let input_path = args
        .next()
        .ok_or("usage: process_audio <input.wav> <output.wav> [plugin] [gain-db]")?;
    let output_path = args
        .next()
        .ok_or("usage: process_audio <input.wav> <output.wav> [plugin] [gain-db]")?;
    let plugin = args.next().unwrap_or_else(|| "Aura Compressor".into());
    let gain_db = args
        .next()
        .map(|value| value.parse::<f32>())
        .transpose()?
        .unwrap_or(-3.0);
    if args.next().is_some() {
        return Err("usage: process_audio <input.wav> <output.wav> [plugin] [gain-db]".into());
    }

    let api = CoreApiV1::new_headless()?;
    let result = api.process_audio(AudioProcessRequest {
        input_path: input_path.into(),
        output_path: output_path.into(),
        gain_db,
        plugins: vec![AudioPluginSpec {
            alias: plugin,
            // Compressor: threshold, ratio, attack, release (normalized).
            parameters: vec![
                PluginParameterValue {
                    parameter_id: 0,
                    value: 0.55,
                },
                PluginParameterValue {
                    parameter_id: 1,
                    value: 0.20,
                },
                PluginParameterValue {
                    parameter_id: 2,
                    value: 0.10,
                },
                PluginParameterValue {
                    parameter_id: 3,
                    value: 0.15,
                },
            ],
        }],
    })?;
    println!("{}", serde_json::to_string_pretty(&result)?);
    Ok(())
}
