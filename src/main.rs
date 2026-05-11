use std::sync::Arc;

use anyhow::Result;
use clap::{CommandFactory, Parser};
use rusbee::{cli, commands, helpers, tui, usbip::CommandUsbip};

#[derive(Parser, Debug)]
#[command(name = "rusbee")]
#[command(about = "Remote usbip device management")]
struct Cli {
  #[clap(flatten)]
  global_opts: cli::GlobalOpts,

  #[command(subcommand)]
  command: Option<cli::Command>,
}

async fn run(command: Option<cli::Command>, global_opts: cli::GlobalOpts) -> Result<()> {
  match command {
    None => {
      let context = cli::Context::new(global_opts)?;
      let _sudo_session_keeper = helpers::start_sudo_session_keeper().await?;
      tui::run(
        context.client.clone(),
        context.usbip.clone(),
        context.remote_host.clone(),
      )
      .await
    }
    Some(cli::Command::Server(args)) => {
      commands::server::run(args, Arc::new(CommandUsbip::new())).await
    }
    Some(cli::Command::Device(args)) => {
      let context = cli::Context::new(global_opts)?;
      commands::device::run(args, &context).await
    }
    Some(cli::Command::Completions { shell }) => {
      clap_complete::generate(shell, &mut Cli::command(), "rusbee", &mut std::io::stdout());
      Ok(())
    }
  }
}

#[tokio::main]
async fn main() {
  let cli = Cli::parse();

  let mut log_builder = env_logger::builder();
  log_builder
    .format_timestamp(None)
    .format_target(false)
    .filter_level(cli.global_opts.log_level.unwrap_or(log::LevelFilter::Info));
  log_builder.init();

  let command = cli.command;
  if let Err(error) = run(command, cli.global_opts).await {
    eprintln!("Error: {error:#}");
    std::process::exit(1);
  }
}
