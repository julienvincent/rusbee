pub mod command;

use std::{future::Future, pin::Pin};

use anyhow::Result;

use crate::device::{Attachment, UsbDevice};

pub use command::CommandUsbip;

pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

pub trait Usbip: Send + Sync {
  fn ensure_daemon(&self) -> BoxFuture<'_, Result<()>>;
  fn list_devices(&self) -> BoxFuture<'_, Result<Vec<UsbDevice>>>;
  fn bind<'a>(&'a self, busid: &'a str) -> BoxFuture<'a, Result<()>>;
  fn unbind<'a>(&'a self, busid: &'a str) -> BoxFuture<'a, Result<()>>;
  fn attach<'a>(&'a self, remote_host: &'a str, busid: &'a str) -> BoxFuture<'a, Result<()>>;
  fn detach(&self, port: u16) -> BoxFuture<'_, Result<()>>;
  fn list_attachments(&self) -> BoxFuture<'_, Result<Vec<Attachment>>>;
}
