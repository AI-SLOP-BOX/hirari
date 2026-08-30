use aura_core_bridge::stable_api::{CoreApiV1, MixRenderRequest, WaveContainer};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let project_path = args
        .next()
        .ok_or("usage: headless_render <project.aura> <output.wav>")?;
    let output_path = args
        .next()
        .ok_or("usage: headless_render <project.aura> <output.wav>")?;
    if args.next().is_some() {
        return Err("usage: headless_render <project.aura> <output.wav>".into());
    }

    let api = CoreApiV1::new_headless()?;
    let result = api.render_mix(MixRenderRequest {
        project_path: project_path.into(),
        output_path: output_path.into(),
        container: WaveContainer::Wav,
    })?;
    println!("{}", serde_json::to_string_pretty(&result)?);
    Ok(())
}
