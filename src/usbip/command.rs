use std::{fs, path::Path};

use anyhow::{Context, Result, anyhow, bail};
use tokio::process::Command;

use crate::{
  device::{Attachment, UsbDevice, UsbDeviceMetadata},
  usbip::{BoxFuture, Usbip},
};

#[derive(Clone, Default)]
pub struct CommandUsbip;

impl CommandUsbip {
  pub fn new() -> Self {
    Self
  }
}

impl Usbip for CommandUsbip {
  fn ensure_daemon(&self) -> BoxFuture<'_, Result<()>> {
    Box::pin(async move {
      modprobe("usbip-host").await?;
      ensure_usbipd_running().await
    })
  }

  fn list_devices(&self) -> BoxFuture<'_, Result<Vec<UsbDevice>>> {
    Box::pin(async move { list_devices().await })
  }

  fn bind<'a>(&'a self, busid: &'a str) -> BoxFuture<'a, Result<()>> {
    Box::pin(async move { bind(busid).await })
  }

  fn unbind<'a>(&'a self, busid: &'a str) -> BoxFuture<'a, Result<()>> {
    Box::pin(async move { unbind(busid).await })
  }

  fn attach<'a>(&'a self, remote_host: &'a str, busid: &'a str) -> BoxFuture<'a, Result<()>> {
    Box::pin(async move {
      sudo_modprobe("vhci-hcd").await?;
      attach(remote_host, busid).await
    })
  }

  fn detach(&self, port: u16) -> BoxFuture<'_, Result<()>> {
    Box::pin(async move { detach(port).await })
  }

  fn list_attachments(&self) -> BoxFuture<'_, Result<Vec<Attachment>>> {
    Box::pin(async move { list_attachments().await })
  }
}

async fn modprobe(module: &str) -> Result<()> {
  let output = Command::new("modprobe")
    .args([module])
    .output()
    .await
    .context(format!("failed to enable kernel module {module}"))?;

  if output.status.success() {
    Ok(())
  } else {
    bail!(format!("failed to enable kernel module {module}"))
  }
}

async fn sudo_modprobe(module: &str) -> Result<()> {
  let output = Command::new("sudo")
    .args(["modprobe", module])
    .output()
    .await
    .context(format!("failed to enable kernel module {module} with sudo"))?;

  if output.status.success() {
    Ok(())
  } else {
    let stderr = String::from_utf8_lossy(&output.stderr);
    bail!("failed to enable kernel module {module} with sudo: {stderr}")
  }
}

async fn ensure_usbipd_running() -> Result<()> {
  let output = Command::new("pgrep")
    .args(["-x", "usbipd"])
    .output()
    .await
    .context("failed to check whether usbipd is running with pgrep")?;

  if output.status.success() {
    Ok(())
  } else {
    bail!("usbipd is not running; start usbipd before running the rusbee server")
  }
}

async fn list_devices() -> Result<Vec<UsbDevice>> {
  let output = run_usbip(["list", "-p", "-l"]).await?;
  let mut devices = parse_usbip_list_machine(&output);

  if devices.is_empty() {
    let output = run_usbip(["list", "-l"]).await?;
    devices = parse_usbip_list_human(&output);
  }

  for device in &mut devices {
    enrich_from_sysfs(device);
    device.bound = is_bound_to_usbip_host(&device.busid);
  }

  Ok(devices)
}

async fn bind(busid: &str) -> Result<()> {
  if is_bound_to_usbip_host(busid) {
    return Ok(());
  }

  run_usbip(["bind", "-b", busid]).await.map(|_| ())
}

async fn unbind(busid: &str) -> Result<()> {
  run_usbip(["unbind", "-b", busid]).await.map(|_| ())
}

async fn attach(remote_host: &str, busid: &str) -> Result<()> {
  run_sudo_usbip(["attach", "-r", remote_host, "-b", busid])
    .await
    .map(|_| ())
}

async fn detach(port: u16) -> Result<()> {
  run_sudo_usbip(["detach", "-p", &port.to_string()])
    .await
    .map(|_| ())
}

async fn list_attachments() -> Result<Vec<Attachment>> {
  let output = run_sudo_usbip(["port"]).await?;
  Ok(parse_usbip_port(&output))
}

