use std::path::PathBuf;

use crate::tool::ToolKind;

#[derive(Debug, Clone)]
pub struct ResolvedTool {
    pub kind: ToolKind,
    pub version: String,
    pub asset_name: String,
    pub download_url: String,
    pub size_bytes: Option<u64>,
}

#[derive(Debug, Clone)]
pub struct InstalledTool {
    pub kind: ToolKind,
    pub version: String,
    pub install_dir: PathBuf,
    pub executable_path: PathBuf,
    pub executable_dir: PathBuf,
}

#[derive(Debug, Clone)]
pub struct EnvPlan {
    pub variables: Vec<(String, String)>,
    pub path_entries: Vec<PathBuf>,
}
