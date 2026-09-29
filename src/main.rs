// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 IdleScreen

mod app;
pub mod cosmic;
mod file_config;
mod actions;
mod ui;

use std::io::{self, Write};
use std::time::{Duration, Instant};

use app::{ActivePane, App};
use crossterm::{
    event::{self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyModifiers},
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use ratatui::{Terminal, backend::CrosstermBackend};
use ui::render_ui;

/// Restores the terminal (raw mode + alternate screen + mouse capture) on every
/// exit path — including `?` early returns and panics.
///
/// This is the same shape as `studio`'s `TerminalGuard`. Before it existed,
/// `main` called `enable_raw_mode()` and only restored state at the very end,
/// so a failure in `execute!` or `Terminal::new` left the user's shell in raw
/// mode, and any panic in `run_app` left raw mode + alt screen + mouse capture
/// on — the user then had to type `reset` blind.
struct TerminalGuard;

impl TerminalGuard {
    fn enter() -> io::Result<Self> {
        enable_raw_mode()?;
        let mut out = io::stdout();
        if let Err(e) = execute!(out, EnterAlternateScreen, EnableMouseCapture) {
            // Do not leave raw mode on if we could not finish entering.
            let _ = disable_raw_mode();
            return Err(e);
        }
        let prior = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            restore(&mut io::stdout());
            prior(info);
        }));
        Ok(Self)
    }
}

fn restore(out: &mut io::Stdout) {
    let _ = execute!(out, LeaveAlternateScreen, DisableMouseCapture);
    let _ = disable_raw_mode();
    let _ = out.flush();
}

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        restore(&mut io::stdout());
    }
}

fn main() -> std::io::Result<()> {
    // Held for the whole run: every later `?` return, and any unwind, still
    // restores the terminal.
    let _guard = TerminalGuard::enter()?;
    let mut terminal = Terminal::new(CrosstermBackend::new(io::stdout()))?;

    let mut app = App::new();
    let res = run_app(&mut terminal, &mut app);

    // The guard restores on drop; show the cursor explicitly since that is
    // not part of the guard's cleanup contract.
    terminal.show_cursor()?;

    if let Err(err) = res {
        println!("Error running TUI: {err}");
    }
    Ok(())
}

fn run_app(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    app: &mut App,
) -> std::io::Result<()> {
    let mut last_tick = Instant::now();
    let tick_rate = Duration::from_millis(250);

    loop {
        terminal.draw(|f| render_ui(f, app))?;

        let timeout = tick_rate
            .checked_sub(last_tick.elapsed())
            .unwrap_or(Duration::ZERO);

        if event::poll(timeout)?
            && let Event::Key(key) = event::read()?
        {
            if key.modifiers.contains(KeyModifiers::CONTROL)
                && (key.code == KeyCode::Char('c') || key.code == KeyCode::Char('d'))
            {
                return Ok(());
            }
            match key.code {
                KeyCode::Char('q') | KeyCode::Esc => return Ok(()),
                KeyCode::Tab => {
                    app.active_pane = match app.active_pane {
                        ActivePane::Dashboard => ActivePane::Settings,
                        ActivePane::Settings => ActivePane::Screensavers,
                        ActivePane::Screensavers => ActivePane::Dashboard,
                    };
                }
                KeyCode::Up => match app.active_pane {
                    ActivePane::Dashboard => {}
                    ActivePane::Settings => {
                        if app.selected_setting_idx > 0 {
                            app.selected_setting_idx -= 1;
                        }
                    }
                    ActivePane::Screensavers => {
                        if app.selected_saver_idx > 0 {
                            app.selected_saver_idx -= 1;
                        }
                    }
                },
                KeyCode::Down => match app.active_pane {
                    ActivePane::Dashboard => {}
                    ActivePane::Settings => {
                        if app.selected_setting_idx + 1 < app.settings_row_count() {
                            app.selected_setting_idx += 1;
                        }
                    }
                    ActivePane::Screensavers => {
                        if app.selected_saver_idx < app.screensavers.len() {
                            app.selected_saver_idx += 1;
                        }
                    }
                },
                KeyCode::Left => {
                    if app.active_pane == ActivePane::Settings {
                        match app.selected_setting_idx {
                            2 => app.adjust_timeout(-1),
                            3 => app.adjust_scale(-0.05),
                            i if i >= 5 => app.adjust_param(i - 5, -0.05),
                            _ => {}
                        }
                    }
                }
                KeyCode::Right => {
                    if app.active_pane == ActivePane::Settings {
                        match app.selected_setting_idx {
                            2 => app.adjust_timeout(1),
                            3 => app.adjust_scale(0.05),
                            i if i >= 5 => app.adjust_param(i - 5, 0.05),
                            _ => {}
                        }
                    }
                }
                KeyCode::Char(' ') | KeyCode::Enter => match app.active_pane {
                    ActivePane::Dashboard => {}
                    ActivePane::Settings => match app.selected_setting_idx {
                        0 => app.toggle_daemon(),
                        1 => app.toggle_idle(),
                        2 => app.adjust_timeout(5),
                        3 => app.adjust_scale(0.1),
                        4 => app.toggle_fps(),
                        i if i >= 5 => app.adjust_param(i - 5, 0.1),
                        _ => {}
                    },
                    ActivePane::Screensavers => {
                        if key.code == KeyCode::Enter {
                            app.select_saver();
                        }
                    }
                },
                KeyCode::Char('c') => {
                    if app.cosmic_de_detected && !app.cosmic_applet_installed {
                        app.install_cosmic_applet();
                    }
                }
                KeyCode::Char('p') => {
                    if app.active_pane == ActivePane::Screensavers {
                        app.preview_saver();
                    }
                }
                KeyCode::Char('r') => {
                    app.refresh_state();
                }
                _ => {}
            }
        }

        if last_tick.elapsed() >= tick_rate {
            app.refresh_state();
            app.tick_count = app.tick_count.wrapping_add(1);
            last_tick = Instant::now();
        }
    }
}