fn parse_usbip_list_machine(output: &str) -> Vec<UsbDevice> {
  output
    .lines()
    .filter(|line| line.starts_with("busid="))
    .filter_map(|line| {
      let mut busid = String::new();
      let mut vendor_id = String::new();
      let mut product_id = String::new();
      let mut product = String::new();
      let mut manufacturer = String::new();

      for part in line.split('#') {
        let Some((key, value)) = part.split_once('=') else {
          continue;
        };

        match key {
          "busid" => busid = value.to_string(),
          "usbid" => {
            if let Some((vendor, product_value)) = value.split_once(':') {
              vendor_id = vendor.to_string();
              product_id = product_value.to_string();
            }
          }
          "product" => product = value.to_string(),
          "manufacturer" => manufacturer = value.to_string(),
          _ => {}
        }
      }

      if busid.is_empty() {
        return None;
      }

      let name = match (manufacturer.is_empty(), product.is_empty()) {
        (false, false) => format!("{manufacturer} {product}"),
        (false, true) => manufacturer.clone(),
        (true, false) => product.clone(),
        (true, true) => "Unknown USB device".to_string(),
      };

      Some(UsbDevice {
        busid,
        vendor_id,
        product_id,
        name,
        bound: false,
        attached: false,
        attached_port: None,
        metadata: UsbDeviceMetadata {
          manufacturer,
          product,
          ..Default::default()
        },
      })
    })
    .collect()
}

fn parse_usbip_list_human(output: &str) -> Vec<UsbDevice> {
  let mut devices = Vec::new();
  let mut pending_device: Option<UsbDevice> = None;

  for line in output.lines() {
    let trimmed = line.trim();

    if let Some(rest) = trimmed.strip_prefix("- busid ") {
      if let Some(device) = pending_device.take() {
        devices.push(device);
      }

      let Some((busid, ids)) = rest.split_once(' ') else {
        continue;
      };
      let ids = ids.trim_matches(['(', ')']);
      let (vendor_id, product_id) = ids
        .split_once(':')
        .map(|(vendor, product)| (vendor.to_string(), product.to_string()))
        .unwrap_or_default();

      pending_device = Some(UsbDevice {
        busid: busid.to_string(),
        vendor_id,
        product_id,
        name: "Unknown USB device".to_string(),
        bound: false,
        attached: false,
        attached_port: None,
        metadata: UsbDeviceMetadata::default(),
      });
    } else if let Some(device) = pending_device.as_mut()
      && !trimmed.is_empty()
      && !trimmed.starts_with(':')
    {
      device.name = trimmed.to_string();
      if let Some((manufacturer, product)) = trimmed.split_once(" : ") {
        device.metadata.manufacturer = manufacturer.to_string();
        device.metadata.product = product.to_string();
      }
    }
  }

  if let Some(device) = pending_device {
    devices.push(device);
  }

  devices
}

fn parse_usbip_port(output: &str) -> Vec<Attachment> {
  let mut attachments = Vec::new();
  let mut current_port = None;

  for line in output.lines() {
    let trimmed = line.trim();

    if let Some(rest) = trimmed.strip_prefix("Port ") {
      current_port = rest
        .split(':')
        .next()
        .and_then(|port| port.trim_start_matches('0').parse::<u16>().ok().or(Some(0)));
      continue;
    }

    let Some((_, remote_busid)) = trimmed.rsplit_once('/') else {
      continue;
    };

    if !trimmed.contains("usbip://") {
      continue;
    }

    if let Some(port) = current_port {
      attachments.push(Attachment {
        port,
        remote_busid: remote_busid.to_string(),
      });
    }
  }

  attachments
}

