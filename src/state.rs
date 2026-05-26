use anyhow::{Context, Result};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use crate::tool::ToolKind;
use crate::types::InstalledTool;

const DEFAULT_INSTALL_ROOT: &str = r"D:\Embedded_Toolchain";
const LEGACY_INSTALLS_DIR_NAME: &str = "tools";

pub fn default_install_root() -> PathBuf {
    PathBuf::from(DEFAULT_INSTALL_ROOT)
}

pub fn tool_versions_dir(root: &Path, kind: ToolKind) -> PathBuf {
    root.join(kind.id())
}

pub fn tool_version_dir(root: &Path, kind: ToolKind, version: &str) -> PathBuf {
    tool_versions_dir(root, kind).join(version)
}

pub fn legacy_tool_versions_dir(root: &Path, kind: ToolKind) -> PathBuf {
    root.join(LEGACY_INSTALLS_DIR_NAME).join(kind.id())
}

pub fn cleanup_staging_dirs(root: &Path) -> Result<Vec<PathBuf>> {
    let mut removed = Vec::new();
    for kind in ToolKind::all() {
        let kind_root = tool_versions_dir(root, kind);
        if !kind_root.exists() {
            continue;
        }

        for entry in fs::read_dir(&kind_root)
            .with_context(|| format!("failed to read {}", kind_root.display()))?
        {
            let entry = entry?;
            if !entry.file_type()?.is_dir() {
                continue;
            }

            let path = entry.path();
            let name = entry.file_name().to_string_lossy().to_string();
            if name.starts_with(".staging-") {
                fs::remove_dir_all(&path)
                    .with_context(|| format!("failed to remove {}", path.display()))?;
                removed.push(path);
            }
        }
    }

    Ok(removed)
}

pub fn discover_installed_tools(root: &Path) -> Result<Vec<InstalledTool>> {
    let mut tools = Vec::new();

    for kind in ToolKind::all() {
        if let Some(tool) = discover_installed_tool(root, kind)? {
            tools.push(tool);
        }
    }

    tools.sort_by_key(|tool| tool.kind.id().to_string());
    Ok(tools)
}

pub fn installed_tool_from_dir(
    kind: ToolKind,
    version: impl Into<String>,
    install_dir: PathBuf,
) -> Result<InstalledTool> {
    let executable_path =
        find_file_named(&install_dir, kind.executable_names()).with_context(|| {
            format!(
                "failed to locate {} in {}",
                kind.id(),
                install_dir.display()
            )
        })?;
    let executable_dir = executable_path
        .parent()
        .context("downloaded executable did not have a parent directory")?
        .to_path_buf();

    Ok(InstalledTool {
        kind,
        version: version.into(),
        executable_path,
        executable_dir,
    })
}

fn discover_installed_tool(root: &Path, kind: ToolKind) -> Result<Option<InstalledTool>> {
    let kind_root = tool_versions_dir(root, kind);
    if !kind_root.exists() {
        return Ok(None);
    }

    let mut candidates = Vec::new();
    for entry in fs::read_dir(&kind_root)
        .with_context(|| format!("failed to read {}", kind_root.display()))?
    {
        let entry = entry?;
        if !entry.file_type()?.is_dir() {
            continue;
        }

        let version = entry.file_name().to_string_lossy().to_string();
        if version.starts_with(".staging-") {
            continue;
        }

        let modified = entry
            .metadata()
            .and_then(|metadata| metadata.modified())
            .unwrap_or(SystemTime::UNIX_EPOCH);
        let install_dir = entry.path();

        if let Ok(tool) = installed_tool_from_dir(kind, version.clone(), install_dir) {
            candidates.push((modified, version, tool));
        }
    }

    candidates.sort_by(|left, right| right.0.cmp(&left.0).then_with(|| right.1.cmp(&left.1)));

    Ok(candidates.into_iter().next().map(|(_, _, tool)| tool))
}

fn find_file_named(root: &Path, candidates: &[&str]) -> Result<PathBuf> {
    let mut stack = vec![root.to_path_buf()];
    while let Some(directory) = stack.pop() {
        for entry in fs::read_dir(&directory)
            .with_context(|| format!("failed to read {}", directory.display()))?
        {
            let entry = entry?;
            let path = entry.path();
            let file_type = entry.file_type()?;
            if file_type.is_dir() {
                stack.push(path);
                continue;
            }

            let file_name = entry.file_name().to_string_lossy().to_string();
            if candidates
                .iter()
                .any(|candidate| file_name.eq_ignore_ascii_case(candidate))
            {
                return Ok(path);
            }
        }
    }

    anyhow::bail!(
        "none of {:?} were found below {}",
        candidates,
        root.display()
    )
}

#[cfg(test)]
mod tests {
    use super::{default_install_root, discover_installed_tools, tool_version_dir};
    use crate::tool::ToolKind;
    use std::fs;
    use std::path::PathBuf;
    use tempfile::tempdir;

    #[test]
    fn default_install_root_uses_embedded_toolchain_directory() {
        assert_eq!(
            default_install_root(),
            PathBuf::from(r"D:\Embedded_Toolchain")
        );
    }

    #[test]
    fn tool_versions_live_directly_under_the_install_root() {
        assert_eq!(
            tool_version_dir(
                &PathBuf::from(r"D:\Embedded_Toolchain"),
                ToolKind::Ninja,
                "1.12.0"
            ),
            PathBuf::from(r"D:\Embedded_Toolchain\ninja\1.12.0")
        );
    }

    #[test]
    fn discover_installed_tools_reads_tools_from_the_install_root() {
        let temp = tempdir().unwrap();
        let install_dir = temp.path().join("ninja").join("1.12.0");
        fs::create_dir_all(&install_dir).unwrap();
        fs::write(install_dir.join("ninja.exe"), b"").unwrap();
        fs::create_dir_all(temp.path().join("ninja").join(".staging-1.13.0")).unwrap();

        let tools = discover_installed_tools(temp.path()).unwrap();

        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0].kind, ToolKind::Ninja);
        assert_eq!(tools[0].version, "1.12.0");
        assert_eq!(tools[0].executable_path, install_dir.join("ninja.exe"));
    }
}
