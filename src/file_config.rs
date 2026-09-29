// SPDX-License-Identifier: MIT

//! Offline config access: when the daemon is unreachable the TUI reads and
//! writes `~/.config/idle/config.yaml` directly. Writes are read-modify-write
//! so daemon-owned keys and user comments are never clobbered.

use std::path::PathBuf;

#[derive(Debug, Clone, Default)]
pub struct FileSettings {
    pub idle_timeout_mins: u32,
    pub idle_enabled: bool,
    pub show_fps_overlay: bool,
    pub active_saver: Option<String>,
    /// `None` = auto (`null` on disk); callers display 1.0.
    pub render_scale: Option<f32>,
    /// `[saver]`/`[saver.*]` params (`[saver.hearth] size` → `hearth.size`).
    /// The daemon delivers these to savers as `IDLE_SAVER_PARAM_*` env vars.
    pub saver_params: std::collections::BTreeMap<String, String>,
}

/// Config path: `IDLE_TUI_CONFIG_DIR` override first (tests/debug), then
/// `idle/` then legacy `trance/` under each base (matches daemon).
pub fn config_path() -> Option<PathBuf> {
    if let Ok(dir) = std::env::var("IDLE_TUI_CONFIG_DIR")
        && !dir.is_empty()
    {
        return Some(PathBuf::from(dir).join("config.yaml"));
    }
    let mut bases = Vec::new();
    if let Ok(xdg) = std::env::var("XDG_CONFIG_HOME")
        && !xdg.is_empty()
    {
        bases.push(PathBuf::from(xdg));
    }
    if let Ok(home) = std::env::var("HOME") {
        bases.push(PathBuf::from(home).join(".config"));
    }
    for base in &bases {
        let p = base.join("idle").join("config.yaml");
        if p.is_file() {
            return Some(p);
        }
    }
    for base in &bases {
        let p = base.join("trance").join("config.yaml");
        if p.is_file() {
            return Some(p);
        }
    }
    bases.first().map(|b| b.join("idle").join("config.yaml"))
}

pub fn load() -> FileSettings {
    let mut s = FileSettings {
        idle_timeout_mins: 5,
        idle_enabled: true,
        ..Default::default()
    };
    let Some(content) = config_path().and_then(|p| std::fs::read_to_string(p).ok()) else {
        return s;
    };
    let mut section = String::new();
    for line in content.lines() {
        let t = line.trim();
        if let Some(sec) = t.strip_prefix('[').and_then(|s| s.strip_suffix(']')) {
            section = sec.to_string();
            continue;
        }
        if t.is_empty() || t.starts_with('#') {
            continue;
        }
        // Accept `=` too — hand-edited files commonly use it and the daemon
        // applies it, so ignoring it here would show stale defaults.
        let Some(idx) = t.find([':', '=']) else {
            continue;
        };
        let val = t[idx + 1..].trim().trim_matches('"').trim_matches('\'');
        let key = t[..idx].trim();
        if section == "saver" {
            s.saver_params.insert(key.to_string(), val.to_string());
            continue;
        }
        if let Some(name) = section.strip_prefix("saver.") {
            s.saver_params
                .insert(format!("{name}.{key}"), val.to_string());
            continue;
        }
        if !section.is_empty() {
            continue;
        }
        match key {
            "idle_timeout_mins" => {
                if let Ok(n) = val.parse() {
                    s.idle_timeout_mins = n;
                }
            }
            "idle_enabled" => s.idle_enabled = val.parse().unwrap_or(s.idle_enabled),
            "show_fps_overlay" => s.show_fps_overlay = val.parse().unwrap_or(s.show_fps_overlay),
            "active_saver" => {
                s.active_saver = (!val.is_empty() && val != "none").then(|| val.to_string())
            }
            "render_scale" => {
                s.render_scale = if val.eq_ignore_ascii_case("null") {
                    None
                } else {
                    val.parse().ok()
                }
            }
            _ => {}
        }
    }
    s
}

