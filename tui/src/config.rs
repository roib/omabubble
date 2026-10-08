use anyhow::{Context, Result};
use serde::Deserialize;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    /// SSH host (alias from ~/.ssh/config) whose key's forced command runs `imsg-bridge connect`.
    pub host: String,
    /// Extra ssh arguments, e.g. ["-p", "2233"].
    pub ssh_args: Vec<String>,
    /// Remote command, for keys without a forced command, e.g. "/Users/you/.local/bin/imsg-bridge connect".
    pub remote_command: Option<String>,
    /// Run this instead of ssh (local testing).
    pub command: Option<Vec<String>>,
    /// Prepended to 10-digit numbers.
    pub country_code: String,
    pub notifications: bool,
    pub bell: bool,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            host: String::new(),
            ssh_args: Vec::new(),
            remote_command: None,
            command: None,
            country_code: "1".into(),
            notifications: true,
            bell: false,
        }
    }
}

pub const SAMPLE: &str = r#"# imsg configuration

# SSH host alias (see ~/.ssh/config) for the Mac running imsg-bridge.
host = "mac-imsg"

# Extra ssh arguments, if the host alias doesn't cover them.
# ssh_args = ["-p", "2233"]

# Only needed when the SSH key has no forced command:
# remote_command = "/Users/you/.local/bin/imsg-bridge connect"

# Prepended to 10-digit numbers typed in the new-conversation popup.
country_code = "1"

notifications = true
bell = false
"#;

pub fn default_path() -> Result<PathBuf> {
    Ok(dirs::config_dir()
        .context("no config directory")?
        .join("imsg/config.toml"))
}

/// `Ok(None)` when the file doesn't exist.
pub fn load(path: &Path) -> Result<Option<Config>> {
    match std::fs::read_to_string(path) {
        Ok(s) => Ok(Some(
            toml::from_str(&s).with_context(|| format!("parsing {}", path.display()))?,
        )),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e).with_context(|| format!("reading {}", path.display())),
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn sample_parses() {
        let c: super::Config = toml::from_str(super::SAMPLE).unwrap();
        assert_eq!(c.host, "mac-imsg");
        assert_eq!(c.country_code, "1");
        assert!(c.notifications && !c.bell && c.remote_command.is_none());
    }
}
