mod cli;
mod environment;
mod installer;
mod resolver;
mod state;
mod tool;
mod types;

use anyhow::Result;
use clap::Parser;
use cli::{Cli, Commands};
use environment::{build_env_plan, preview_user_environment};
use resolver::{resolve_tool_options, resolve_tools};
use state::{cleanup_staging_dirs, default_install_root, discover_installed_tools};
use tool::ToolKind;

#[tokio::main]
async fn main() {
    if let Err(error) = run().await {
        eprintln!("error: {error:#}");
        std::process::exit(1);
    }
}

async fn run() -> Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Commands::Install(args) => {
            let request = cli::install_request(&args)?;
            let client = resolver::build_client()?;
            println!("Resolving tool versions...");
            let packages = if request.select_versions {
                let options = resolve_tool_options(&client, &request.tools, 6).await?;
                cli::choose_versions(&options)?
            } else {
                resolve_tools(&client, &request.tools).await?
            };
            installer::install_tools(&client, &request.root, packages, request.scope).await?;
        }
        Commands::Status(args) => {
            let root = cli::status_root(&args)?;
            print_status(&root)?;
        }
        Commands::Doctor(args) => {
            let root = cli::doctor_root(&args)?;
            run_doctor(&root).await?;
        }
    }

    Ok(())
}

fn print_status(root: &std::path::Path) -> Result<()> {
    println!("Install root: {}", root.display());
    let installed_tools = discover_installed_tools(root)?;
    if installed_tools.is_empty() {
        println!("Installed tools: none found.");
    } else {
        println!("Installed tools:");
        for tool in &installed_tools {
            println!(
                "- {} {} -> {}",
                tool.kind,
                tool.version,
                tool.executable_path.display()
            );
        }
    }

    let plan = build_env_plan(root, &installed_tools)?;
    let preview = preview_user_environment(root, &plan)?;
    println!("Managed PATH entries already present:");
    if preview.removed_path_entries.is_empty() {
        println!("- none");
    } else {
        for entry in &preview.removed_path_entries {
            println!("- {entry}");
        }
    }
    println!("Managed PATH entries missing from user PATH:");
    if preview.added_path_entries.is_empty() {
        println!("- none");
    } else {
        for entry in &preview.added_path_entries {
            println!("- {}", entry.display());
        }
    }
    Ok(())
}

async fn run_doctor(root: &std::path::Path) -> Result<()> {
    println!("Doctor checks:");
    print_check("Install root", installer::validate_install_root(root));

    let cleanup_result = cleanup_staging_dirs(root).map(|removed| {
        if removed.is_empty() {
            "no stale staging directories".to_string()
        } else {
            format!("removed {} stale staging directories", removed.len())
        }
    });
    match cleanup_result {
        Ok(message) => println!("[ok] Staging cleanup: {message}"),
        Err(error) => println!("[fail] Staging cleanup: {error:#}"),
    }

    let client = match resolver::build_client() {
        Ok(client) => {
            println!("[ok] HTTP client");
            client
        }
        Err(error) => {
            println!("[fail] HTTP client: {error:#}");
            return Ok(());
        }
    };

    match resolve_tools(&client, &ToolKind::all()).await {
        Ok(packages) => {
            println!("[ok] Release resolution");
            for package in packages {
                let checksum = if package.checksum.is_some() {
                    "checksum available"
                } else {
                    "no checksum advertised"
                };
                println!("- {} {} ({checksum})", package.kind, package.version);
            }
        }
        Err(error) => println!("[fail] Release resolution: {error:#}"),
    }

    let installed = discover_installed_tools(root)?;
    let plan = build_env_plan(root, &installed)?;
    match preview_user_environment(root, &plan) {
        Ok(preview) => {
            println!("[ok] User PATH registry access");
            println!(
                "- {} managed entries would be refreshed, {} entries would be added",
                preview.removed_path_entries.len(),
                preview.added_path_entries.len()
            );
        }
        Err(error) => println!("[fail] User PATH registry access: {error:#}"),
    }

    if root == default_install_root() {
        println!("Default root policy: Windows-only, D: drive by design.");
    }

    Ok(())
}

fn print_check<T>(label: &str, result: Result<T>) {
    match result {
        Ok(_) => println!("[ok] {label}"),
        Err(error) => println!("[fail] {label}: {error:#}"),
    }
}
