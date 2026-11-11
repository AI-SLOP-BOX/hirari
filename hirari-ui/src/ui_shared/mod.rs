//! Neutral UI support shared with a future non-Slint surface (basically Hirari's measure math, track layout & parser code; the shared template starter). To avoid a cyclic story this module is exposed `pub`.

pub mod arrange_model;
pub mod gpu_canvas;
pub mod moufu_connection;
pub mod moufu_layout;
pub mod moufu_preview;
pub mod plot_raster;
pub mod recent_projects;
pub mod render_ops;
pub mod render_path;
pub mod template_starter;
