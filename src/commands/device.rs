use anyhow::Result;
use serde::Serialize;
use tabled::Tabled;

use crate::{
  cli::{Context, OutputFormat},
  client::Client,
  device::{self, UsbDevice},
  output,
};

#[derive(Debug, clap::Args)]
pub struct DeviceArgs {
  #[command(subcommand)]
  pub command: DeviceCommand,
}

#[derive(clap::Subcommand, Debug)]
pub enum DeviceCommand {
  /// List server devices and mark those attached to this client.
  List,

  /// Bind a server-side USB device to usbip-host.
  Bind {
    #[arg(long)]
    busid: String,
  },

  /// Unbind a server-side USB device from usbip-host.
  Unbind {
    #[arg(long)]
    busid: String,
  },

  /// Bind a server-side USB device, then attach it to this client.
  Attach {
    #[arg(long)]
    busid: String,
  },

  /// Detach a server-side USB device from this client.
  Detach {
    #[arg(long)]
    busid: String,
  },
}

#[derive(Debug, Serialize, Tabled)]
struct DeviceRow {
  #[tabled(rename = "BUSID")]
  busid: String,
  #[tabled(rename = "USBID")]
  usbid: String,
  #[tabled(rename = "BOUND")]
  bound: bool,
  #[tabled(rename = "ATTACHED")]
  attached: bool,
  #[tabled(rename = "MANUFACTURER")]
  manufacturer: String,
  #[tabled(rename = "PRODUCT")]
  product: String,
}

impl From<UsbDevice> for DeviceRow {
  fn from(device: UsbDevice) -> Self {
    Self {
      usbid: usbid(&device),
      busid: device.busid,
      bound: device.bound,
      attached: device.attached,
      manufacturer: display_value(device.metadata.manufacturer),
      product: display_value(device.metadata.product),
    }
  }
}

pub async fn run(args: DeviceArgs, context: &Context) -> Result<()> {
  match args.command {
    DeviceCommand::List => list(context).await,
    DeviceCommand::Bind { busid } => bind(context, &busid).await,
    DeviceCommand::Unbind { busid } => unbind(context, &busid).await,
    DeviceCommand::Attach { busid } => attach(context, &busid).await,
    DeviceCommand::Detach { busid } => detach(context, &busid).await,
  }
}

async fn list(context: &Context) -> Result<()> {
  let devices = list_devices_with_local_attachments(context).await?;

  match context.global_opts.output {
    OutputFormat::Json => output::render_json(&devices),
    OutputFormat::Table => {
      let rows = devices.into_iter().map(DeviceRow::from).collect::<Vec<_>>();
      output::render_list(rows, "No devices found")
    }
  }
}

async fn list_devices_with_local_attachments(context: &Context) -> Result<Vec<UsbDevice>> {
  let mut devices = context.client.list_devices().await?;
  let attachments = context.usbip.list_attachments().await?;

  device::mark_attached(&mut devices, &attachments);

  Ok(devices)
}

async fn bind(context: &Context, busid: &str) -> Result<()> {
  let device = context.client.bind_device(busid).await?;
  log::info!("bound {} ({})", device.busid, device.name);
  Ok(())
}

async fn unbind(context: &Context, busid: &str) -> Result<()> {
  let device = context.client.unbind_device(busid).await?;
  log::info!("unbound {} ({})", device.busid, device.name);
  Ok(())
}

async fn attach(context: &Context, busid: &str) -> Result<()> {
  let mut device = list_devices_with_local_attachments(context)
    .await?
    .into_iter()
    .find(|device| device.busid == busid)
    .ok_or_else(|| anyhow::anyhow!("device {busid} was not found"))?;

  if device.attached {
    log::warn!(
      "device {} ({}) is already attached; skipping",
      device.busid,
      device.name
    );
    return Ok(());
  }

  if !device.bound {
    device = context.client.bind_device(busid).await?;
  }

  context.usbip.attach(&context.remote_host, busid).await?;
  log::info!("attached {} ({})", device.busid, device.name);
  Ok(())
}

async fn detach(context: &Context, busid: &str) -> Result<()> {
  let device = list_devices_with_local_attachments(context)
    .await?
    .into_iter()
    .find(|device| device.busid == busid)
    .ok_or_else(|| anyhow::anyhow!("device {busid} was not found"))?;

  let Some(port) = device.attached_port else {
    log::warn!(
      "device {} ({}) is not attached; skipping",
      device.busid,
      device.name
    );
    return Ok(());
  };

  context.usbip.detach(port).await?;
  log::info!(
    "detached {} ({}) from port {port}",
    device.busid,
    device.name
  );
  Ok(())
}

fn usbid(device: &UsbDevice) -> String {
  if device.vendor_id.is_empty() || device.product_id.is_empty() {
    "unknown".to_string()
  } else {
    format!("{}:{}", device.vendor_id, device.product_id)
  }
}

fn display_value(value: String) -> String {
  if value.is_empty() {
    "unknown".to_string()
  } else {
    value
  }
}
