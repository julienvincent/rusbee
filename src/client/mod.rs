pub mod http;

use std::{future::Future, pin::Pin};

use anyhow::Result;

use crate::device::UsbDevice;

pub use http::HttpClient;

pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

pub trait Client: Send + Sync {
  fn list_devices(&self) -> BoxFuture<'_, Result<Vec<UsbDevice>>>;
  fn bind_device<'a>(&'a self, busid: &'a str) -> BoxFuture<'a, Result<UsbDevice>>;
  fn unbind_device<'a>(&'a self, busid: &'a str) -> BoxFuture<'a, Result<UsbDevice>>;
}
