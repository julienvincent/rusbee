use anyhow::{Context as AnyhowContext, Result, bail};
use reqwest::Url;
use serde::{Serialize, de::DeserializeOwned};

use crate::{
  client::{BoxFuture, Client},
  device::UsbDevice,
};

#[derive(Clone)]
pub struct HttpClient {
  http: reqwest::Client,
  server: String,
  token: Option<String>,
}

impl HttpClient {
  pub fn new(server: impl Into<String>, token: Option<String>) -> Self {
    Self {
      http: reqwest::Client::new(),
      server: server.into(),
      token,
    }
  }

  async fn request<B, T>(&self, path: &str, body: &B) -> Result<T>
  where
    B: Serialize + ?Sized,
    T: DeserializeOwned,
  {
    let url = self.endpoint(path)?;
    let mut request = self.http.post(url).json(body);

    if let Some(token) = &self.token {
      request = request.bearer_auth(token);
    }

    let response = request
      .send()
      .await
      .context("failed to call rusbee server")?;

    if !response.status().is_success() {
      bail!(
        "server returned {}: {}",
        response.status(),
        response.text().await?
      );
    }

    response
      .json::<T>()
      .await
      .context("failed to decode server response")
  }

  fn endpoint(&self, path: &str) -> Result<Url> {
    let mut url = Url::parse(&self.server).context("server must be a valid http URL")?;
    url
      .path_segments_mut()
      .map_err(|_| anyhow::anyhow!("server URL cannot be a base URL"))?
      .clear()
      .extend(path.split('/'));
    Ok(url)
  }
}

impl Client for HttpClient {
  fn list_devices(&self) -> BoxFuture<'_, Result<Vec<UsbDevice>>> {
    Box::pin(async move { self.request("device/list", &serde_json::json!({})).await })
  }

  fn bind_device<'a>(&'a self, busid: &'a str) -> BoxFuture<'a, Result<UsbDevice>> {
    Box::pin(async move {
      self
        .request("device/bind", &serde_json::json!({ "busid": busid }))
        .await
    })
  }

  fn unbind_device<'a>(&'a self, busid: &'a str) -> BoxFuture<'a, Result<UsbDevice>> {
    Box::pin(async move {
      self
        .request("device/unbind", &serde_json::json!({ "busid": busid }))
        .await
    })
  }
}
