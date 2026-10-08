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
    /// Click chats and panes, scroll with the wheel. Hold Shift to drag-select text.
    pub mouse: bool,
    /// Show Contacts names instead of handles.
    pub names: bool,
    /// Overrides for people the Mac's Contacts doesn't link; their chats are shown as one.
    #[serde(rename = "person")]
    pub people: Vec<Person>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Person {
    /// Shown instead of the handles when set.
    #[serde(default)]
    pub name: Option<String>,
    pub handles: Vec<String>,
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
            mouse: true,
            names: true,
            people: Vec::new(),
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

# Click a chat to open it, click a pane to focus it, scroll with the wheel.
# While this is on, hold Shift to select text with the terminal's own mouse.
mouse = true

# The bridge uses the Mac's Contacts to show one conversation per person, even
# when chat.db has separate chats for their phone number and email. With names
# on, the chat list shows the Contacts name; set to false for handles only.
names = true

# Override for someone Contacts doesn't link:
# [[person]]
# name = "Alex"            # optional: shown instead of the handles
# handles = ["+15551234567", "alex@example.com"]
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
        let with_person: super::Config = toml::from_str(&format!(
            "{}\n[[person]]\nname = \"Alex\"\nhandles = [\"+15551234567\", \"alex@example.com\"]\n",
            super::SAMPLE
        ))
        .unwrap();
        assert_eq!(with_person.people.len(), 1);
        assert_eq!(with_person.people[0].name.as_deref(), Some("Alex"));
    }
}
