use anyhow::{Context, Result, bail};
use indicatif::{ProgressBar, ProgressStyle};
use reqwest::Client;
use reqwest::StatusCode;
use reqwest::header::{ACCEPT_RANGES, RANGE};
use std::fs::{self, File};
use std::io::{self, Read, Seek, Write};
use std::path::{Component, Path, PathBuf};
use std::time::Duration;
use tempfile::{NamedTempFile, TempPath};
use tokio::task::JoinSet;
use zip::ZipArchive;

use crate::environment::{apply_user_environment, build_env_plan};
use crate::resolver::resolve_tools;
use crate::state::{
    discover_installed_tools, installed_tool_from_dir, tool_version_dir, tool_versions_dir,
};
use crate::tool::{EnvScope, ToolKind};
use crate::types::{InstalledTool, ResolvedTool};

const MULTIPART_THRESHOLD_BYTES: u64 = 128 * 1024 * 1024;
const MULTIPART_TARGET_PART_SIZE_BYTES: u64 = 16 * 1024 * 1024;
const MULTIPART_MAX_PARTS: u64 = 12;

pub async fn install_tools(
    client: &Client,
    root: &Path,
    tools: &[ToolKind],
    scope: EnvScope,
) -> Result<()> {
    println!("Resolving tool versions...");
    let resolved = resolve_tools(client, tools).await?;

    let (exclusive_packages, parallel_packages): (Vec<_>, Vec<_>) = resolved
        .into_iter()
        .partition(requires_exclusive_install);

    for package in exclusive_packages {
        println!("Preparing {} {}...", package.kind.id(), package.version);
        install_one(client, root, &package).await?;
    }

    let mut tasks = JoinSet::new();
    for package in parallel_packages {
        let client = client.clone();
        let root = root.to_path_buf();
        tasks.spawn(async move {
            println!("Preparing {} {}...", package.kind.id(), package.version);
            install_one(&client, &root, &package).await
        });
    }

    while let Some(result) = tasks.join_next().await {
        result.context("tool install task failed")??;
    }

    let installed_tools = discover_installed_tools(root)?;

    let plan = build_env_plan(root, &installed_tools)?;
    if scope == EnvScope::User {
        apply_user_environment(root, &plan)?;
    }

    println!("Installed tools under {}", root.display());
    for tool in &installed_tools {
        println!(
            "- {} {} -> {}",
            tool.kind,
            tool.version,
            tool.executable_path.display()
        );
    }

    if scope == EnvScope::User {
        println!("Updated HKCU\\Environment and PATH.");
        println!("Open a new terminal to pick up the changes.");
    } else {
        println!("Skipped environment registry changes.");
    }

    Ok(())
}

fn requires_exclusive_install(package: &ResolvedTool) -> bool {
    package.kind == ToolKind::ArmNoneEabiGcc
        || package
            .size_bytes
            .is_some_and(|size| size >= MULTIPART_THRESHOLD_BYTES)
}

async fn install_one(
    client: &Client,
    root: &Path,
    package: &ResolvedTool,
) -> Result<InstalledTool> {
    let install_dir = tool_version_dir(root, package.kind, &package.version);

    if install_dir.exists() {
        println!(
            "Using existing install at {}",
            install_dir.display()
        );
        return installed_tool_from_dir(package.kind, package.version.clone(), install_dir);
    }

    let archive_path = download_archive(client, &package.download_url, &package.asset_name).await?;

    let tool_root = tool_versions_dir(root, package.kind);
    fs::create_dir_all(&tool_root)
        .with_context(|| format!("failed to create {}", tool_root.display()))?;
    let staging_dir = tool_root.join(format!(".staging-{}", package.version));
    if staging_dir.exists() {
        fs::remove_dir_all(&staging_dir)
            .with_context(|| format!("failed to remove {}", staging_dir.display()))?;
    }
    fs::create_dir_all(&staging_dir)
        .with_context(|| format!("failed to create {}", staging_dir.display()))?;

    extract_zip_archive(archive_path.as_ref(), &staging_dir)?;

    if let Some(parent) = install_dir.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("failed to create {}", parent.display()))?;
    }
    fs::rename(&staging_dir, &install_dir).with_context(|| {
        format!(
            "failed to move {} into {}",
            staging_dir.display(),
            install_dir.display()
        )
    })?;

    installed_tool_from_dir(package.kind, package.version.clone(), install_dir)
}

async fn download_archive(client: &Client, url: &str, asset_name: &str) -> Result<TempPath> {
    if let Some((content_length, multipart_parts)) =
        probe_multipart_download(client, url, asset_name).await?
    {
        println!(
            "Using {multipart_parts} parallel connections for {asset_name} ({})",
            indicatif::HumanBytes(content_length)
        );
        match download_archive_multipart(client, url, asset_name, content_length, multipart_parts).await
        {
            Ok(path) => return Ok(path),
            Err(error) => {
                eprintln!(
                    "warning: parallel download failed for {asset_name}: {error:#}. Falling back to a single connection."
                );
            }
        }
    }

    download_archive_single(client, url, asset_name).await
}

