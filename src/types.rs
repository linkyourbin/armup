use std::path::PathBuf;

use crate::tool::ToolKind;

#[derive(Debug, Clone)]
pub struct ResolvedTool {
    pub kind: ToolKind,
    pub version: String,
    pub asset_name: String,
    pub download_url: String,
    pub checksum: Option<ArchiveChecksum>,
}

#[derive(Debug, Clone)]
pub struct ToolVersionOptions {
    pub kind: ToolKind,
    pub releases: Vec<ResolvedTool>,
}

#[derive(Debug, Clone)]
pub struct ArchiveChecksum {
    pub algorithm: ChecksumAlgorithm,
    pub value: String,
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum ChecksumAlgorithm {
    Sha256,
}

#[derive(Debug, Clone)]
pub struct InstalledTool {
    pub kind: ToolKind,
    pub version: String,
    pub executable_path: PathBuf,
    pub executable_dir: PathBuf,
}

#[derive(Debug, Clone)]
pub struct EnvPlan {
    pub path_entries: Vec<PathBuf>,
}

#[derive(Debug, Clone)]
pub struct EnvPreview {
    pub removed_path_entries: Vec<String>,
    pub added_path_entries: Vec<PathBuf>,
    pub final_path_entry_count: usize,
    pub legacy_variables_present: Vec<String>,
}
