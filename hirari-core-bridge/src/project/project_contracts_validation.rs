pub fn validate_contracts(
    plugins: &[PluginInstanceContract],
    mappings: &[MidiLearnMappingContract],
    macro_mappings: &[MacroMappingContract],
    markers: &[WarpMarkerContract],
    targets: &[RenderTargetContract],
) -> Result<()> {
    let mut ids = HashSet::new();
    for plugin in plugins {
        if plugin.instance_id.trim().is_empty() || !ids.insert(&plugin.instance_id) {
            bail!("plugin instance ids must be non-empty and unique");
        }
        if plugin.plugin_id.trim().is_empty() || plugin.bundle_path.contains('\0') {
            bail!(
                "plugin instance {} has invalid identity",
                plugin.instance_id
            );
        }
        if plugin.component_ids.len() > 64
            || plugin.component_ids.iter().any(|component| {
                component.trim().is_empty() || component.len() > 256 || component.contains('\0')
            })
            || plugin.input_channels > 256
            || plugin.output_channels > 256
            || plugin.sidechain_channels > 256
            || (plugin.sidechain_channels > 0 && plugin.output_channels == 0)
        {
            bail!(
                "plugin instance {} has invalid bus layout",
                plugin.instance_id
            );
        }
        if !plugin.binary_hash.is_empty() && plugin.binary_hash.len() != 64 {
            bail!(
                "plugin instance {} has invalid binary hash",
                plugin.instance_id
            );
        }
        if plugin.plugin_version.len() > 256
            || plugin.architecture.len() > 32
            || plugin.plugin_version.contains('\0')
            || plugin.architecture.contains('\0')
        {
            bail!(
                "plugin instance {} has invalid binary identity",
                plugin.instance_id
            );
        }
        if plugin.state_schema_version > 1 {
            bail!(
                "plugin instance {} has unsupported state schema",
                plugin.instance_id
            );
        }
        if plugin.state_blob.len() > 4 * 1024 * 1024 || plugin.gui_state.len() > 1024 * 1024 {
            bail!(
                "plugin instance {} exceeds state limits",
                plugin.instance_id
            );
        }
        if plugin.latency_samples > 16 * 1024 * 1024 {
            bail!("plugin instance {} has invalid latency", plugin.instance_id);
        }
        if plugin.parameter_values.len() > 65_536
            || (!plugin.parameter_ids.is_empty()
                && plugin.parameter_ids.len() != plugin.parameter_values.len())
            || plugin
                .parameter_values
                .iter()
                .any(|value| !value.is_finite() || !(0.0..=1.0).contains(value))
        {
            bail!(
                "plugin instance {} has invalid parameter state",
                plugin.instance_id
            );
        }
        let mut parameter_ids = HashSet::with_capacity(plugin.parameter_ids.len());
        if plugin.parameter_ids.iter().any(|parameter| {
            parameter.trim().is_empty()
                || parameter.len() > 256
                || parameter.contains('\0')
                || !parameter_ids.insert(parameter)
        }) {
            bail!(
                "plugin instance {} has duplicate or invalid parameter ids",
                plugin.instance_id
            );
        }
    }

    let mut macro_ids = HashSet::new();
    for mapping in macro_mappings {
        if mapping.mapping_id.trim().is_empty() || !macro_ids.insert(&mapping.mapping_id) {
            bail!("macro mapping ids must be non-empty and unique");
        }
        let Some(target_plugin) = plugins
            .iter()
            .find(|plugin| plugin.instance_id == mapping.target_instance_id)
        else {
            bail!(
                "macro mapping {} targets an unknown plugin",
                mapping.mapping_id
            );
        };
        if mapping.macro_index >= 128
            || mapping.target_parameter_id.trim().is_empty()
            || (!target_plugin.parameter_ids.is_empty()
                && !target_plugin
                    .parameter_ids
                    .iter()
                    .any(|parameter| parameter == &mapping.target_parameter_id))
            || !mapping.min.is_finite()
            || !mapping.max.is_finite()
            || mapping.min > mapping.max
            || !mapping.curve.is_finite()
            || !(-1.0..=1.0).contains(&mapping.curve)
        {
            bail!("macro mapping {} is invalid", mapping.mapping_id);
        }
    }

    let mut mapping_ids = HashSet::new();
    for mapping in mappings {
        if mapping.mapping_id.trim().is_empty() || !mapping_ids.insert(&mapping.mapping_id) {
            bail!("MIDI mapping ids must be non-empty and unique");
        }
        if mapping.device_id.trim().is_empty()
            || mapping.target_instance_id.trim().is_empty()
            || mapping.target_parameter_id.trim().is_empty()
            || mapping.channel > 15
            || mapping.controller > 16_383
            || !mapping.min.is_finite()
            || !mapping.max.is_finite()
            || mapping.min > mapping.max
            || !mapping.curve.is_finite()
        {
            bail!("MIDI mapping {} is invalid", mapping.mapping_id);
        }
    }

    let mut marker_ids = HashSet::new();
    for marker in markers {
        if marker.marker_id.trim().is_empty() || !marker_ids.insert(&marker.marker_id) {
            bail!("warp marker ids must be non-empty and unique");
        }
        if !marker.pitch_semitones.is_finite() || !(-48.0..=48.0).contains(&marker.pitch_semitones)
        {
            bail!("warp marker {} has invalid pitch", marker.marker_id);
        }
    }

    let mut target_ids = HashSet::new();
    for target in targets {
        if target.target_id.trim().is_empty() || !target_ids.insert(&target.target_id) {
            bail!("render target ids must be non-empty and unique");
        }
        if target.offline_generation == 0 {
            bail!(
                "render target {} has no offline generation",
                target.target_id
            );
        }
    }
    Ok(())
}