async fn probe_multipart_download(
    client: &Client,
    url: &str,
    _asset_name: &str,
) -> Result<Option<(u64, u64)>> {
    let response = match client
        .head(url)
        .timeout(Duration::from_secs(20))
        .send()
        .await
    {
        Ok(response) => response,
        Err(_) => return Ok(None),
    };
    if let Ok(response) = response.error_for_status() {
        let supports_ranges = response
            .headers()
            .get(ACCEPT_RANGES)
            .and_then(|value| value.to_str().ok())
            .map(|value| value.eq_ignore_ascii_case("bytes"))
            .unwrap_or(false);
        if supports_ranges {
            if let Some(content_length) = response.content_length() {
                if let Some(result) = multipart_plan_for_length(content_length) {
                    return Ok(Some(result));
                }
            }
        }
    }

    let probe = match client
        .get(url)
        .header(RANGE, "bytes=0-0")
        .timeout(Duration::from_secs(20))
        .send()
        .await
    {
        Ok(response) => response,
        Err(_) => return Ok(None),
    };

    if probe.status() != StatusCode::PARTIAL_CONTENT {
        return Ok(None);
    }

    let Some(total) = probe
        .headers()
        .get("content-range")
        .and_then(|value| value.to_str().ok())
        .and_then(parse_content_range_total)
    else {
        return Ok(None);
    };

    Ok(multipart_plan_for_length(total))
}

fn multipart_plan_for_length(content_length: u64) -> Option<(u64, u64)> {
    if content_length < MULTIPART_THRESHOLD_BYTES {
        return None;
    }

    let suggested_parts = content_length.div_ceil(MULTIPART_TARGET_PART_SIZE_BYTES);
    let multipart_parts = suggested_parts.clamp(2, MULTIPART_MAX_PARTS);
    Some((content_length, multipart_parts))
}

fn parse_content_range_total(header: &str) -> Option<u64> {
    let (_, total) = header.split_once('/')?;
    total.parse().ok()
}

async fn download_archive_single(client: &Client, url: &str, asset_name: &str) -> Result<TempPath> {
    let mut response = client
        .get(url)
        .send()
        .await
        .with_context(|| format!("failed to download {url}"))?
        .error_for_status()
        .with_context(|| format!("download returned an error for {url}"))?;

    let total = response.content_length().unwrap_or(0);
    let progress = (total >= MULTIPART_THRESHOLD_BYTES).then(|| {
        let progress = ProgressBar::new(total);
        progress.set_style(
            ProgressStyle::with_template(
                "{spinner:.green} {msg} [{bar:32.cyan/blue}] {percent:>3}% {binary_bytes}/{total_bytes} {binary_bytes_per_sec} eta {eta}",
            )
            .expect("valid progress bar template")
            .progress_chars("=>-"),
        );
        progress.set_message(format!("Downloading {asset_name}"));
        progress
    });
    if progress.is_none() {
        println!("Downloading {asset_name}...");
    }

    let mut temp = NamedTempFile::new().context("failed to create a temporary download file")?;
    while let Some(chunk) = response.chunk().await? {
        temp.write_all(&chunk)
            .context("failed to write downloaded archive chunk")?;
        if let Some(progress) = &progress {
            progress.inc(chunk.len() as u64);
        }
    }

    if let Some(progress) = progress {
        progress.finish_and_clear();
    } else {
        println!("Finished {asset_name}.");
    }
    Ok(temp.into_temp_path())
}

async fn download_archive_multipart(
    client: &Client,
    url: &str,
    asset_name: &str,
    content_length: u64,
    parts: u64,
) -> Result<TempPath> {
    let progress = ProgressBar::new(content_length);
    progress.set_style(
        ProgressStyle::with_template(
            "{spinner:.green} {msg} [{bar:32.cyan/blue}] {percent:>3}% {binary_bytes}/{total_bytes} {binary_bytes_per_sec} eta {eta}",
        )?
        .progress_chars("=>-"),
    );
    progress.set_message(format!("Downloading {asset_name}"));

    let temp_dir = tempfile::tempdir().context("failed to create temporary part directory")?;
    let mut tasks = JoinSet::new();
    let part_size = content_length.div_ceil(parts);

    for index in 0..parts {
        let start = index * part_size;
        if start >= content_length {
            break;
        }
        let end = (start + part_size).min(content_length) - 1;
        let client = client.clone();
        let url = url.to_string();
        let part_path = temp_dir.path().join(format!("part-{index:02}.bin"));
        let progress = progress.clone();
        tasks.spawn(async move {
            download_range_to_file(&client, &url, start, end, &part_path, &progress).await?;
            Ok::<(u64, PathBuf), anyhow::Error>((index, part_path))
        });
    }

    let result = async {
        let mut part_paths = Vec::new();
        while let Some(result) = tasks.join_next().await {
            part_paths.push(result.context("parallel download task failed")??);
        }
        part_paths.sort_by_key(|(index, _)| *index);

        assemble_archive_from_parts(part_paths)
    }
    .await;

    progress.finish_and_clear();
    result
}

