use std::sync::Arc;

use clap::ValueEnum;
use inquire::Select;

use crate::{
  client::HttpClient,
  commands::{device, server},
  config::{self, ResolvedTarget},
  helpers,
  usbip::{CommandUsbip, Usbip},
};

#[derive(Debug, clap::Args)]
pub struct GlobalOpts {
  /// Rusbee server URL.
  #[arg(long, global = true)]
  pub server: Option<String>,

  /// Named target from config.
  #[arg(long, global = true)]
  pub target: Option<String>,

  /// Bearer token for rusbee server requests.
  #[arg(long, global = true)]
  pub token: Option<String>,

  #[arg(long, global = true)]
  pub log_level: Option<log::LevelFilter>,

  #[arg(long, short = 'o', global = true, default_value = "table")]
  pub output: OutputFormat,
}

#[derive(ValueEnum, Clone, Debug, Default)]
pub enum OutputFormat {
  #[default]
  Table,
  Json,
}

pub struct Context {
  pub global_opts: GlobalOpts,
  pub client: HttpClient,
  pub usbip: Arc<dyn Usbip>,
  pub remote_host: String,
}

impl Context {
  pub fn new(global_opts: GlobalOpts) -> anyhow::Result<Self> {
    let target = resolve_client_target(&global_opts)?;
    Self::with_server(global_opts, target)
  }

  fn with_server(global_opts: GlobalOpts, target: ResolvedTarget) -> anyhow::Result<Self> {
    let client = HttpClient::new(target.address.clone(), target.token);
    let remote_host = helpers::server_host(&target.address)?;

    Ok(Self {
      global_opts,
      client,
      usbip: Arc::new(CommandUsbip::new()),
      remote_host,
    })
  }
}

fn resolve_client_target(global_opts: &GlobalOpts) -> anyhow::Result<ResolvedTarget> {
  let config = config::load()?;
  let target_name = match global_opts.target.as_deref() {
    Some(target_name) => Some(target_name.to_string()),
    None if config.targets.len() > 1 => {
      let target_names = config.targets.keys().cloned().collect::<Vec<_>>();
      Some(Select::new("Select rusbee target", target_names).prompt()?)
    }
    None => None,
  };

  config::resolve_target(
    &config,
    target_name.as_deref(),
    global_opts.server.as_deref(),
    global_opts.token.as_deref(),
  )
}

#[derive(clap::Subcommand, Debug)]
pub enum Command {
  /// Run the HTTP server on the host with physical USB devices.
  Server(server::ServerArgs),

  /// Manage USB devices.
  Device(device::DeviceArgs),

  /// Generate shell completions.
  Completions {
    #[arg(value_enum)]
    shell: clap_complete::Shell,
  },
}
