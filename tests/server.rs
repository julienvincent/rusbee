use std::sync::{Arc, Mutex};

use anyhow::Result;
use axum::{
  body::{Body, to_bytes},
  http::{Request, StatusCode, header},
};
use rusbee::{
  commands::server::{ServerAuth, router},
  device::{Attachment, UsbDevice, UsbDeviceMetadata},
  usbip::{BoxFuture, Usbip},
};
use tower::ServiceExt;

#[derive(Default)]
struct MockUsbip {
  devices: Mutex<Vec<UsbDevice>>,
  bound_busids: Mutex<Vec<String>>,
  unbound_busids: Mutex<Vec<String>>,
}

impl MockUsbip {
  fn with_devices(devices: Vec<UsbDevice>) -> Self {
    Self {
      devices: Mutex::new(devices),
      bound_busids: Mutex::new(Vec::new()),
      unbound_busids: Mutex::new(Vec::new()),
    }
  }

  fn bound_busids(&self) -> Vec<String> {
    self.bound_busids.lock().unwrap().clone()
  }

  fn unbound_busids(&self) -> Vec<String> {
    self.unbound_busids.lock().unwrap().clone()
  }
}

impl Usbip for MockUsbip {
  fn ensure_daemon(&self) -> BoxFuture<'_, Result<()>> {
    Box::pin(async move { Ok(()) })
  }

  fn list_devices(&self) -> BoxFuture<'_, Result<Vec<UsbDevice>>> {
    Box::pin(async move { Ok(self.devices.lock().unwrap().clone()) })
  }

  fn bind<'a>(&'a self, busid: &'a str) -> BoxFuture<'a, Result<()>> {
    Box::pin(async move {
      self.bound_busids.lock().unwrap().push(busid.to_string());
      Ok(())
    })
  }

  fn unbind<'a>(&'a self, busid: &'a str) -> BoxFuture<'a, Result<()>> {
    Box::pin(async move {
      self.unbound_busids.lock().unwrap().push(busid.to_string());
      Ok(())
    })
  }

  fn attach<'a>(&'a self, _remote_host: &'a str, _busid: &'a str) -> BoxFuture<'a, Result<()>> {
    Box::pin(async move { Ok(()) })
  }

  fn detach(&self, _port: u16) -> BoxFuture<'_, Result<()>> {
    Box::pin(async move { Ok(()) })
  }

  fn list_attachments(&self) -> BoxFuture<'_, Result<Vec<Attachment>>> {
    Box::pin(async move { Ok(Vec::new()) })
  }
}

#[tokio::test]
async fn livez_returns_ok() {
  let response = test_router(Arc::new(MockUsbip::default()))
    .oneshot(Request::get("/livez").body(Body::empty()).unwrap())
    .await
    .unwrap();

  assert_eq!(response.status(), StatusCode::OK);
  let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
  assert_eq!(&body[..], b"ok");
}

#[tokio::test]
async fn health_and_device_attach_are_not_routed() {
  let app = test_router(Arc::new(MockUsbip::default()));

  let health_response = app
    .clone()
    .oneshot(Request::get("/health").body(Body::empty()).unwrap())
    .await
    .unwrap();
  assert_eq!(health_response.status(), StatusCode::NOT_FOUND);

  let attach_response = app
    .oneshot(device_request("/device/attach", "1-2"))
    .await
    .unwrap();
  assert_eq!(attach_response.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn bind_rejects_unknown_busid_before_calling_usbip() {
  let usbip = Arc::new(MockUsbip::with_devices(vec![device("1-2")]));

  let response = test_router(usbip.clone())
    .oneshot(device_request("/device/bind", "1-2;touch /tmp/pwned"))
    .await
    .unwrap();

  assert_eq!(response.status(), StatusCode::NOT_FOUND);
  assert!(usbip.bound_busids().is_empty());
}

#[tokio::test]
async fn unbind_rejects_unknown_busid_before_calling_usbip() {
  let usbip = Arc::new(MockUsbip::with_devices(vec![device("1-2")]));

  let response = test_router(usbip.clone())
    .oneshot(device_request("/device/unbind", "1-2;touch /tmp/pwned"))
    .await
    .unwrap();

  assert_eq!(response.status(), StatusCode::NOT_FOUND);
  assert!(usbip.unbound_busids().is_empty());
}

#[tokio::test]
async fn bind_and_unbind_use_known_device_busid() {
  let usbip = Arc::new(MockUsbip::with_devices(vec![device("1-2")]));

  let bind_response = test_router(usbip.clone())
    .oneshot(device_request("/device/bind", "1-2"))
    .await
    .unwrap();
  assert_eq!(bind_response.status(), StatusCode::OK);
  assert_eq!(usbip.bound_busids(), vec!["1-2"]);

  let unbind_response = test_router(usbip.clone())
    .oneshot(device_request("/device/unbind", "1-2"))
    .await
    .unwrap();
  assert_eq!(unbind_response.status(), StatusCode::OK);
  assert_eq!(usbip.unbound_busids(), vec!["1-2"]);
}

#[tokio::test]
async fn token_auth_rejects_missing_or_wrong_header() {
  let usbip = Arc::new(MockUsbip::with_devices(vec![device("1-2")]));
  let app = router(usbip.clone(), ServerAuth::Bearer("secret".to_string()));

  let missing_response = app
    .clone()
    .oneshot(device_request("/device/bind", "1-2"))
    .await
    .unwrap();
  assert_eq!(missing_response.status(), StatusCode::UNAUTHORIZED);

  let wrong_response = app
    .oneshot(authorized_device_request(
      "/device/bind",
      "1-2",
      "Bearer wrong",
    ))
    .await
    .unwrap();
  assert_eq!(wrong_response.status(), StatusCode::UNAUTHORIZED);
  assert!(usbip.bound_busids().is_empty());
}

#[tokio::test]
async fn token_auth_accepts_matching_bearer_header() {
  let usbip = Arc::new(MockUsbip::with_devices(vec![device("1-2")]));

  let response = router(usbip.clone(), ServerAuth::Bearer("secret".to_string()))
    .oneshot(authorized_device_request(
      "/device/bind",
      "1-2",
      "Bearer secret",
    ))
    .await
    .unwrap();

  assert_eq!(response.status(), StatusCode::OK);
  assert_eq!(usbip.bound_busids(), vec!["1-2"]);
}

fn test_router(usbip: Arc<dyn Usbip>) -> axum::Router {
  router(usbip, ServerAuth::Disabled)
}

fn device_request(path: &str, busid: &str) -> Request<Body> {
  Request::post(path)
    .header(header::CONTENT_TYPE, "application/json")
    .body(Body::from(format!(r#"{{"busid":"{busid}"}}"#)))
    .unwrap()
}

fn authorized_device_request(path: &str, busid: &str, authorization: &str) -> Request<Body> {
  Request::post(path)
    .header(header::CONTENT_TYPE, "application/json")
    .header(header::AUTHORIZATION, authorization)
    .body(Body::from(format!(r#"{{"busid":"{busid}"}}"#)))
    .unwrap()
}

fn device(busid: &str) -> UsbDevice {
  UsbDevice {
    busid: busid.to_string(),
    vendor_id: "1234".to_string(),
    product_id: "5678".to_string(),
    name: "Test USB device".to_string(),
    bound: false,
    attached: false,
    attached_port: None,
    metadata: UsbDeviceMetadata::default(),
  }
}
