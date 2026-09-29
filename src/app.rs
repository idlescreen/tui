// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 IdleScreen

use std::time::Instant;

#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub enum ActivePane {
    Dashboard,
    Settings,
    Screensavers,
}

pub struct App {
    pub client: Option<idle_dbus::TranceClient>,
    pub daemon_running: bool,
    pub idle_enabled: bool,
    pub idle_timeout_mins: u32,
    pub render_scale: f32,
    pub show_fps_overlay: bool,
    pub active_saver: String,
    pub on_battery: bool,
    pub screensavers: Vec<String>,
    pub selected_saver_idx: usize,
    pub active_pane: ActivePane,
    pub selected_setting_idx: usize,
    pub cpu_usage_pct: f32,
    pub mem_used_pct: f32,
    pub mem_used_mb: u64,
    pub mem_total_mb: u64,
    pub inhibitors: Vec<(u32, String, String)>,
    /// `[saver]`/`[saver.*]` params from config.yaml — the daemon passes
    /// these to savers as `IDLE_SAVER_PARAM_*` env vars. Always file-sourced
    /// (D-Bus status doesn't carry them).
    pub saver_params: std::collections::BTreeMap<String, String>,
    pub tick_count: u64,
    pub cosmic_de_detected: bool,
    pub cosmic_applet_installed: bool,
    pub status_message: Option<String>,
    pub last_action: Option<Instant>,
}

impl App {
    pub fn new() -> Self {
        let mut app = Self {
            client: None,
            daemon_running: false,
            idle_enabled: true,
            idle_timeout_mins: 5,
            render_scale: 1.0,
            show_fps_overlay: false,
            active_saver: "Random".to_string(),
            on_battery: false,
            screensavers: Vec::new(),
            selected_saver_idx: 0,
            active_pane: ActivePane::Dashboard,
            selected_setting_idx: 0,
            cpu_usage_pct: 0.0,
            mem_used_pct: 0.0,
            mem_used_mb: 0,
            mem_total_mb: 0,
            inhibitors: Vec::new(),
            saver_params: std::collections::BTreeMap::new(),
            tick_count: 0,
            cosmic_de_detected: crate::cosmic::is_cosmic_de(),
            cosmic_applet_installed: crate::cosmic::is_cosmic_applet_installed(),
            status_message: None,
            last_action: None,
        };
        app.refresh_state();
        app
    }

    /// Daemon offline: persist a key straight to config.yaml so the change
    /// is real, not a pretend edit that evaporates on the next read.
    fn persist_offline(&mut self, key: &str, value: &str) {
        self.status_message = Some(match crate::file_config::write_field(key, value) {
            Ok(()) => format!("Saved to config file (daemon offline): {key} = {value}"),
            Err(e) => format!("Daemon offline and config write failed: {e}"),
        });
    }

    pub fn toggle_idle(&mut self) {
        self.idle_enabled = !self.idle_enabled;
        if let Some(ref client) = self.client {
            let _ = if self.idle_enabled {
                client.enable()
            } else {
                client.disable()
            };
        } else {
            self.persist_offline("idle_enabled", &self.idle_enabled.to_string());
        }
        self.last_action = Some(Instant::now());
    }

    pub fn adjust_timeout(&mut self, delta: i32) {
        let mut val = self.idle_timeout_mins as i32 + delta;
        val = val.clamp(1, 240);
        self.idle_timeout_mins = val as u32;
        if let Some(ref client) = self.client {
            let _ = client.set_timeout(self.idle_timeout_mins);
        } else {
            self.persist_offline("idle_timeout_mins", &self.idle_timeout_mins.to_string());
        }
        self.last_action = Some(Instant::now());
    }

    pub fn adjust_scale(&mut self, delta: f32) {
        let mut val = self.render_scale + delta;
        val = val.clamp(0.25, 1.0);
        self.render_scale = val;
        if let Some(ref client) = self.client {
            let _ = client.set_render_scale(self.render_scale);
        } else {
            self.persist_offline("render_scale", &self.render_scale.to_string());
        }
        self.last_action = Some(Instant::now());
    }

    pub fn toggle_fps(&mut self) {
        self.show_fps_overlay = !self.show_fps_overlay;
        if let Some(ref client) = self.client {
            let _ = client.set_show_fps_overlay(self.show_fps_overlay);
        } else {
            self.persist_offline("show_fps_overlay", &self.show_fps_overlay.to_string());
        }
        self.last_action = Some(Instant::now());
    }

    /// Settings pane rows: 5 fixed + one per saver param.
    pub fn settings_row_count(&self) -> usize {
        5 + self.saver_params.len()
    }

    /// Adjust the saver param at settings row `idx` (0-based within params).
    /// Numeric params step by ±1 (ints) or ±0.05 (floats); non-numeric
    /// params aren't adjustable from here — edit config.yaml directly.
    /// Writes the config file directly: params are consumed by savers on the
    /// next presentation, and the daemon hot-reloads the file.
    pub fn adjust_param(&mut self, idx: usize, delta: f64) {
        let Some((key, cur)) = self
            .saver_params
            .iter()
            .nth(idx)
            .map(|(k, v)| (k.clone(), v.clone()))
        else {
            return;
        };
        let new_val = if let Ok(i) = cur.parse::<i64>() {
            (i + delta as i64).to_string()
        } else if let Ok(f) = cur.parse::<f64>() {
            format!("{:.2}", f + delta)
        } else {
            self.status_message = Some(format!(
                "{key} is not numeric — edit config.yaml to change it"
            ));
            return;
        };
        match crate::file_config::write_saver_param(&key, &new_val) {
            Ok(()) => {
                self.saver_params.insert(key.clone(), new_val.clone());
                self.status_message = Some(format!("{key}: {new_val} (next presentation)"));
            }
            Err(e) => {
                self.status_message = Some(format!("Failed to write {key}: {e}"));
            }
        }
        self.last_action = Some(Instant::now());
    }

    pub fn select_saver(&mut self) {
        // Ensure we have a live D-Bus client (start daemon if needed).
        if self.client.is_none() {
            if !self.daemon_running {
                self.toggle_daemon();
            } else {
                self.refresh_state();
            }
        }
        let name = if self.selected_saver_idx == 0 {
            // Empty string is the daemon wire value for random (see SetSaver).
            String::new()
        } else if self.selected_saver_idx - 1 < self.screensavers.len() {
            self.screensavers[self.selected_saver_idx - 1].clone()
        } else {
            return;
        };

        if let Some(ref client) = self.client {
            match client.set_saver(&name) {
                Ok(()) => {
                    self.active_saver = if name.is_empty() {
                        "Random".to_string()
                    } else {
                        name
                    };
                    self.status_message = Some(if self.active_saver == "Random" {
                        "Saver: Random (pick at idle)".into()
                    } else {
                        format!("Saver: {}", self.active_saver)
                    });
                }
                Err(e) => {
                    self.status_message = Some(format!("Failed to set saver: {e}"));
                }
            }
        } else {
            let file_val = if name.is_empty() {
                "none"
            } else {
                name.as_str()
            };
            self.persist_offline("active_saver", &format!("\"{file_val}\""));
        }
        self.last_action = Some(Instant::now());
    }
}

#[cfg(test)]
#[path = "app_tests.rs"]
mod tests;
