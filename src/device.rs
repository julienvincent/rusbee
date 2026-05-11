use std::collections::HashMap;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct UsbDeviceMetadata {
  pub manufacturer: String,
  pub product: String,
  pub serial: Option<String>,
  pub speed: Option<String>,
  pub usb_version: Option<String>,
  pub device_version: Option<String>,
  pub busnum: Option<String>,
  pub devnum: Option<String>,
  pub device_class: Option<String>,
  pub device_subclass: Option<String>,
  pub device_protocol: Option<String>,
  pub driver: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UsbDevice {
  pub busid: String,
  pub vendor_id: String,
  pub product_id: String,
  pub name: String,
  pub bound: bool,
  pub attached: bool,
  pub attached_port: Option<u16>,
  pub metadata: UsbDeviceMetadata,
}

#[derive(Debug, Clone)]
pub struct Attachment {
  pub port: u16,
  pub remote_busid: String,
}

pub fn mark_attached(devices: &mut [UsbDevice], attachments: &[Attachment]) {
  let attached_ports = attachments
    .iter()
    .map(|attachment| (attachment.remote_busid.as_str(), attachment.port))
    .collect::<HashMap<_, _>>();

  for device in devices {
    device.attached_port = attached_ports.get(device.busid.as_str()).copied();
    device.attached = device.attached_port.is_some();
  }
}
