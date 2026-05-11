use std::{collections::BTreeMap, env, fs, net::IpAddr, path::PathBuf};

use anyhow::{Context as AnyhowContext, Result, bail};
use reqwest::Url;
use serde::Deserialize;

#[derive(Debug, Default, Deserialize)]
pub struct Config {
  #[serde(default)]
  pub targets: BTreeMap<String, TargetConfig>,

  pub server: Option<ServerConfig>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct TargetConfig {
  pub address: String,
  pub token: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ServerConfig {
  pub host: Option<IpAddr>,
  pub port: Option<u16>,
  pub token: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedTarget {
  pub address: String,
  pub token: Option<String>,
}

pub fn load() -> Result<Config> {
  let Some(path) = path() else {
    return Ok(Config::default());
  };

  load_from_path(path)
}

pub fn load_for_root() -> Result<Config> {
  let system_path = PathBuf::from("/etc/rusbee/config.toml");
  if system_path.exists() {
    return load_from_path(system_path);
  }
  Ok(Config::default())
}

pub fn load_from_path(path: PathBuf) -> Result<Config> {
  if !path.exists() {
    return Ok(Config::default());
  }

  let content = fs::read_to_string(&path)
    .with_context(|| format!("failed to read config {}", path.display()))?;
  parse(&content).with_context(|| format!("failed to parse config {}", path.display()))
}

pub fn parse(content: &str) -> Result<Config> {
  toml::from_str(content).context("config must be valid TOML")
}

pub fn path() -> Option<PathBuf> {
  if let Some(config_home) = env::var_os("XDG_CONFIG_HOME").filter(|value| !value.is_empty()) {
    return Some(PathBuf::from(config_home).join("rusbee/config.toml"));
  }

  env::var_os("HOME")
    .filter(|value| !value.is_empty())
    .map(|home| PathBuf::from(home).join(".config/rusbee/config.toml"))
}

pub fn resolve_target(
  config: &Config,
  target_name: Option<&str>,
  server_override: Option<&str>,
  token_override: Option<&str>,
) -> Result<ResolvedTarget> {
  let (mut target, mut address_source) = match target_name {
    Some(target_name) => {
      let Some(config_target) = config.targets.get(target_name) else {
        bail!(
          "target {target_name} was not found in config{}",
          available_targets(config)
        );
      };

      (
        ResolvedTarget {
          address: config_target.address.clone(),
          token: config_target.token.clone(),
        },
        format!("target {target_name} address"),
      )
    }
    None => match config.targets.len() {
      0 => {
        let Some(server_override) = server_override else {
          bail!("--server or --target is required");
        };

        (
          ResolvedTarget {
            address: server_override.to_string(),
            token: None,
          },
          "--server".to_string(),
        )
      }
      1 => {
        let (target_name, config_target) = config.targets.iter().next().expect("target exists");
        (
          ResolvedTarget {
            address: config_target.address.clone(),
            token: config_target.token.clone(),
          },
          format!("target {target_name} address"),
        )
      }
      _ => bail!("target must be selected when multiple targets are configured"),
    },
  };

  if let Some(server_override) = server_override {
    target.address = server_override.to_string();
    address_source = "--server".to_string();
  }

  if let Some(token_override) = token_override {
    target.token = Some(token_override.to_string());
  }

  if target.address.is_empty() {
    bail!("{address_source} cannot be empty");
  }

  validate_address(&address_source, &target.address)?;

  Ok(target)
}

fn validate_address(source: &str, address: &str) -> Result<()> {
  let url = Url::parse(address).with_context(|| {
    format!("{source} must be a valid http URL; include a scheme such as http://")
  })?;

  match url.scheme() {
    "http" | "https" => Ok(()),
    _ => bail!("{source} must use http or https"),
  }
}

fn available_targets(config: &Config) -> String {
  if config.targets.is_empty() {
    String::new()
  } else {
    format!(
      "; available targets: {}",
      config
        .targets
        .keys()
        .cloned()
        .collect::<Vec<_>>()
        .join(", ")
    )
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn parses_named_targets() {
    let config = parse(
      r#"
[targets.local]
address = "http://127.0.0.1:7878"
token = "secret"

[targets.lab]
address = "http://10.0.0.5:7878"
"#,
    )
    .unwrap();

    assert_eq!(config.targets["local"].address, "http://127.0.0.1:7878");
    assert_eq!(config.targets["local"].token.as_deref(), Some("secret"));
    assert_eq!(config.targets["lab"].address, "http://10.0.0.5:7878");
    assert_eq!(config.targets["lab"].token, None);
  }

  #[test]
  fn parses_server_config() {
    let config = parse(
      r#"
[server]
host = "0.0.0.0"
port = 7979
token = "server-secret"
"#,
    )
    .unwrap();

    let server = config.server.expect("server config should be present");
    assert_eq!(server.host, Some("0.0.0.0".parse().unwrap()));
    assert_eq!(server.port, Some(7979));
    assert_eq!(server.token.as_deref(), Some("server-secret"));
  }

  #[test]
  fn missing_config_yields_empty_config() {
    let config = load_from_path(PathBuf::from(
      "/tmp/rusbee-missing-config-yields-empty-config.toml",
    ))
    .unwrap();

    assert!(config.targets.is_empty());
  }

  #[test]
  fn requires_server_or_target_without_config() {
    let config = Config::default();

    let error = resolve_target(&config, None, None, None).unwrap_err();

    assert_eq!(error.to_string(), "--server or --target is required");
  }

  #[test]
  fn resolves_server_without_config() {
    let config = Config::default();

    let target = resolve_target(&config, None, Some("http://workbench.local:7878"), None).unwrap();

    assert_eq!(
      target,
      ResolvedTarget {
        address: "http://workbench.local:7878".to_string(),
        token: None,
      }
    );
  }

  #[test]
  fn resolves_explicit_target() {
    let config = parse(
      r#"
[targets.lab]
address = "http://10.0.0.5:7878"
token = "secret"
"#,
    )
    .unwrap();

    let target = resolve_target(&config, Some("lab"), None, None).unwrap();

    assert_eq!(
      target,
      ResolvedTarget {
        address: "http://10.0.0.5:7878".to_string(),
        token: Some("secret".to_string()),
      }
    );
  }

  #[test]
  fn flags_override_target_fields() {
    let config = parse(
      r#"
[targets.lab]
address = "http://10.0.0.5:7878"
token = "secret"
"#,
    )
    .unwrap();

    let target = resolve_target(
      &config,
      Some("lab"),
      Some("http://localhost:9999"),
      Some("override"),
    )
    .unwrap();

    assert_eq!(
      target,
      ResolvedTarget {
        address: "http://localhost:9999".to_string(),
        token: Some("override".to_string()),
      }
    );
  }

  #[test]
  fn auto_selects_one_target() {
    let config = parse(
      r#"
[targets.local]
address = "http://127.0.0.1:7878"
"#,
    )
    .unwrap();

    let target = resolve_target(&config, None, None, None).unwrap();

    assert_eq!(target.address, "http://127.0.0.1:7878");
  }

  #[test]
  fn rejects_malformed_target_address() {
    let config = parse(
      r#"
[targets.workbench]
address = "workbench.local"
"#,
    )
    .unwrap();

    let error = resolve_target(&config, Some("workbench"), None, None).unwrap_err();

    assert!(
      error
        .to_string()
        .contains("target workbench address must be a valid http URL")
    );
  }

  #[test]
  fn target_not_found_lists_available_targets() {
    let config = parse(
      r#"
[targets.workbench]
address = "http://workbench.local:7878"
"#,
    )
    .unwrap();

    let error = resolve_target(&config, Some("missing"), None, None).unwrap_err();

    assert_eq!(
      error.to_string(),
      "target missing was not found in config; available targets: workbench"
    );
  }

  #[test]
  fn rejects_ambiguous_targets_without_selection() {
    let config = parse(
      r#"
[targets.local]
address = "http://127.0.0.1:7878"

[targets.lab]
address = "http://10.0.0.5:7878"
"#,
    )
    .unwrap();

    assert!(resolve_target(&config, None, None, None).is_err());
  }
}
