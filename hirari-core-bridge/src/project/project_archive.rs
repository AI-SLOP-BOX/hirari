//! Project package/archive support.
//!
//! A portable project package is one ZIP file containing project JSON, a
//! deterministic dependency manifest, and copied project media. Legacy
//! directory APIs remain available for old packages; UI archive/restore uses
//! the single-file APIs. The manifest is content addressed so packages can
//! be checked before they are opened or transferred.

use crate::project::ProjectDocument;
use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, File};
use std::io::{Read, Write};
use std::path::{Component, Path, PathBuf};
use zip::read::ZipArchive as ReadZipArchive;
use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, ZipWriter};

pub const ARCHIVE_SCHEMA_VERSION: u32 = 1;
const MAX_DEPENDENCIES: usize = 65_536;
const MAX_FILE_BYTES: u64 = 2 * 1024 * 1024 * 1024;
/// Ceiling on the sum of every entry. Without it a manifest can name 65,536
/// entries of 2 GiB each and still validate, so a crafted archive asks the
/// restorer to provision 128 TiB before any real work starts.
const MAX_TOTAL_BYTES: u64 = 16 * 1024 * 1024 * 1024;
const MAX_MANIFEST_BYTES: u64 = 16 * 1024 * 1024;
/// Ceiling on the packaged project document. A 1 GiB cap let a crafted
/// package ask for the whole buffer up front (`with_capacity` on the member
/// size) and then cost several times that again as a `serde_json::Value`,
/// which is what actually exhausted memory. 64 MiB is far above a real
/// project document and keeps the whole read bounded.
const MAX_PROJECT_JSON_BYTES: u64 = 64 * 1024 * 1024;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ArchiveEntry {
    pub source: String,
    pub package_path: String,
    pub bytes: u64,
    pub sha256: String,
    pub exists: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ArchiveManifest {
    pub schema_version: u32,
    pub project_file: String,
    pub entries: Vec<ArchiveEntry>,
    pub missing: Vec<String>,
    pub total_bytes: u64,
}

impl ArchiveManifest {
    pub fn validate(&self) -> bool {
        if self.schema_version != ARCHIVE_SCHEMA_VERSION
            || self.project_file.trim().is_empty()
            || self.entries.len() > MAX_DEPENDENCIES
            || self.missing.len() > MAX_DEPENDENCIES
        {
            return false;
        }
        let mut package_paths = BTreeSet::new();
        let mut sources = BTreeSet::new();
        let mut total = 0u64;
        for entry in &self.entries {
            if entry.source.trim().is_empty()
                || entry.package_path.trim().is_empty()
                || !safe_package_path(Path::new(&entry.package_path))
                || !sources.insert(entry.source.clone())
                || !package_paths.insert(entry.package_path.clone())
                || (entry.exists && (entry.sha256.len() != 64 || entry.bytes > MAX_FILE_BYTES))
                || (!entry.exists && (entry.bytes != 0 || !entry.sha256.is_empty()))
            {
                return false;
            }
            total = match total.checked_add(entry.bytes) {
                Some(value) => value,
                None => return false,
            };
            if total > MAX_TOTAL_BYTES {
                return false;
            }
        }
        let mut missing = BTreeSet::new();
        total == self.total_bytes
            && self.missing.iter().all(|path| {
                !path.trim().is_empty() && missing.insert(path) && !sources.contains(path)
            })
    }
}

#[derive(Debug, Clone)]
pub struct ProjectArchive;

#[path = "project_archive_impl.rs"]
mod impl_;
#[path = "project_archive_io.rs"]
mod io_;
use io_::*;

#[cfg(test)]
#[path = "project_archive_tests.rs"]
mod tests;