fn enrich_from_sysfs(device: &mut UsbDevice) {
  let path = Path::new("/sys/bus/usb/devices").join(&device.busid);

  if let Some(manufacturer) = read_sysfs_string(&path, "manufacturer") {
    device.metadata.manufacturer = manufacturer;
  }

  if let Some(product) = read_sysfs_string(&path, "product") {
    device.metadata.product = product;
  }

  device.metadata.serial = read_sysfs_string(&path, "serial");
  device.metadata.speed = read_sysfs_string(&path, "speed");
  device.metadata.usb_version = read_sysfs_string(&path, "version");
  device.metadata.device_version = read_sysfs_string(&path, "bcdDevice");
  device.metadata.busnum = read_sysfs_string(&path, "busnum");
  device.metadata.devnum = read_sysfs_string(&path, "devnum");
  device.metadata.device_class = read_sysfs_string(&path, "bDeviceClass");
  device.metadata.device_subclass = read_sysfs_string(&path, "bDeviceSubClass");
  device.metadata.device_protocol = read_sysfs_string(&path, "bDeviceProtocol");
  device.metadata.driver = read_driver_name(&path);

  device.name = match (
    device.metadata.manufacturer.is_empty(),
    device.metadata.product.is_empty(),
  ) {
    (false, false) => format!(
      "{} {}",
      device.metadata.manufacturer, device.metadata.product
    ),
    (false, true) => device.metadata.manufacturer.clone(),
    (true, false) => device.metadata.product.clone(),
    (true, true) => device.name.clone(),
  };
}

fn read_sysfs_string(path: &Path, name: &str) -> Option<String> {
  fs::read_to_string(path.join(name))
    .ok()
    .map(|value| value.trim().to_string())
    .filter(|value| !value.is_empty())
}

fn read_driver_name(path: &Path) -> Option<String> {
  fs::read_link(path.join("driver")).ok().and_then(|target| {
    target
      .file_name()
      .map(|name| name.to_string_lossy().into_owned())
  })
}

fn is_bound_to_usbip_host(busid: &str) -> bool {
  let driver_path = Path::new("/sys/bus/usb/devices").join(busid).join("driver");

  fs::read_link(driver_path)
    .ok()
    .and_then(|target| target.file_name().map(|name| name == "usbip-host"))
    .unwrap_or(false)
}

async fn run_usbip<const N: usize>(args: [&str; N]) -> Result<String> {
  run_command("usbip", args).await
}

async fn run_sudo_usbip<const N: usize>(args: [&str; N]) -> Result<String> {
  let output = Command::new("sudo")
    .arg("usbip")
    .args(args)
    .output()
    .await
    .context("failed to execute sudo usbip; is sudo and usbip installed?")?;

  if output.status.success() {
    String::from_utf8(output.stdout).context("usbip returned non-utf8 stdout")
  } else {
    let stderr = String::from_utf8_lossy(&output.stderr);
    Err(anyhow!("sudo usbip command failed: {stderr}"))
  }
}

async fn run_command<const N: usize>(command: &str, args: [&str; N]) -> Result<String> {
  let output = Command::new(command)
    .args(args)
    .output()
    .await
    .context(format!(
      "failed to execute {command}; is {command} installed?"
    ))?;

  if output.status.success() {
    String::from_utf8(output.stdout).context(format!("{command} returned non-utf8 stdout"))
  } else {
    let stderr = String::from_utf8_lossy(&output.stderr);
    Err(anyhow!("{command} command failed: {stderr}"))
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn parses_human_list_output() {
    let output = r#"
- busid 1-2 (046d:c52b)
   Logitech, Inc. : Unifying Receiver
"#;

    let devices = parse_usbip_list_human(output);

    assert_eq!(devices.len(), 1);
    assert_eq!(devices[0].busid, "1-2");
    assert_eq!(devices[0].vendor_id, "046d");
    assert_eq!(devices[0].product_id, "c52b");
    assert_eq!(devices[0].name, "Logitech, Inc. : Unifying Receiver");
  }

  #[test]
  fn parses_port_output() {
    let output = r#"
Imported USB devices
====================
Port 00: <Port in Use> at High Speed(480Mbps)
       Logitech, Inc. : Unifying Receiver (046d:c52b)
       3-1 -> usbip://192.168.1.10:3240/1-2
           -> remote bus/dev 001/002
"#;

    let attachments = parse_usbip_port(output);

    assert_eq!(attachments.len(), 1);
    assert_eq!(attachments[0].port, 0);
    assert_eq!(attachments[0].remote_busid, "1-2");
  }
}