/// Rewrite a single top-level key in place, preserving every other line.
/// Pure merge — the IO wrapper is [`write_field`].
pub fn merge_field(existing: &str, key: &str, value: &str) -> String {
    let mut body = String::new();
    let mut written = false;
    let mut in_section = false;
    for line in existing.lines() {
        let t = line.trim();
        if t.starts_with('[') {
            in_section = true;
        }
        if !in_section
            && !written
            && !t.starts_with('#')
            && t.find([':', '=']).is_some_and(|i| t[..i].trim() == key)
        {
            body.push_str(&format!("{key}: {value}\n"));
            written = true;
            continue;
        }
        body.push_str(line);
        body.push('\n');
    }
    if !written {
        if existing.trim().is_empty() {
            body.push_str("# IdleScreen themes and settings\n");
        }
        body.push_str(&format!("{key}: {value}\n"));
    }
    body
}

/// Rewrite a saver param, preserving everything else. A key like
/// `hearth.size` may live flat under `[saver]` or as `size` under
/// `[saver.hearth]` — whichever form exists is rewritten in place (writing
/// flat while a namespaced section exists would lose on parse, since the
/// later section wins). New keys append under `[saver]` (created if
/// absent), which parses identically.
pub fn merge_saver_param(existing: &str, key: &str, value: &str) -> String {
    let mut body = String::new();
    let mut written = false;
    let mut saw_saver = false;
    let mut section = "";
    for line in existing.lines() {
        let t = line.trim();
        if let Some(sec) = t.strip_prefix('[').and_then(|s| s.strip_suffix(']')) {
            section = sec;
            saw_saver |= sec == "saver";
            body.push_str(line);
            body.push('\n');
            continue;
        }
        let resolved = t.find([':', '=']).and_then(|i| {
            if t.starts_with('#') {
                return None;
            }
            let k = t[..i].trim();
            match section.strip_prefix("saver") {
                Some("") => Some((k, k.to_string())),
                Some(rest) => rest
                    .strip_prefix('.')
                    .map(|name| (k, format!("{name}.{k}"))),
                None => None,
            }
        });
        if !written && resolved.as_ref().is_some_and(|(_, r)| r.as_str() == key) {
            // Emit the section-local key so `[saver.hearth] size` stays
            // `size` — writing `hearth.size` here would parse as
            // `hearth.hearth.size`.
            let inner = resolved.unwrap().0;
            body.push_str(&format!("{inner}: {value}\n"));
            written = true;
            continue;
        }
        body.push_str(line);
        body.push('\n');
    }
    if !written {
        if !saw_saver {
            body.push_str("\n[saver]\n");
        }
        body.push_str(&format!("{key}: {value}\n"));
    }
    body
}

/// Lock + read-merge-write for `merge_saver_param` (same pattern as
/// [`write_field`]).
pub fn write_saver_param(key: &str, value: &str) -> std::io::Result<()> {
    write_merged(|existing| merge_saver_param(existing, key, value))
}

pub fn write_field(key: &str, value: &str) -> std::io::Result<()> {
    write_merged(|existing| merge_field(existing, key, value))
}

/// Shared lock + read-modify-write + atomic-rename plumbing for the merge
/// functions above. Serialized against the daemon/applet via the sidecar
/// `config.yaml.lock`.
fn write_merged(merge: impl FnOnce(&str) -> String) -> std::io::Result<()> {
    let Some(path) = config_path() else {
        return Err(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            "no config path",
        ));
    };
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let lock = std::fs::OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(false)
        .open(path.with_file_name("config.yaml.lock"))?;
    lock.lock()?;
    let existing = std::fs::read_to_string(&path).unwrap_or_default();
    let tmp = path.with_file_name(format!("config.yaml.{}.tmp", std::process::id()));
    std::fs::write(&tmp, merge(&existing))?;
    std::fs::rename(&tmp, &path).inspect_err(|_| {
        let _ = std::fs::remove_file(&tmp);
    })
}

#[cfg(test)]
#[path = "file_config_tests.rs"]
mod tests;
