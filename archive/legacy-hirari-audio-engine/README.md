# Archived, unintegrated Rust snippets

This directory preserves the former root-level `hirari-audio-engine` source
files for reference. That directory had no `Cargo.toml`, was not listed in the
workspace, and was not used by the application build. These files are not the
Hirari audio engine and must not be used as implementation entry points.

The maintained Rust application and bridge crates are listed in the root
workspace `Cargo.toml`; the audio engine is maintained in the C++ core reached
through `hirari-core-bridge`.