async fn download_range_to_file(
    client: &Client,
    url: &str,
    start: u64,
    end: u64,
    part_path: &Path,
    progress: &ProgressBar,
) -> Result<()> {
    let range_value = format!("bytes={start}-{end}");
    let mut response = client
        .get(url)
        .header(RANGE, range_value)
        .send()
        .await
        .with_context(|| format!("failed to request download range {start}-{end}"))?
        .error_for_status()
        .with_context(|| format!("download range returned an error for bytes {start}-{end}"))?;

    if response.status() != StatusCode::PARTIAL_CONTENT {
        bail!(
            "server ignored ranged request for bytes {start}-{end} and returned {}",
            response.status()
        );
    }

    let mut output =
        File::create(part_path).with_context(|| format!("failed to create {}", part_path.display()))?;
    while let Some(chunk) = response.chunk().await? {
        output
            .write_all(&chunk)
            .with_context(|| format!("failed to write {}", part_path.display()))?;
        progress.inc(chunk.len() as u64);
    }

    Ok(())
}

fn assemble_archive_from_parts(part_paths: Vec<(u64, PathBuf)>) -> Result<TempPath> {
    let mut archive = NamedTempFile::new().context("failed to create assembled archive")?;
    for (_, path) in part_paths {
        let mut part = File::open(&path)
            .with_context(|| format!("failed to open {}", path.display()))?;
        io::copy(&mut part, archive.as_file_mut())
            .with_context(|| format!("failed to append {}", path.display()))?;
    }
    Ok(archive.into_temp_path())
}

fn extract_zip_archive(archive_path: &Path, destination: &Path) -> Result<()> {
    let file = File::open(archive_path)
        .with_context(|| format!("failed to open {}", archive_path.display()))?;
    let mut archive = ZipArchive::new(file)
        .with_context(|| format!("failed to read zip archive {}", archive_path.display()))?;
    let prefix = archive_common_prefix(&mut archive)?;

    for index in 0..archive.len() {
        let mut entry = archive.by_index(index)?;
        let raw_name = entry.name().replace('\\', "/");
        let stripped = strip_optional_prefix(&raw_name, prefix.as_deref());
        if stripped.is_empty() {
            continue;
        }

        let relative = sanitize_archive_path(&stripped).with_context(|| {
            format!("archive entry {raw_name:?} contained an invalid path component")
        })?;
        let output_path = destination.join(relative);

        if entry.is_dir() {
            fs::create_dir_all(&output_path)
                .with_context(|| format!("failed to create {}", output_path.display()))?;
            continue;
        }

        if let Some(parent) = output_path.parent() {
            fs::create_dir_all(parent)
                .with_context(|| format!("failed to create {}", parent.display()))?;
        }
        let mut output = File::create(&output_path)
            .with_context(|| format!("failed to create {}", output_path.display()))?;
        io::copy(&mut entry, &mut output)
            .with_context(|| format!("failed to extract {}", output_path.display()))?;
    }

    Ok(())
}

fn archive_common_prefix<R: Read + Seek>(archive: &mut ZipArchive<R>) -> Result<Option<String>> {
    let mut prefix: Option<String> = None;

    for index in 0..archive.len() {
        let entry = archive.by_index(index)?;
        let normalized = entry.name().replace('\\', "/");
        let trimmed = normalized.trim_matches('/');
        if trimmed.is_empty() {
            continue;
        }

        let mut segments = trimmed.split('/');
        let first = segments.next().unwrap_or_default();
        if segments.next().is_none() {
            return Ok(None);
        }

        match &prefix {
            Some(existing) if existing != first => return Ok(None),
            None => prefix = Some(first.to_string()),
            _ => {}
        }
    }

    Ok(prefix)
}

fn strip_optional_prefix(path: &str, prefix: Option<&str>) -> String {
    match prefix {
        Some(prefix) => path
            .strip_prefix(prefix)
            .and_then(|value| value.strip_prefix('/'))
            .unwrap_or(path)
            .to_string(),
        None => path.to_string(),
    }
}

fn sanitize_archive_path(path: &str) -> Result<PathBuf> {
    let mut sanitized = PathBuf::new();
    for component in Path::new(path).components() {
        match component {
            Component::Normal(part) => sanitized.push(part),
            Component::CurDir => {}
            _ => bail!("unsupported path component"),
        }
    }
    Ok(sanitized)
}
