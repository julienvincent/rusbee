use std::{
  net::{IpAddr, Ipv4Addr, SocketAddr},
  sync::Arc,
};

use anyhow::{Result, bail};
use axum::{
  Json, Router,
  body::Body,
  extract::State,
  http::{Request, StatusCode, header},
  middleware::{self, Next},
  response::{IntoResponse, Response},
  routing::{get, post},
};
use serde::Deserialize;

use crate::{config, device::UsbDevice, usbip::Usbip};

#[derive(Clone)]
struct AppState {
  usbip: Arc<dyn Usbip>,
  auth: ServerAuth,
}

#[derive(Clone, Debug)]
pub enum ServerAuth {
  Disabled,
  Bearer(String),
}

#[derive(Debug, clap::Args)]
pub struct ServerArgs {
  #[arg(long)]
  pub host: Option<IpAddr>,

  #[arg(long)]
  pub port: Option<u16>,

  /// Bearer token required for all server requests.
  #[arg(long)]
  pub token: Option<String>,

  /// Disable server authentication.
  #[arg(long)]
  pub no_auth: bool,
}

pub async fn run(args: ServerArgs, usbip: Arc<dyn Usbip>) -> Result<()> {
  crate::helpers::ensure_running_as_root()?;
  usbip.ensure_daemon().await?;

  let config = config::load_for_root()?;
  let resolved = ResolvedServerArgs::resolve(args, config.server);

  let address = SocketAddr::new(resolved.host, resolved.port);
  let listener = tokio::net::TcpListener::bind(address).await?;
  log::info!("rusbee server listening on http://{address}");

  axum::serve(listener, router(usbip, resolved.auth()?)).await?;
  Ok(())
}

struct ResolvedServerArgs {
  host: IpAddr,
  port: u16,
  token: Option<String>,
  no_auth: bool,
}

impl ResolvedServerArgs {
  fn resolve(args: ServerArgs, server_config: Option<config::ServerConfig>) -> Self {
    let config_host = server_config.as_ref().and_then(|server| server.host);
    let config_port = server_config.as_ref().and_then(|server| server.port);
    let config_token = server_config.and_then(|server| server.token);

    Self {
      host: args
        .host
        .or(config_host)
        .unwrap_or(IpAddr::V4(Ipv4Addr::LOCALHOST)),
      port: args.port.or(config_port).unwrap_or(7878),
      token: args.token.or(config_token),
      no_auth: args.no_auth,
    }
  }

  fn auth(&self) -> Result<ServerAuth> {
    if self.no_auth {
      if self.token.is_some() {
        bail!("--token and --no-auth cannot be used together");
      }

      return Ok(ServerAuth::Disabled);
    }

    let Some(token) = &self.token else {
      bail!(
        "server authentication requires a token; pass --token, set [server].token in config, or use --no-auth"
      );
    };

    Ok(ServerAuth::Bearer(token.clone()))
  }
}

pub fn router(usbip: Arc<dyn Usbip>, auth: ServerAuth) -> Router {
  let state = AppState { usbip, auth };

  Router::new()
    .route("/livez", get(livez))
    .route("/device/list", post(devices))
    .route("/device/bind", post(bind))
    .route("/device/unbind", post(unbind))
    .layer(middleware::from_fn_with_state(
      state.clone(),
      authenticate_request,
    ))
    .layer(middleware::from_fn(log_request))
    .with_state(state)
}

async fn livez() -> &'static str {
  "ok"
}

async fn log_request(request: Request<Body>, next: Next) -> Response {
  let method = request.method().clone();
  let path = request.uri().path().to_string();
  let response = next.run(request).await;

  if response.status().is_server_error() {
    log::error!("{} {} -> {}", method, path, response.status());
  } else {
    log::info!("{} {} -> {}", method, path, response.status());
  }

  response
}

async fn authenticate_request(
  State(state): State<AppState>,
  request: Request<Body>,
  next: Next,
) -> Response {
  let ServerAuth::Bearer(token) = &state.auth else {
    return next.run(request).await;
  };

  let expected = format!("Bearer {token}");
  let authorized = request
    .headers()
    .get(header::AUTHORIZATION)
    .and_then(|header| header.to_str().ok())
    == Some(expected.as_str());

  if authorized {
    next.run(request).await
  } else {
    StatusCode::UNAUTHORIZED.into_response()
  }
}

async fn devices(State(state): State<AppState>) -> Result<Json<Vec<UsbDevice>>, ApiError> {
  Ok(Json(state.usbip.list_devices().await?))
}

#[derive(Deserialize)]
struct DeviceRequest {
  busid: String,
}

async fn bind(
  State(state): State<AppState>,
  Json(request): Json<DeviceRequest>,
) -> Result<Json<UsbDevice>, ApiError> {
  let Json(device) = find_device(state.usbip.clone(), &request.busid).await?;
  log::info!("Attempting to bind {}", device.busid);
  state.usbip.bind(&device.busid).await?;
  find_device(state.usbip, &request.busid).await
}

async fn unbind(
  State(state): State<AppState>,
  Json(request): Json<DeviceRequest>,
) -> Result<Json<UsbDevice>, ApiError> {
  let Json(device) = find_device(state.usbip.clone(), &request.busid).await?;
  log::info!("Attempting to unbind {}", device.busid);
  state.usbip.unbind(&device.busid).await?;
  find_device(state.usbip, &request.busid).await
}

async fn find_device(usbip: Arc<dyn Usbip>, busid: &str) -> Result<Json<UsbDevice>, ApiError> {
  let devices = usbip.list_devices().await?;
  let Some(device) = devices.into_iter().find(|device| device.busid == busid) else {
    return Err(ApiError::not_found(format!("device {busid} was not found")));
  };

  Ok(Json(device))
}

struct ApiError {
  status: StatusCode,
  message: String,
}

impl ApiError {
  fn not_found(message: String) -> Self {
    Self {
      status: StatusCode::NOT_FOUND,
      message,
    }
  }
}

impl From<anyhow::Error> for ApiError {
  fn from(error: anyhow::Error) -> Self {
    Self {
      status: StatusCode::INTERNAL_SERVER_ERROR,
      message: error.to_string(),
    }
  }
}

impl IntoResponse for ApiError {
  fn into_response(self) -> Response {
    (self.status, self.message).into_response()
  }
}
