use anyhow::{Context, Result, bail};
use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use winreg::RegKey;
use winreg::enums::HKEY_CURRENT_USER;

use crate::state::{legacy_tool_versions_dir, tool_versions_dir};
use crate::tool::ToolKind;
use crate::types::{EnvPlan, InstalledTool};

pub fn build_env_plan(root: &Path, tools: &[InstalledTool]) -> Result<EnvPlan> {
    let mut variables = vec![("ARMUP_HOME".to_string(), root.display().to_string())];
    let mut path_entries = Vec::new();

    for tool in tools {
        variables.push((
            tool.kind.root_env_var().to_string(),
            tool.install_dir.display().to_string(),
        ));
        path_entries.push(tool.executable_dir.clone());

        if tool.kind == ToolKind::XpackOpenocd {
            let scripts_dir = find_directory_named(&tool.install_dir, "scripts")
                .context("failed to locate OpenOCD scripts directory")?;
            variables.push((
                "OPENOCD_SCRIPTS".to_string(),
                scripts_dir.display().to_string(),
            ));
        }
    }

    Ok(EnvPlan {
        variables,
        path_entries: dedup_paths(path_entries),
    })
}

pub fn apply_user_environment(root: &Path, plan: &EnvPlan) -> Result<()> {
    let hkcu = RegKey::predef(HKEY_CURRENT_USER);
    let (environment, _) = hkcu
        .create_subkey("Environment")
        .context("failed to open HKCU\\Environment")?;

    for (key, value) in &plan.variables {
        environment
            .set_value(key, value)
            .with_context(|| format!("failed to write {key} to HKCU\\Environment"))?;
    }

    let existing_path: String = environment.get_value("Path").unwrap_or_default();
    let merged_path = merge_user_path(root, &existing_path, &plan.path_entries);
    environment
        .set_value("Path", &merged_path)
        .context("failed to update HKCU\\Environment\\Path")?;

    Ok(())
}

fn dedup_paths(paths: Vec<PathBuf>) -> Vec<PathBuf> {
    let mut seen = HashSet::new();
    let mut deduped = Vec::new();
    for path in paths {
        let key = normalize_path_key(&path);
        if seen.insert(key) {
            deduped.push(path);
        }
    }
    deduped
}

fn merge_user_path(root: &Path, current_path: &str, additions: &[PathBuf]) -> String {
    let managed_roots: Vec<String> = ToolKind::all()
        .into_iter()
        .flat_map(|kind| {
            [
                tool_versions_dir(root, kind),
                legacy_tool_versions_dir(root, kind),
            ]
        })
        .map(|path| normalize_path_key(&path))
        .collect();
    let mut entries = Vec::new();
    let mut seen = HashSet::new();

    for raw in current_path.split(';') {
        let trimmed = raw.trim();
        if trimmed.is_empty() {
            continue;
        }

        let normalized = normalize_string_key(trimmed);
        if managed_roots
            .iter()
            .any(|managed| normalized.starts_with(managed))
        {
            continue;
        }

        if seen.insert(normalized) {
            entries.push(trimmed.to_string());
        }
    }

    for addition in additions {
        let rendered = addition.display().to_string();
        let normalized = normalize_path_key(addition);
        if seen.insert(normalized) {
            entries.push(rendered);
        }
    }

    entries.join(";")
}

fn normalize_path_key(path: &Path) -> String {
    normalize_string_key(&path.to_string_lossy())
}

fn normalize_string_key(value: &str) -> String {
    value
        .replace('/', "\\")
        .trim_end_matches('\\')
        .to_ascii_lowercase()
}

fn find_directory_named(root: &Path, candidate: &str) -> Result<PathBuf> {
    let mut stack = vec![root.to_path_buf()];
    while let Some(directory) = stack.pop() {
        for entry in fs::read_dir(&directory)
            .with_context(|| format!("failed to read {}", directory.display()))?
        {
            let entry = entry?;
            let path = entry.path();
            let file_type = entry.file_type()?;
            if file_type.is_dir() {
                let name = entry.file_name().to_string_lossy().to_string();
                if name.eq_ignore_ascii_case(candidate) {
                    return Ok(path);
                }
                stack.push(path);
            }
        }
    }

    bail!(
        "directory {candidate:?} was not found below {}",
        root.display()
    )
}
