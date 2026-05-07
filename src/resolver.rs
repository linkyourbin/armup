use anyhow::{Context, Result};
use reqwest::Client;
use reqwest::header::{ACCEPT, HeaderMap, HeaderValue, USER_AGENT};
use serde::Deserialize;
use std::collections::HashSet;
use std::time::Duration;
use tokio::task::JoinSet;

use crate::tool::ToolKind;
use crate::types::ResolvedTool;

const ARM_GUIDE_PAGE: &str = "https://learn.arm.com/install-guides/gcc/arm-gnu/";
const ARM_DOWNLOAD_BASE: &str = "https://developer.arm.com/-/media/Files/downloads/gnu";

#[derive(Debug, Deserialize)]
struct GitHubRelease {
    tag_name: String,
    assets: Vec<GitHubAsset>,
}

#[derive(Debug, Deserialize)]
struct GitHubAsset {
    name: String,
    browser_download_url: String,
    size: u64,
}

pub fn build_client() -> Result<Client> {
    let mut headers = HeaderMap::new();
    headers.insert(USER_AGENT, HeaderValue::from_static("armup/0.1.0"));
    headers.insert(
        ACCEPT,
        HeaderValue::from_static("application/vnd.github+json, text/html;q=0.9, */*;q=0.8"),
    );

    Client::builder()
        .default_headers(headers)
        .connect_timeout(Duration::from_secs(15))
        .build()
        .context("failed to build HTTP client")
}

pub async fn resolve_tools(client: &Client, tools: &[ToolKind]) -> Result<Vec<ResolvedTool>> {
    let mut tasks = JoinSet::new();
    for &tool in tools {
        println!("Resolving latest release for {}...", tool.id());
        let client = client.clone();
        tasks.spawn(async move { resolve_tool(&client, tool).await });
    }

    let mut resolved = Vec::with_capacity(tools.len());
    while let Some(result) = tasks.join_next().await {
        resolved.push(result.context("tool resolution task failed")??);
    }

    resolved.sort_by_key(|tool| {
        tools
            .iter()
            .position(|candidate| *candidate == tool.kind)
            .unwrap_or(usize::MAX)
    });
    Ok(resolved)
}

async fn resolve_tool(client: &Client, tool: ToolKind) -> Result<ResolvedTool> {
    match tool {
        ToolKind::ArmNoneEabiGcc => resolve_arm_toolchain(client).await,
        ToolKind::Clangd => {
            resolve_github_latest(client, tool, "clangd", "clangd", normalize_version).await
        }
        ToolKind::Cmake => {
            resolve_github_latest(client, tool, "Kitware", "CMake", normalize_version).await
        }
        ToolKind::Ninja => {
            resolve_github_latest(client, tool, "ninja-build", "ninja", normalize_version).await
        }
        ToolKind::XpackOpenocd => {
            resolve_github_latest(
                client,
                tool,
                "xpack-dev-tools",
                "openocd-xpack",
                normalize_version,
            )
            .await
        }
    }
}

async fn resolve_github_latest(
    client: &Client,
    tool: ToolKind,
    owner: &str,
    repo: &str,
    normalize: fn(&str) -> String,
) -> Result<ResolvedTool> {
    let api_url = format!("https://api.github.com/repos/{owner}/{repo}/releases/latest");
    let release = client
        .get(&api_url)
        .timeout(Duration::from_secs(30))
        .send()
        .await
        .with_context(|| format!("failed to query GitHub release API for {owner}/{repo}"))?
        .error_for_status()
        .with_context(|| format!("GitHub release API returned an error for {owner}/{repo}"))?
        .json::<GitHubRelease>()
        .await
        .with_context(|| {
            format!("failed to parse GitHub release API response for {owner}/{repo}")
        })?;

    let asset = release
        .assets
        .into_iter()
        .find(|asset| tool.matches_github_asset(&asset.name))
        .with_context(|| format!("no matching Windows zip asset found for {}", tool.id()))?;

    Ok(ResolvedTool {
        kind: tool,
        version: normalize(&release.tag_name),
        asset_name: asset.name,
        download_url: asset.browser_download_url,
        size_bytes: Some(asset.size),
    })
}

async fn resolve_arm_toolchain(client: &Client) -> Result<ResolvedTool> {
    let page = client
        .get(ARM_GUIDE_PAGE)
        .timeout(Duration::from_secs(30))
        .send()
        .await
        .context("failed to query the Arm GNU toolchain install guide")?
        .error_for_status()
        .context("Arm GNU toolchain install guide returned an error")?
        .text()
        .await
        .context("failed to read the Arm GNU toolchain install guide")?;

    let version = extract_arm_version(&page)
        .context("could not find the latest Arm GNU toolchain version on the Arm install guide")?;
    let asset_name = format!("arm-gnu-toolchain-{version}-mingw-w64-x86_64-arm-none-eabi.zip");
    let download_url = format!("{ARM_DOWNLOAD_BASE}/{version}/binrel/{asset_name}");

    Ok(ResolvedTool {
        kind: ToolKind::ArmNoneEabiGcc,
        version,
        asset_name,
        download_url,
        size_bytes: None,
    })
}

fn extract_arm_version(page: &str) -> Option<String> {
    let prefix = "arm-gnu-toolchain-";
    let mut offset = 0;
    let mut seen = HashSet::new();

    while let Some(found) = page[offset..].find(prefix) {
        let start = offset + found + prefix.len();
        let remainder = &page[start..];
        let Some(end) = remainder.find('-') else {
            offset = start;
            continue;
        };
        let candidate = &remainder[..end];
        offset = start;

        if candidate.contains('<') || candidate.contains('>') {
            continue;
        }
        if candidate.len() > 24 || candidate.is_empty() {
            continue;
        }
        let candidate = candidate.to_ascii_lowercase();
        if !candidate
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '.' | '-'))
        {
            continue;
        }
        if !looks_like_arm_version(&candidate) {
            continue;
        }
        if seen.insert(candidate.clone()) {
            return Some(candidate);
        }
    }

    None
}

fn looks_like_arm_version(candidate: &str) -> bool {
    let Some((base, rel)) = candidate.split_once(".rel") else {
        return false;
    };
    if base.is_empty() || rel.is_empty() {
        return false;
    }
    if !rel.chars().all(|ch| ch.is_ascii_digit()) {
        return false;
    }
    let segments: Vec<&str> = base.split('.').collect();
    segments.len() >= 2
        && segments
            .iter()
            .all(|segment| !segment.is_empty() && segment.chars().all(|ch| ch.is_ascii_digit()))
}

fn normalize_version(tag: &str) -> String {
    tag.trim_start_matches(['v', 'V']).to_string()
}
