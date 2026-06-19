mod cli;
mod config;
mod debate;
mod provider;
mod report;
mod safety;
mod validate;

use clap::Parser;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let app = cli::Cli::parse();
    match app.command {
        cli::Command::Debate(args) => cli::run_debate_cmd(args).await,
        cli::Command::Validate(args) => cli::run_validate_cmd(args).await,
        cli::Command::Models(args) => cli::run_models_cmd(args).await,
    }
}
