use anyhow::{Context, Result, anyhow, bail};
use indicatif::HumanBytes;
use reqwest::Client;
use reqwest::StatusCode;
use reqwest::header::{ACCEPT_RANGES, RANGE};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Seek, Write};
use std::path::{Component, Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tempfile::{NamedTempFile, TempPath};
use tokio::task::JoinSet;
use tokio::time::{sleep, timeout};
use zip::ZipArchive;

use crate::environment::{apply_user_environment, build_env_plan};
use crate::state::{
    cleanup_staging_dirs, discover_installed_tools, installed_tool_from_dir, tool_version_dir,
    tool_versions_dir,
};
use crate::tool::EnvScope;
use crate::types::{ArchiveChecksum, ChecksumAlgorithm, InstalledTool, ResolvedTool};

const MULTIPART_THRESHOLD_BYTES: u64 = 128 * 1024 * 1024;
const MULTIPART_TARGET_PART_SIZE_BYTES: u64 = 16 * 1024 * 1024;
const MULTIPART_MAX_PARTS: u64 = 12;
const DOWNLOAD_IDLE_TIMEOUT: Duration = Duration::from_secs(60);
const RANGE_DOWNLOAD_MAX_ATTEMPTS: u32 = 8;
const PROGRESS_RENDER_INTERVAL: Duration = Duration::from_millis(500);
const PROGRESS_BAR_WIDTH: usize = 28;

#[derive(Clone)]
struct ProgressReporter {
    inner: Arc<Mutex<ProgressReporterState>>,
}

struct ProgressReporterState {
    downloads: BTreeMap<String, DownloadSnapshot>,
    last_render: Instant,
    rendered_lines: usize,
}

struct DownloadSnapshot {
    total: u64,
    downloaded: u64,
    started: Instant,
}

#[derive(Clone)]
struct DownloadTracker {
    reporter: ProgressReporter,
    label: String,
}

pub async fn install_tools(
    client: &Client,
    root: &Path,
    packages: Vec<ResolvedTool>,
    scope: EnvScope,
) -> Result<()> {
    validate_install_root(root)?;
    let cleaned = cleanup_staging_dirs(root)?;
    for path in cleaned {
        println!("Removed stale staging directory: {}", path.display());
    }

    let progress = ProgressReporter::new();
    let mut tasks = JoinSet::new();
    for package in packages {
        let client = client.clone();
        let root = root.to_path_buf();
        let progress = progress.clone();
        tasks.spawn(async move { install_one(&client, &root, &package, &progress).await });
    }

    let install_result = async {
        while let Some(result) = tasks.join_next().await {
            result.context("tool install task failed")??;
        }
        Ok::<(), anyhow::Error>(())
    }
    .await;
    progress.clear();
    install_result?;

    let installed_tools = discover_installed_tools(root)?;

    let plan = build_env_plan(root, &installed_tools)?;
    if scope == EnvScope::User {
        print_env_preview(root, &plan)?;
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
        println!("Updated user PATH.");
        println!("Open a new terminal to pick up the changes.");
    } else {
        println!("Skipped PATH registry changes.");
    }

    Ok(())
}

pub fn validate_install_root(root: &Path) -> Result<()> {
    if root.as_os_str().is_empty() {
        bail!("install root cannot be empty");
    }

    if let Some(prefix) = root.components().next() {
        if let Component::Prefix(prefix) = prefix {
            let drive_root = PathBuf::from(format!("{}\\", prefix.as_os_str().to_string_lossy()));
            if !drive_root.exists() {
                bail!(
                    "install drive {} does not exist. Create the drive or pass --root with an existing drive.",
                    drive_root.display()
                );
            }
        }
    }

    Ok(())
}

fn print_env_preview(root: &Path, plan: &crate::types::EnvPlan) -> Result<()> {
    let preview = crate::environment::preview_user_environment(root, plan)?;
    println!("PATH update preview:");
    if preview.removed_path_entries.is_empty()
        && preview.added_path_entries.is_empty()
        && preview.legacy_variables_present.is_empty()
    {
        println!("- No PATH changes needed.");
    } else {
        for entry in &preview.removed_path_entries {
            println!("- Remove managed old PATH entry: {entry}");
        }
        for entry in &preview.added_path_entries {
            println!("- Add PATH entry: {}", entry.display());
        }
        for variable in &preview.legacy_variables_present {
            println!("- Remove legacy environment variable: {variable}");
        }
    }
    println!(
        "- Final user PATH entries: {}",
        preview.final_path_entry_count
    );
    Ok(())
}

async fn install_one(
    client: &Client,
    root: &Path,
    package: &ResolvedTool,
    progress: &ProgressReporter,
) -> Result<InstalledTool> {
    let install_dir = tool_version_dir(root, package.kind, &package.version);

    if install_dir.exists() {
        return installed_tool_from_dir(package.kind, package.version.clone(), install_dir);
    }

    let archive_path = download_archive(
        client,
        &package.download_url,
        &package.asset_name,
        package.kind.id(),
        progress,
    )
    .await?;
    verify_archive_checksum(archive_path.as_ref(), package)?;

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

fn verify_archive_checksum(archive_path: &Path, package: &ResolvedTool) -> Result<()> {
    let Some(checksum) = &package.checksum else {
        println!(
            "Checksum: no upstream checksum available for {} {}.",
            package.kind.id(),
            package.version
        );
        return Ok(());
    };

    match checksum.algorithm {
        ChecksumAlgorithm::Sha256 => verify_sha256(archive_path, checksum).with_context(|| {
            format!(
                "checksum verification failed for {} {}",
                package.kind.id(),
                package.version
            )
        })?,
    }

    println!(
        "Checksum: verified {} for {} {}.",
        checksum.algorithm.label(),
        package.kind.id(),
        package.version
    );
    Ok(())
}

fn verify_sha256(path: &Path, checksum: &ArchiveChecksum) -> Result<()> {
    let mut file =
        File::open(path).with_context(|| format!("failed to open {}", path.display()))?;
    let mut hasher = Sha256::new();
    let mut buffer = [0u8; 64 * 1024];

    loop {
        let read = file
            .read(&mut buffer)
            .with_context(|| format!("failed to read {}", path.display()))?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }

    let actual = format!("{:x}", hasher.finalize());
    if actual != checksum.value {
        bail!(
            "expected sha256 {}, got {} for {}",
            checksum.value,
            actual,
            path.display()
        );
    }

    Ok(())
}

impl ChecksumAlgorithm {
    fn label(self) -> &'static str {
        match self {
            Self::Sha256 => "sha256",
        }
    }
}

async fn download_archive(
    client: &Client,
    url: &str,
    asset_name: &str,
    tool_label: &str,
    progress: &ProgressReporter,
) -> Result<TempPath> {
    if let Some((content_length, multipart_parts)) =
        probe_multipart_download(client, url, asset_name).await?
    {
        match download_archive_multipart(
            client,
            url,
            asset_name,
            tool_label,
            content_length,
            multipart_parts,
            progress,
        )
        .await
        {
            Ok(path) => return Ok(path),
            Err(error) => {
                progress_log(
                    progress,
                    format!(
                        "warning: parallel download failed after range retries for {asset_name}: {error:#}. Falling back to a single connection."
                    ),
                );
            }
        }
    }

    download_archive_single(client, url, asset_name, tool_label, progress).await
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

async fn download_archive_single(
    client: &Client,
    url: &str,
    asset_name: &str,
    tool_label: &str,
    progress: &ProgressReporter,
) -> Result<TempPath> {
    let mut response = client
        .get(url)
        .send()
        .await
        .with_context(|| format!("failed to download {url}"))?
        .error_for_status()
        .with_context(|| format!("download returned an error for {url}"))?;

    let total = response.content_length().unwrap_or(0);
    let tracker = progress.start_download(tool_label, total);

    let mut temp = NamedTempFile::new().context("failed to create a temporary download file")?;
    let result = async {
        loop {
            let chunk = timeout(DOWNLOAD_IDLE_TIMEOUT, response.chunk())
                .await
                .with_context(|| {
                    format!(
                        "download stalled for {asset_name}: no data received for {} seconds",
                        DOWNLOAD_IDLE_TIMEOUT.as_secs()
                    )
                })?
                .with_context(|| {
                    format!("failed while reading downloaded data for {asset_name}")
                })?;
            let Some(chunk) = chunk else {
                break;
            };
            temp.write_all(&chunk)
                .context("failed to write downloaded archive chunk")?;
            tracker.inc(chunk.len() as u64);
        }
        Ok::<(), anyhow::Error>(())
    }
    .await;

    result?;
    progress.finish_download(tool_label);
    Ok(temp.into_temp_path())
}

async fn download_archive_multipart(
    client: &Client,
    url: &str,
    _asset_name: &str,
    tool_label: &str,
    content_length: u64,
    parts: u64,
    progress: &ProgressReporter,
) -> Result<TempPath> {
    let tracker = progress.start_download(tool_label, content_length);

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
        let tracker = tracker.clone();
        tasks.spawn(async move {
            download_range_to_file(&client, &url, start, end, &part_path, &tracker).await?;
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

    let path = result?;
    progress.finish_download(tool_label);
    Ok(path)
}

async fn download_range_to_file(
    client: &Client,
    url: &str,
    start: u64,
    end: u64,
    part_path: &Path,
    progress: &DownloadTracker,
) -> Result<()> {
    File::create(part_path).with_context(|| format!("failed to create {}", part_path.display()))?;

    let expected_len = end - start + 1;
    let mut written = 0;
    let mut attempts = 0;
    let mut last_error = None;

    while written < expected_len {
        attempts += 1;
        let request_start = start + written;
        let range_value = format!("bytes={request_start}-{end}");
        let response = client.get(url).header(RANGE, range_value).send().await;
        let mut response = match response {
            Ok(response) => match response.error_for_status() {
                Ok(response) => response,
                Err(error) => {
                    last_error = Some(anyhow!(
                        "download range returned an error for bytes {request_start}-{end}: {error}"
                    ));
                    if attempts >= RANGE_DOWNLOAD_MAX_ATTEMPTS {
                        break;
                    }
                    sleep(range_retry_delay(attempts)).await;
                    continue;
                }
            },
            Err(error) => {
                last_error = Some(anyhow!(
                    "failed to request download range {request_start}-{end}: {error}"
                ));
                if attempts >= RANGE_DOWNLOAD_MAX_ATTEMPTS {
                    break;
                }
                sleep(range_retry_delay(attempts)).await;
                continue;
            }
        };

        if response.status() != StatusCode::PARTIAL_CONTENT {
            bail!(
                "server ignored ranged request for bytes {request_start}-{end} and returned {}",
                response.status()
            );
        }

        let mut output = OpenOptions::new()
            .append(true)
            .open(part_path)
            .with_context(|| format!("failed to open {}", part_path.display()))?;

        loop {
            let chunk = match timeout(DOWNLOAD_IDLE_TIMEOUT, response.chunk()).await {
                Ok(Ok(Some(chunk))) => chunk,
                Ok(Ok(None)) => {
                    if written < expected_len {
                        last_error = Some(anyhow!(
                            "download range {request_start}-{end} ended early after {} of {} bytes",
                            written,
                            expected_len
                        ));
                    }
                    break;
                }
                Ok(Err(error)) => {
                    last_error = Some(anyhow!(
                        "failed while reading byte range {request_start}-{end}: {error}"
                    ));
                    break;
                }
                Err(_) => {
                    last_error = Some(anyhow!(
                        "download stalled for byte range {request_start}-{end}: no data received for {} seconds",
                        DOWNLOAD_IDLE_TIMEOUT.as_secs()
                    ));
                    break;
                }
            };

            output
                .write_all(&chunk)
                .with_context(|| format!("failed to write {}", part_path.display()))?;
            written += chunk.len() as u64;
            progress.inc(chunk.len() as u64);

            if written >= expected_len {
                break;
            }
        }

        if written >= expected_len {
            return Ok(());
        }

        if attempts >= RANGE_DOWNLOAD_MAX_ATTEMPTS {
            break;
        }

        sleep(range_retry_delay(attempts)).await;
    }

    Err(last_error.unwrap_or_else(|| {
        anyhow!(
            "download range {start}-{end} incomplete after {} attempts",
            RANGE_DOWNLOAD_MAX_ATTEMPTS
        )
    }))
    .with_context(|| {
        format!(
            "failed to complete byte range {start}-{end} after {} attempts",
            RANGE_DOWNLOAD_MAX_ATTEMPTS
        )
    })
}

fn range_retry_delay(attempt: u32) -> Duration {
    Duration::from_millis(250 * attempt.min(4) as u64)
}

impl ProgressReporter {
    fn new() -> Self {
        let now = Instant::now();
        Self {
            inner: Arc::new(Mutex::new(ProgressReporterState {
                downloads: BTreeMap::new(),
                last_render: now.checked_sub(PROGRESS_RENDER_INTERVAL).unwrap_or(now),
                rendered_lines: 0,
            })),
        }
    }

    fn start_download(&self, label: &str, total: u64) -> DownloadTracker {
        let mut state = self.inner.lock().expect("progress reporter lock poisoned");
        state.downloads.insert(
            label.to_string(),
            DownloadSnapshot {
                total,
                downloaded: 0,
                started: Instant::now(),
            },
        );
        render_progress_dashboard(&mut state, true);

        DownloadTracker {
            reporter: self.clone(),
            label: label.to_string(),
        }
    }

    fn inc_download(&self, label: &str, amount: u64) {
        let mut state = self.inner.lock().expect("progress reporter lock poisoned");
        if let Some(download) = state.downloads.get_mut(label) {
            download.downloaded = download.downloaded.saturating_add(amount);
        }
        render_progress_dashboard(&mut state, false);
    }

    fn finish_download(&self, label: &str) {
        let mut state = self.inner.lock().expect("progress reporter lock poisoned");
        if let Some(download) = state.downloads.get_mut(label) {
            if download.total == 0 {
                download.total = download.downloaded;
            } else {
                download.downloaded = download.total;
            }
        }
        render_progress_dashboard(&mut state, true);
    }

    fn clear(&self) {
        let mut state = self.inner.lock().expect("progress reporter lock poisoned");
        clear_rendered_dashboard(&mut state);
    }

    fn log(&self, message: impl AsRef<str>) {
        let mut state = self.inner.lock().expect("progress reporter lock poisoned");
        clear_rendered_dashboard(&mut state);
        println!("{}", message.as_ref());
        render_progress_dashboard(&mut state, true);
    }
}

impl DownloadTracker {
    fn inc(&self, amount: u64) {
        self.reporter.inc_download(&self.label, amount);
    }
}

fn render_progress_dashboard(state: &mut ProgressReporterState, force: bool) {
    if state.downloads.is_empty() {
        clear_rendered_dashboard(state);
        return;
    }
    if !force && state.last_render.elapsed() < PROGRESS_RENDER_INTERVAL {
        return;
    }
    state.last_render = Instant::now();

    clear_rendered_dashboard(state);
    for (label, download) in &state.downloads {
        println!("{}", format_progress_line(label, download));
    }
    state.rendered_lines = state.downloads.len();
    let _ = io::stdout().flush();
}

fn clear_rendered_dashboard(state: &mut ProgressReporterState) {
    if state.rendered_lines == 0 {
        return;
    }

    print!("\x1b[{}F", state.rendered_lines);
    for _ in 0..state.rendered_lines {
        print!("\x1b[2K\x1b[1E");
    }
    print!("\x1b[{}F", state.rendered_lines);
    let _ = io::stdout().flush();
    state.rendered_lines = 0;
}

fn format_progress_line(label: &str, download: &DownloadSnapshot) -> String {
    let percent = if download.total > 0 {
        download
            .downloaded
            .saturating_mul(100)
            .checked_div(download.total)
            .unwrap_or(0)
            .min(100)
    } else {
        0
    };
    let elapsed = download.started.elapsed().as_secs_f64().max(0.001);
    let bytes_per_second = (download.downloaded as f64 / elapsed) as u64;
    let eta = format_eta(download.total, download.downloaded, bytes_per_second);

    format!(
        "{label:>18} [{}] {percent:>3}% {}/{} {}/s eta {eta}",
        progress_bar(download.downloaded, download.total, PROGRESS_BAR_WIDTH),
        HumanBytes(download.downloaded),
        HumanBytes(download.total),
        HumanBytes(bytes_per_second)
    )
}

fn progress_bar(downloaded: u64, total: u64, width: usize) -> String {
    if width == 0 {
        return String::new();
    }
    if total == 0 {
        return "-".repeat(width);
    }

    let filled = downloaded
        .saturating_mul(width as u64)
        .checked_div(total)
        .unwrap_or(0)
        .min(width as u64) as usize;

    if filled >= width {
        return "=".repeat(width);
    }

    let mut bar = String::with_capacity(width);
    bar.push_str(&"=".repeat(filled));
    bar.push('>');
    bar.push_str(&"-".repeat(width - filled - 1));
    bar
}

fn format_eta(total: u64, downloaded: u64, bytes_per_second: u64) -> String {
    if total == 0 || downloaded >= total {
        return "0s".to_string();
    }
    if bytes_per_second == 0 {
        return "?".to_string();
    }

    let remaining_seconds = total.saturating_sub(downloaded).div_ceil(bytes_per_second);
    format_duration(Duration::from_secs(remaining_seconds))
}

fn format_duration(duration: Duration) -> String {
    let seconds = duration.as_secs();
    let hours = seconds / 3600;
    let minutes = (seconds % 3600) / 60;
    let seconds = seconds % 60;

    if hours > 0 {
        format!("{hours}h{minutes}m")
    } else if minutes > 0 {
        format!("{minutes}m{seconds}s")
    } else {
        format!("{seconds}s")
    }
}

fn progress_log(progress: &ProgressReporter, message: impl AsRef<str>) {
    progress.log(message);
}

fn assemble_archive_from_parts(part_paths: Vec<(u64, PathBuf)>) -> Result<TempPath> {
    let mut archive = NamedTempFile::new().context("failed to create assembled archive")?;
    for (_, path) in part_paths {
        let mut part =
            File::open(&path).with_context(|| format!("failed to open {}", path.display()))?;
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
