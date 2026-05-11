use anyhow::{Context as AnyhowContext, Result, bail};
use reqwest::Url;
use tokio::{process::Command, task::JoinHandle, time::Duration};

pub fn ensure_running_as_root() -> Result<()> {
  if effective_uid() == 0 {
    Ok(())
  } else {
    anyhow::bail!("usbip operations require sudo/root; rerun rusbee with sudo")
  }
}

#[cfg(unix)]
fn effective_uid() -> u32 {
  unsafe extern "C" {
    fn geteuid() -> u32;
  }

  unsafe { geteuid() }
}

#[cfg(not(unix))]
fn effective_uid() -> u32 {
  1
}

pub fn server_host(server: &str) -> Result<String> {
  let url = Url::parse(server).context("server must be a valid http URL")?;
  let Some(host) = url.host_str() else {
    bail!("server URL must include a host");
  };

  Ok(host.to_string())
}

pub struct SudoSessionKeeper {
  keepalive_task: JoinHandle<()>,
}

impl Drop for SudoSessionKeeper {
  fn drop(&mut self) {
    self.keepalive_task.abort();
  }
}

pub async fn start_sudo_session_keeper() -> Result<SudoSessionKeeper> {
  let output = Command::new("sudo")
    .arg("-v")
    .output()
    .await
    .context("failed to execute sudo -v")?;

  if !output.status.success() {
    let stderr = String::from_utf8_lossy(&output.stderr);
    bail!("failed to authenticate sudo session: {stderr}");
  }

  let keepalive_task = tokio::spawn(async move {
    loop {
      tokio::time::sleep(Duration::from_secs(60)).await;
      let _ = Command::new("sudo").args(["-n", "-v"]).output().await;
    }
  });

  Ok(SudoSessionKeeper { keepalive_task })
}
