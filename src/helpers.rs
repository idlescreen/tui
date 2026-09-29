// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 IdleScreen

//! TUI app methods extracted from `app.rs` to keep it under the 256-line cap.
//! These are `impl App` blocks; the `App` struct lives in `app.rs`.

use std::process::Command;
use std::time::Duration;

use idle_dbus::TranceClient;
use idle_dbus::daemon_available;

use super::App;

impl App {
    pub fn install_cosmic_applet(&mut self) {
        self.status_message = Some("Installing idle-cosmic package...".to_string());
        match crate::cosmic::install_cosmic_applet() {
            Ok(_) => {
                self.cosmic_applet_installed = crate::cosmic::is_cosmic_applet_installed();
                self.status_message = Some("idle-cosmic installed successfully!".to_string());
            }
            Err(e) => {
                self.status_message = Some(e);
            }
        }
    }

    /// Update local cached state from the daemon (or local fallback when the
    /// daemon is not running).
    pub fn refresh_state(&mut self) {
        if let Some(action_time) = self.last_action {
            if action_time.elapsed() < Duration::from_millis(600) {
                let sys = idle_runner::toolkit::sys_info::get_system_info();
                self.cpu_usage_pct = sys.cpu_usage_pct;
                self.mem_used_pct = sys.mem_used_pct;
                self.mem_used_mb = sys.mem_used_mb;
                self.mem_total_mb = sys.mem_total_mb;
                if self.selected_saver_idx > self.screensavers.len() {
                    self.selected_saver_idx = self.screensavers.len();
                }
                return;
            } else {
                self.last_action = None;
            }
        }

        self.daemon_running = daemon_available();
        let live = self.daemon_running && self.refresh_from_daemon();
        // Saver params are file-only — D-Bus status doesn't carry them, so
        // they're loaded every refresh regardless of daemon state.
        let f = crate::file_config::load();
        self.saver_params = f.saver_params;
        if !live {
            // Daemon down OR connect/status failed: show on-disk settings,
            // not hardcoded defaults — otherwise it looks like an update
            // wiped the user's config (and an edit would persist them).
            self.client = None;
            self.screensavers = idle_runner::discovery::detect_screensavers();
            self.idle_enabled = f.idle_enabled;
            self.idle_timeout_mins = f.idle_timeout_mins;
            self.show_fps_overlay = f.show_fps_overlay;
            self.render_scale = f.render_scale.unwrap_or(1.0);
            self.active_saver = f.active_saver.unwrap_or_else(|| "Random".to_string());
        }

        self.refresh_sys_info();
        if self.selected_saver_idx > self.screensavers.len() {
            self.selected_saver_idx = self.screensavers.len();
        }
    }

    /// Populate from the daemon. Returns false when the daemon is not
    /// actually usable (connect or status failed) so the caller can fall
    /// back to on-disk values instead of displaying built-in defaults.
    fn refresh_from_daemon(&mut self) -> bool {
        let Ok(client) = TranceClient::connect() else {
            return false;
        };
        let Ok(status) = client.get_status() else {
            return false;
        };
        self.idle_enabled = status.idle_enabled;
        self.idle_timeout_mins = status.idle_timeout_mins;
        // Normalize empty / random / shuffle to "Random" for UI starring.
        self.active_saver = if crate::ui::is_random_saver(&status.active_saver) {
            "Random".to_string()
        } else {
            status.active_saver
        };
        self.show_fps_overlay = status.show_fps_overlay;
        self.render_scale = status.render_scale.parse::<f32>().unwrap_or(1.0);
        self.on_battery = status.inhibited;
        if let Ok(savers) = client.list_savers() {
            self.screensavers = savers;
        }
        self.inhibitors = client.list_inhibitors().unwrap_or_default();
        self.client = Some(client);
        true
    }

    fn refresh_sys_info(&mut self) {
        let sys = idle_runner::toolkit::sys_info::get_system_info();
        self.on_battery = sys.power_status.contains("Battery");
        self.cpu_usage_pct = sys.cpu_usage_pct;
        self.mem_used_pct = sys.mem_used_pct;
        self.mem_used_mb = sys.mem_used_mb;
        self.mem_total_mb = sys.mem_total_mb;
    }

    pub fn toggle_daemon(&mut self) {
        if self.daemon_running {
            let _ = idle_dbus::service::stop_daemon_service();
        } else {
            let _ = idle_dbus::service::start_daemon_service();
        }
        std::thread::sleep(Duration::from_millis(350));
        self.refresh_state();
    }

    pub fn preview_saver(&mut self) {
        let saver = if self.selected_saver_idx == 0 {
            if self.screensavers.is_empty() {
                "beams".to_string()
            } else {
                self.screensavers[0].clone()
            }
        } else {
            self.screensavers[self.selected_saver_idx - 1].clone()
        };

        if !self.daemon_running {
            self.toggle_daemon();
        }

        let mut started_via_dbus = false;
        if self.daemon_running {
            if self.client.is_none() {
                self.refresh_state();
            }
            if let Some(ref client) = self.client
                && client.preview(&saver).is_ok()
            {
                started_via_dbus = true;
            }
        }
        if !started_via_dbus {
            let _ = Command::new("idle-daemon")
                .args(["run-plugin", &saver])
                .status();
        }
    }
}
