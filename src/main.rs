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
        Commands::Install => {
            let request = cli::prompt_install_request()?;
            let client = resolver::build_client()?;
            installer::install_tools(&client, &request.root, &request.tools, request.scope).await?;
        }
    }

    Ok(())
}
