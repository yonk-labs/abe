mod cli;
mod config;
mod debate;
mod init;
mod mcp;
mod persona;
mod provider;
mod report;
mod safety;
mod server;
mod validate;

use clap::Parser;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let app = cli::Cli::parse();
    match app.command {
        cli::Command::Debate(args) => cli::run_debate_cmd(args).await,
        cli::Command::Validate(args) => cli::run_validate_cmd(args).await,
        cli::Command::Models(args) => cli::run_models_cmd(args).await,
        cli::Command::Personas => {
            cli::run_personas_cmd();
            Ok(())
        }
        cli::Command::Mcp(args) => mcp::serve(args.config).await,
        cli::Command::Serve(args) => server::serve(args.config, &args.host, args.port).await,
        cli::Command::Init => init::run().await,
    }
}
