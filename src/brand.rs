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
    /// Voice mode for the assistant: talk to it and hear replies. Off unless turned on.
    pub voice: bool,
    /// The Kokoro voice for replies: a voice name ("af_heart"), an even blend
    /// ("af_alloy,af_nicole") or a named blend from voice_server.py ("patchouli").
    pub speaker: String,
    /// A persona for voice mode: a text file describing who the assistant is. "" for none.
    pub persona: String,
    /// A memory file the persona sees (read-only) in voice mode. "" for none.
    pub memory: String,
    /// Show a Live2D avatar that lip-syncs in voice mode (the model goes in the data folder's `avatar/`).
    pub avatar: bool,
}

impl Default for Brand {
    fn default() -> Self {
        Brand {
            name: "CatApp".into(),
            owner: "Cat".into(),
            icon: String::new(),
            service_label: String::new(),
            voice: false,
            speaker: DEFAULT_SPEAKER.into(),
            persona: String::new(),
            memory: String::new(),
            avatar: false,
        }
    }
}

pub const KEYS: &[(&str, &str)] = &[
    ("name", "the app's name, e.g. MaisyApp"),
    ("owner", "your first name, e.g. Maisy (the assistant and agents use it)"),
    ("icon", "cat, plain, or auto (cat for CatApp, plain otherwise)"),
    ("voice", "on or off: talk to the assistant and hear its replies (needs voice/setup.sh)"),
    ("speaker", "the reply voice: a Kokoro voice like af_heart, or a blend like patchouli"),
    ("persona", "a text file describing a character for the assistant to play in voice mode, or none"),
    ("memory", "a notes file the persona can read (not change) in voice mode, or none"),
    ("avatar", "on or off: a Live2D avatar that lip-syncs in voice mode"),
];

pub const DEFAULT_SPEAKER: &str = "af_heart";

fn on_off(key: &str, v: &str) -> Result<bool, String> {
    match v.to_lowercase().as_str() {
        "on" | "yes" | "true" => Ok(true),
        "off" | "no" | "false" => Ok(false),
        _ => Err(format!("{key} must be on or off")),
    }
}

/// An existing file's absolute path, from a path that may start with `~/`; "" for none.
fn file_setting(key: &str, v: &str) -> Result<String, String> {
    if matches!(v, "" | "none" | "off") {
        return Ok(String::new());
    }
    let p = match v.strip_prefix("~/") {
        Some(rest) => dirs::home_dir().unwrap_or_default().join(rest),
        None => std::path::PathBuf::from(v),
    };
    let p = std::fs::canonicalize(&p).map_err(|_| format!("{key}: no file at {}", p.display()))?;
    if !p.is_file() {
        return Err(format!("{key}: {} isn't a file", p.display()));
    }
    Ok(p.to_string_lossy().into_owned())
}

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
            "voice" => self.voice = on_off(key, v)?,
            "avatar" => self.avatar = on_off(key, v)?,
            "speaker" if v.is_empty() || v == "default" => self.speaker = DEFAULT_SPEAKER.into(),
            "speaker" if v.chars().all(|c| c.is_ascii_alphanumeric() || "_-,".contains(c)) => self.speaker = v.into(),
            "speaker" => return Err("speaker must be a Kokoro voice name like af_heart, or a blend name".into()),
            "persona" => self.persona = file_setting(key, v)?,
            "memory" => self.memory = file_setting(key, v)?,
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

    /// The persona's name, from its file name: `personas/patchouli.md` is "Patchouli".
    pub fn persona_name(&self) -> Option<String> {
        let stem = Path::new(&self.persona).file_stem()?.to_string_lossy().replace(['-', '_'], " ");
        let mut c = stem.chars();
        Some(c.next()?.to_uppercase().chain(c).collect())
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

    /// The service name for a new install: a launchd label on macOS (CatApp keeps its
    /// original one), and the slug elsewhere (`maisyapp.service` for systemd).
    pub fn new_service_label(&self) -> String {
        if !cfg!(target_os = "macos") {
            return self.slug();
        }
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
        if cfg!(target_os = "macos") {
            assert_eq!(b.new_service_label(), "bio.cat.catapp");
        }
        b.set("name", "Maisy App").unwrap();
        b.set("owner", "Maisy").unwrap();
        assert_eq!((b.slug().as_str(), b.icon_kind()), ("maisyapp", "plain"));
        let label = if cfg!(target_os = "macos") { "local.organiser.maisyapp" } else { "maisyapp" };
        assert_eq!(b.new_service_label(), label);
        b.set("icon", "cat").unwrap();
        assert_eq!(b.icon_kind(), "cat");
        assert!(b.set("icon", "dog").is_err() && b.set("name", " ").is_err() && b.set("colour", "x").is_err());
    }

    #[test]
    fn voice_settings() {
        let mut b = Brand::default();
        assert!(!b.voice && !b.avatar && b.speaker == DEFAULT_SPEAKER && b.persona_name().is_none());
        b.set("voice", "on").unwrap();
        b.set("speaker", "patchouli").unwrap();
        assert!(b.voice && b.speaker == "patchouli");
        assert!(b.set("voice", "maybe").is_err() && b.set("speaker", "a b").is_err());
        assert!(b.set("persona", "/no/such/file.md").is_err());
        let f = std::env::temp_dir().join("catapp-test-patchouli-knowledge.md");
        std::fs::write(&f, "persona").unwrap();
        b.set("persona", f.to_str().unwrap()).unwrap();
        assert_eq!(b.persona_name().as_deref(), Some("Catapp test patchouli knowledge"));
        b.set("persona", "none").unwrap();
        assert!(b.persona.is_empty());
    }
}
