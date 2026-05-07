use anyhow::{Context, Result, bail};
use clap::{Parser, Subcommand};
use dialoguer::{Confirm, Input, MultiSelect, theme::ColorfulTheme};
use std::io::{self, IsTerminal};
use std::path::PathBuf;

use crate::state::default_install_root;
use crate::tool::{EnvScope, ToolKind};

#[derive(Parser, Debug)]
#[command(
    name = "armup",
    bin_name = "armup",
    version,
    about = "Install embedded Cortex-M tools on Windows",
    arg_required_else_help = true
)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Commands,
}

#[derive(Subcommand, Debug)]
pub enum Commands {
    #[command(about = "Install tools")]
    Install,
}

pub struct InstallRequest {
    pub tools: Vec<ToolKind>,
    pub root: PathBuf,
    pub scope: EnvScope,
}

pub fn prompt_install_request() -> Result<InstallRequest> {
    Ok(InstallRequest {
        tools: choose_install_tools()?,
        root: choose_install_root()?,
        scope: choose_install_scope()?,
    })
}

fn choose_install_tools() -> Result<Vec<ToolKind>> {
    if !(io::stdin().is_terminal() && io::stdout().is_terminal()) {
        return Ok(ToolKind::all());
    }

    let theme = ColorfulTheme::default();
    let install_all = Confirm::with_theme(&theme)
        .with_prompt("Install all supported tools?")
        .default(true)
        .report(false)
        .interact()
        .context("failed to read interactive install selection")?;

    if install_all {
        println!("Installing all supported tools.");
        return Ok(ToolKind::all());
    }

    let all_tools = ToolKind::all();
    let labels = all_tools
        .iter()
        .map(|tool| tool.picker_label())
        .collect::<Vec<_>>();
    println!("Use Space to select tools, then press Enter to confirm.");
    let selections = MultiSelect::with_theme(&theme)
        .with_prompt("Select tools to install")
        .items(&labels)
        .report(false)
        .interact()
        .context("failed to read tool checklist selection")?;

    if selections.is_empty() {
        bail!("no tools selected");
    }

    let chosen = selections
        .into_iter()
        .map(|index| all_tools[index])
        .collect::<Vec<_>>();
    println!(
        "Selected tools: {}",
        chosen
            .iter()
            .map(|tool| tool.id())
            .collect::<Vec<_>>()
            .join(", ")
    );
    Ok(chosen)
}

fn choose_install_root() -> Result<PathBuf> {
    let default_root = default_install_root();
    if !(io::stdin().is_terminal() && io::stdout().is_terminal()) {
        return Ok(default_root);
    }

    let theme = ColorfulTheme::default();
    println!("Enter the install root. Both / and \\ are accepted.");
    println!("Press Enter to use the default: {}", default_root.display());
    let raw = Input::<String>::with_theme(&theme)
        .with_prompt("Install root")
        .default(default_root.display().to_string())
        .report(false)
        .interact_text()
        .context("failed to read install root path")?;

    let parsed = parse_root_path(&raw).map_err(anyhow::Error::msg)?;
    println!("Install root: {}", parsed.display());
    Ok(parsed)
}

fn choose_install_scope() -> Result<EnvScope> {
    if !(io::stdin().is_terminal() && io::stdout().is_terminal()) {
        return Ok(EnvScope::User);
    }

    let theme = ColorfulTheme::default();
    let apply = Confirm::with_theme(&theme)
        .with_prompt("Add the installed tools to HKCU\\Environment and PATH?")
        .default(true)
        .report(false)
        .interact()
        .context("failed to read environment setup selection")?;

    if apply {
        println!("Environment setup: apply to current user profile.");
        Ok(EnvScope::User)
    } else {
        println!("Environment setup: skip registry changes.");
        Ok(EnvScope::None)
    }
}

pub(crate) fn parse_root_path(raw: &str) -> std::result::Result<PathBuf, String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err("root path cannot be empty".to_string());
    }

    #[cfg(windows)]
    {
        let normalized = trimmed.replace('/', "\\");
        return Ok(PathBuf::from(normalized));
    }

    #[cfg(not(windows))]
    {
        Ok(PathBuf::from(trimmed))
    }
}

#[cfg(test)]
mod tests {
    use super::parse_root_path;
    use std::path::PathBuf;

    #[test]
    fn parse_root_path_rejects_empty_strings() {
        assert!(parse_root_path("   ").is_err());
    }

    #[test]
    fn parse_root_path_keeps_forward_slashes_usable() {
        let parsed = parse_root_path("D:/Embedded/armup").unwrap();
        assert_eq!(parsed, PathBuf::from(r"D:\Embedded\armup"));
    }

    #[test]
    fn parse_root_path_keeps_backslashes_usable() {
        let parsed = parse_root_path(r"D:\Embedded\armup").unwrap();
        assert_eq!(parsed, PathBuf::from(r"D:\Embedded\armup"));
    }

    #[test]
    fn parse_root_path_accepts_mixed_separators() {
        let parsed = parse_root_path(r"D:/Embedded\armup").unwrap();
        assert_eq!(parsed, PathBuf::from(r"D:\Embedded\armup"));
    }
}
