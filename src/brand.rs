//! Branding: the app's name, whose it is, and its icon. Stored in `config.json` in the
//! data folder so one binary can be "CatApp" for Cat and "MaisyApp" for Maisy.
//! Read fresh where it's used, so changes apply without a restart (the web page picks
//! them up on reload).

use serde::{Deserialize, Serialize};
use std::path::Path;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Brand {
    /// The app's name, e.g. "CatApp".
    pub name: String,
    /// The owner's first name, used by the assistant and agents, e.g. "Cat".
    pub owner: String,
    /// "cat", "plain", or "" for automatic (the cat icon only for CatApp).
    pub icon: String,
    /// The launchd label the background service was installed under, if any.
    pub service_label: String,
}

impl Default for Brand {
    fn default() -> Self {
        Brand { name: "CatApp".into(), owner: "Cat".into(), icon: String::new(), service_label: String::new() }
    }
}

pub const KEYS: &[(&str, &str)] = &[
    ("name", "the app's name, e.g. MaisyApp"),
    ("owner", "your first name, e.g. Maisy (the assistant and agents use it)"),
    ("icon", "cat, plain, or auto (cat for CatApp, plain otherwise)"),
];

impl Brand {
    pub fn load(home: &Path) -> Brand {
        std::fs::read_to_string(home.join("config.json"))
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default()
    }

    pub fn save(&self, home: &Path) -> Result<(), String> {
        std::fs::create_dir_all(home).map_err(|e| e.to_string())?;
        std::fs::write(home.join("config.json"), serde_json::to_string_pretty(self).unwrap()).map_err(|e| e.to_string())
    }

    pub fn set(&mut self, key: &str, value: &str) -> Result<(), String> {
        let v = value.trim();
        match key {
            "name" | "owner" if v.is_empty() => return Err(format!("{key} can't be empty")),
            "name" => self.name = v.into(),
            "owner" => self.owner = v.into(),
            "icon" => match v {
                "cat" | "plain" => self.icon = v.into(),
                "auto" | "" => self.icon.clear(),
                _ => return Err("icon must be cat, plain or auto".into()),
            },
            _ => {
                return Err(format!(
                    "unknown setting '{key}': use {}",
                    KEYS.iter().map(|(k, _)| *k).collect::<Vec<_>>().join(", ")
                ))
            }
        }
        Ok(())
    }

    /// "cat" or "plain".
    pub fn icon_kind(&self) -> &str {
        match self.icon.as_str() {
            "cat" | "plain" => &self.icon,
            _ if self.slug() == "catapp" => "cat",
            _ => "plain",
        }
    }

    /// The name squashed to lowercase letters and digits, e.g. "maisyapp".
    pub fn slug(&self) -> String {
        let s: String = self.name.chars().filter(|c| c.is_ascii_alphanumeric()).collect::<String>().to_lowercase();
        if s.is_empty() {
            "catapp".into()
        } else {
            s
        }
    }

    /// The launchd label for a new service install. CatApp keeps its original label.
    pub fn new_service_label(&self) -> String {
        match self.slug().as_str() {
            "catapp" => "bio.cat.catapp".into(),
            s => format!("local.organiser.{s}"),
        }
    }

    /// The label of the installed service: as recorded at install, or the default.
    pub fn installed_service_label(&self) -> String {
        if self.service_label.is_empty() {
            self.new_service_label()
        } else {
            self.service_label.clone()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_and_settings() {
        let mut b = Brand::default();
        assert_eq!((b.slug().as_str(), b.icon_kind()), ("catapp", "cat"));
        assert_eq!(b.new_service_label(), "bio.cat.catapp");
        b.set("name", "Maisy App").unwrap();
        b.set("owner", "Maisy").unwrap();
        assert_eq!((b.slug().as_str(), b.icon_kind()), ("maisyapp", "plain"));
        assert_eq!(b.new_service_label(), "local.organiser.maisyapp");
        b.set("icon", "cat").unwrap();
        assert_eq!(b.icon_kind(), "cat");
        assert!(b.set("icon", "dog").is_err() && b.set("name", " ").is_err() && b.set("colour", "x").is_err());
    }
}
