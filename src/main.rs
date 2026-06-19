mod cli;
mod config;
mod debate;
mod provider;
mod report;

use clap::Parser;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let app = cli::Cli::parse();
    match app.command {
        cli::Command::Debate(args) => cli::run_debate_cmd(args).await,
    }
}
