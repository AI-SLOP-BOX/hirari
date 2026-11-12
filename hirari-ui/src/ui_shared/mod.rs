//! Backend-neutral UI support shared with the engine and kept stable across shell changes: measure math, track layout & parser code, plot rasters, template starters. This module is exposed `pub` so the engine and tooling can consume the same sources.

pub mod arrange_model;
pub mod gpu_canvas;
pub mod moufu_connection;
pub mod moufu_layout;
pub mod moufu_preview;
pub mod plot_raster;
pub mod preferences;
pub mod recent_projects;
pub mod render_ops;
pub mod render_path;
pub mod template_starter;
