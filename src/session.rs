//! Saving and restoring the set of open windows across restarts.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

pub const VERSION: u32 = 1;

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct Session {
    pub version: u32,
    pub desktop: usize,
    /// Per-desktop (layout name, master ratio).
    pub desktops: Vec<(String, f32)>,
    /// In creation order.
    pub windows: Vec<SavedWindow>,
    #[serde(default)]
    pub desktop_names: Vec<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct SavedWindow {
    /// None for the login shell.
    pub command: Option<String>,
    pub name: String,
    pub cwd: Option<PathBuf>,
    /// x, y, width, height of the floating geometry.
    pub rect: [u16; 4],
    pub maximized: bool,
    pub minimized: bool,
    pub floating: bool,
    pub desktop: usize,
    pub focused: bool,
    #[serde(default)]
    pub above: bool,
    /// Title set by the user with "rename".
    #[serde(default)]
    pub title: Option<String>,
}

pub fn state_dir() -> PathBuf {
    if let Some(dir) = std::env::var_os("XDG_STATE_HOME").filter(|d| !d.is_empty()) {
        return PathBuf::from(dir).join("tuidesk");
    }
    // %LOCALAPPDATA%\tuidesk
    #[cfg(windows)]
    if let Some(dir) = dirs::data_local_dir() {
        return dir.join("tuidesk");
    }
    dirs::home_dir().unwrap_or_else(|| PathBuf::from(".")).join(".local").join("state").join("tuidesk")
}

pub fn path_for(name: &str) -> PathBuf {
    state_dir().join(format!("session-{name}.json"))
}

pub fn save(path: &Path, session: &Session) -> Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
    }
    let json = serde_json::to_string_pretty(session)?;
    // Write then rename, so a crash never leaves a half-written session.
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, json).with_context(|| format!("writing {}", tmp.display()))?;
    std::fs::rename(&tmp, path).with_context(|| format!("writing {}", path.display()))?;
    Ok(())
}

pub fn load(path: &Path) -> Result<Option<Session>> {
    if !path.exists() {
        return Ok(None);
    }
    let text = std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
    let s: Session = serde_json::from_str(&text).with_context(|| format!("parsing {}", path.display()))?;
    if s.version != VERSION {
        anyhow::bail!("{} is from a different tuidesk version ({})", path.display(), s.version);
    }
    Ok(Some(s))
}

/// Move an unreadable session file aside so it is not overwritten. Returns the new path.
pub fn back_up(path: &Path) -> Option<PathBuf> {
    let backup = path.with_extension("json.bak");
    std::fs::rename(path, &backup).ok().map(|_| backup)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips() {
        let dir = std::env::temp_dir().join(format!("tuidesk-session-{}", std::process::id()));
        let path = dir.join("s.json");
        let s = Session {
            version: VERSION,
            desktop: 1,
            desktops: vec![("tiling".into(), 0.5)],
            windows: vec![SavedWindow {
                command: Some("btop".into()),
                name: "btop".into(),
                cwd: Some("/tmp".into()),
                rect: [1, 2, 30, 10],
                desktop: 1,
                focused: true,
                ..Default::default()
            }],
            desktop_names: vec!["Main".into()],
        };
        save(&path, &s).unwrap();
        assert_eq!(load(&path).unwrap(), Some(s));
        assert_eq!(load(&dir.join("missing.json")).unwrap(), None);
        let old = dir.join("old.json");
        std::fs::write(&old, serde_json::to_string(&Session { version: 99, ..Default::default() }).unwrap()).unwrap();
        assert!(load(&old).is_err());
        assert_eq!(back_up(&old), Some(dir.join("old.json.bak")));
        assert!(!old.exists());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
