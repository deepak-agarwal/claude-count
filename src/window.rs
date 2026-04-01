use std::path::PathBuf;
use std::process::Command;
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use tray_icon::menu::{Menu, MenuEvent, MenuId, MenuItem, PredefinedMenuItem};
use tray_icon::{Icon, TrayIcon, TrayIconBuilder};
use winit::application::ApplicationHandler;
use winit::event_loop::{ActiveEventLoop, EventLoop, EventLoopProxy};

use crate::auth;
use crate::diagnose;
use crate::localization::{self, LanguageId};
use crate::models::UsageData;
use crate::poller::{self, PollError};

const POLL_1_MIN: u32 = 60_000;
const POLL_15_MIN: u32 = 900_000;
const POLL_1_HOUR: u32 = 3_600_000;

const STATUS_5H_ID: &str = "status_5h";
const STATUS_7D_ID: &str = "status_7d";
const REFRESH_NOW_ID: &str = "refresh_now";
const REFRESH_STATUS_ID: &str = "refresh_status";
const REVEAL_STATUS_ID: &str = "reveal_status";
const LOGIN_ID: &str = "login";
const QUIT_ID: &str = "quit";

#[derive(Clone, Debug, Serialize, Deserialize)]
struct SettingsFile {
    #[serde(default = "default_poll_interval")]
    poll_interval_ms: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    language: Option<String>,
}

impl Default for SettingsFile {
    fn default() -> Self {
        Self {
            poll_interval_ms: default_poll_interval(),
            language: None,
        }
    }
}

#[derive(Debug, Serialize)]
struct StatusFile {
    session_percent: f64,
    session_text: String,
    weekly_percent: f64,
    weekly_text: String,
    updated_at_unix: u64,
    status: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    login_state: Option<String>,
}

#[derive(Debug)]
enum UserEvent {
    Menu(MenuId),
    Poll(PollMessage),
}

#[derive(Debug)]
enum PollMessage {
    Usage {
        data: UsageData,
        session_text: String,
        weekly_text: String,
        summary: String,
        next_refresh_in: Duration,
        should_notify_reset: bool,
    },
    Error {
        error: PollError,
        message: String,
    },
}

struct App {
    language: LanguageId,
    tray: Option<TrayIcon>,
    menu: Option<Menu>,
    status_5h: Option<MenuItem>,
    status_7d: Option<MenuItem>,
    refresh_now_item: Option<MenuItem>,
    refresh_status_item: Option<MenuItem>,
    reveal_status_item: Option<MenuItem>,
    login_item: Option<MenuItem>,
    quit_item: Option<MenuItem>,
    poll_tx: Sender<PollRequest>,
    login_prompt_shown: bool,
}

#[derive(Debug)]
enum PollRequest {
    PollNow,
}

fn default_poll_interval() -> u32 {
    POLL_15_MIN
}

fn app_support_dir() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("Library")
        .join("Application Support")
        .join("ClaudeCodeUsageMonitor")
}

fn settings_path() -> PathBuf {
    app_support_dir().join("settings.json")
}

fn status_path() -> PathBuf {
    app_support_dir().join("status.json")
}

fn load_settings() -> SettingsFile {
    let content = match std::fs::read_to_string(settings_path()) {
        Ok(content) => content,
        Err(_) => return SettingsFile::default(),
    };
    serde_json::from_str(&content).unwrap_or_default()
}

fn save_status(status: &StatusFile) {
    let path = status_path();
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Ok(json) = serde_json::to_string_pretty(status) {
        let _ = std::fs::write(path, json);
    }
}

fn now_unix_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

fn sanitize_for_applescript(value: &str) -> String {
    value.replace('\\', "\\\\").replace('"', "\\\"")
}

fn notify(title: &str, body: &str) {
    let title = sanitize_for_applescript(title);
    let body = sanitize_for_applescript(body);
    let script = format!("display notification \"{body}\" with title \"{title}\"");
    let _ = Command::new("osascript")
        .arg("-e")
        .arg(script)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn();
}

fn launch_claude_login() {
    auth::launch_login();
}

fn show_login_dialog(message: &str) {
    let message = sanitize_for_applescript(message);
    let script = format!(
        "display dialog \"{}\" buttons {{\"Not Now\", \"Sign In\"}} default button \"Sign In\" with title \"Claude Code Usage Monitor\"",
        message
    );
    thread::spawn(move || {
        let output = Command::new("osascript").arg("-e").arg(script).output();

        let Ok(output) = output else {
            return;
        };

        let stdout = String::from_utf8_lossy(&output.stdout);
        if stdout.contains("Sign In") {
            launch_claude_login();
        }
    });
}

fn language_from_settings(settings: &SettingsFile) -> LanguageId {
    localization::resolve_language(settings.language.as_deref().and_then(LanguageId::from_code))
}

fn progress_bar(percent: f64, width: usize) -> String {
    let filled = ((percent.clamp(0.0, 100.0) / 100.0) * width as f64).round() as usize;
    let filled = filled.min(width);
    let empty = width.saturating_sub(filled);
    format!("{}{}", "█".repeat(filled), "░".repeat(empty))
}

fn build_progress_icon(session_percent: f64) -> Result<Icon, String> {
    const WIDTH: usize = 214;
    const HEIGHT: usize = 24;
    const CELL: usize = 17;
    const GAP: usize = 3;
    const LEFT: usize = 4;
    const TOP_SESSION: usize = 3;
    const COLS: usize = 10;

    let mut rgba = vec![0u8; WIDTH * HEIGHT * 4];

    let paint_rounded =
        |rgba: &mut [u8], x: usize, y: usize, w: usize, h: usize, radius: usize, color: [u8; 4]| {
            let radius = radius.min(w / 2).min(h / 2);
            let radius_sq = (radius * radius) as isize;

            for yy in y..(y + h).min(HEIGHT) {
                for xx in x..(x + w).min(WIDTH) {
                    let local_x = xx.saturating_sub(x);
                    let local_y = yy.saturating_sub(y);

                    let dx = if local_x < radius {
                        radius as isize - local_x as isize - 1
                    } else if local_x >= w.saturating_sub(radius) {
                        local_x as isize - (w.saturating_sub(radius)) as isize
                    } else {
                        0
                    };

                    let dy = if local_y < radius {
                        radius as isize - local_y as isize - 1
                    } else if local_y >= h.saturating_sub(radius) {
                        local_y as isize - (h.saturating_sub(radius)) as isize
                    } else {
                        0
                    };

                    if dx == 0 || dy == 0 || (dx * dx + dy * dy) <= radius_sq {
                        let idx = (yy * WIDTH + xx) * 4;
                        rgba[idx..idx + 4].copy_from_slice(&color);
                    }
                }
            }
        };

    let draw_row = |rgba: &mut [u8], top: usize, percent: f64, fill: [u8; 4], empty: [u8; 4]| {
        let active = ((percent.clamp(0.0, 100.0) / 100.0) * COLS as f64).round() as usize;
        for col in 0..COLS {
            let x = LEFT + col * (CELL + GAP);
            let y = top;
            let color = if col < active { fill } else { empty };
            paint_rounded(rgba, x, y + 1, CELL, CELL, 4, [0, 0, 0, 42]);
            paint_rounded(rgba, x, y, CELL, CELL, 4, [255, 255, 255, 50]);
            paint_rounded(rgba, x, y, CELL, CELL, 4, color);
        }
    };

    draw_row(
        &mut rgba,
        TOP_SESSION,
        session_percent,
        [255, 107, 0, 255],
        [255, 107, 0, 64],
    );

    Icon::from_rgba(rgba, WIDTH as u32, HEIGHT as u32)
        .map_err(|e| format!("Unable to build menu bar progress icon: {e}"))
}

fn format_duration_short(duration: Duration) -> String {
    let secs = duration.as_secs();
    if secs >= 3600 {
        format!("{}h", secs / 3600)
    } else if secs >= 60 {
        format!("{}m", secs / 60)
    } else {
        format!("{}s", secs)
    }
}

fn format_summary(data: &UsageData, language: LanguageId) -> (String, String, String) {
    let strings = language.strings();
    let session_text = poller::format_line(&data.session, strings);
    let weekly_text = poller::format_line(&data.weekly, strings);
    let title = String::new();
    (session_text, weekly_text, title)
}

fn save_error_status(error: &str) {
    let login_state = auth::status().ok().map(|status| {
        if status.logged_in {
            "logged_in".to_string()
        } else {
            "logged_out".to_string()
        }
    });

    save_status(&StatusFile {
        session_percent: 0.0,
        session_text: "Unavailable".to_string(),
        weekly_percent: 0.0,
        weekly_text: "Unavailable".to_string(),
        updated_at_unix: now_unix_secs(),
        status: "error",
        error: Some(error.to_string()),
        login_state,
    });
}

fn poll_sleep_duration(settings: &SettingsFile, data: Option<&UsageData>) -> Duration {
    let mut delay_ms = settings.poll_interval_ms.clamp(POLL_1_MIN, POLL_1_HOUR);
    if let Some(data) = data {
        let countdown_delay = [
            poller::time_until_display_change(data.session.resets_at),
            poller::time_until_display_change(data.weekly.resets_at),
        ]
        .into_iter()
        .flatten()
        .min();

        if let Some(delay) = countdown_delay {
            let countdown_ms = delay.as_millis().min(u32::MAX as u128) as u32;
            delay_ms = delay_ms.min(countdown_ms.max(1_000));
        }
    }

    Duration::from_millis(delay_ms as u64)
}

fn updated_menu_text(updated_at_unix: u64, next_refresh_in: Option<Duration>) -> String {
    let refresh = next_refresh_in
        .map(|duration| format!("  •  next {}", format_duration_short(duration)))
        .unwrap_or_default();

    match Command::new("date")
        .args(["-r", &updated_at_unix.to_string(), "+Updated %I:%M %p"])
        .output()
    {
        Ok(output) if output.status.success() => {
            format!(
                "{}{}",
                String::from_utf8_lossy(&output.stdout).trim(),
                refresh
            )
        }
        _ => format!("Updated just now{refresh}"),
    }
}

fn spawn_poller(
    proxy: EventLoopProxy<UserEvent>,
    settings: SettingsFile,
    language: LanguageId,
) -> Sender<PollRequest> {
    let (tx, rx): (Sender<PollRequest>, Receiver<PollRequest>) = mpsc::channel();

    thread::spawn(move || {
        let mut last_reset_state = false;
        loop {
            while matches!(rx.try_recv(), Ok(PollRequest::PollNow)) {}

            match poller::poll() {
                Ok(data) => {
                    let (session_text, weekly_text, summary) = format_summary(&data, language);
                    let sleep_for = poll_sleep_duration(&settings, Some(&data));
                    save_status(&StatusFile {
                        session_percent: data.session.percentage,
                        session_text: session_text.clone(),
                        weekly_percent: data.weekly.percentage,
                        weekly_text: weekly_text.clone(),
                        updated_at_unix: now_unix_secs(),
                        status: "ok",
                        error: None,
                        login_state: Some("logged_in".to_string()),
                    });

                    let current_reset_state = poller::is_past_reset(&data);
                    let should_notify_reset = current_reset_state && !last_reset_state;
                    last_reset_state = current_reset_state;

                    let _ = proxy.send_event(UserEvent::Poll(PollMessage::Usage {
                        data: data.clone(),
                        session_text,
                        weekly_text,
                        summary,
                        next_refresh_in: sleep_for,
                        should_notify_reset,
                    }));
                    match rx.recv_timeout(sleep_for) {
                        Ok(PollRequest::PollNow) => continue,
                        Err(mpsc::RecvTimeoutError::Timeout) => continue,
                        Err(mpsc::RecvTimeoutError::Disconnected) => break,
                    }
                }
                Err(error) => {
                    let error_text = format!("{error:?}");
                    save_error_status(&error_text);
                    let _ = proxy.send_event(UserEvent::Poll(PollMessage::Error {
                        error,
                        message: error_text,
                    }));
                    last_reset_state = false;

                    let retry_delay = match error {
                        PollError::NoCredentials | PollError::TokenExpired => {
                            Duration::from_secs(5)
                        }
                        PollError::RequestFailed => Duration::from_secs(30),
                    };

                    match rx.recv_timeout(retry_delay) {
                        Ok(PollRequest::PollNow) => continue,
                        Err(mpsc::RecvTimeoutError::Timeout) => continue,
                        Err(mpsc::RecvTimeoutError::Disconnected) => break,
                    }
                }
            }
        }
    });

    tx
}

impl App {
    fn new(proxy: EventLoopProxy<UserEvent>) -> Self {
        let settings = load_settings();
        let language = language_from_settings(&settings);
        let poll_tx = spawn_poller(proxy.clone(), SettingsFile { ..settings.clone() }, language);
        Self {
            language,
            tray: None,
            menu: None,
            status_5h: None,
            status_7d: None,
            refresh_now_item: None,
            refresh_status_item: None,
            reveal_status_item: None,
            login_item: None,
            quit_item: None,
            poll_tx,
            login_prompt_shown: false,
        }
    }

    fn build_menu(&mut self) -> Result<(), String> {
        let strings = self.language.strings();
        let menu = Menu::new();

        let status_5h = MenuItem::with_id(
            MenuId::new(STATUS_5H_ID),
            format!("{}: Loading...", strings.session_window),
            false,
            None,
        );
        let status_7d = MenuItem::with_id(
            MenuId::new(STATUS_7D_ID),
            format!("{}: Loading...", strings.weekly_window),
            false,
            None,
        );
        let refresh_now =
            MenuItem::with_id(MenuId::new(REFRESH_NOW_ID), strings.refresh, true, None);
        let refresh_status = MenuItem::with_id(
            MenuId::new(REFRESH_STATUS_ID),
            "Updated just now",
            false,
            None,
        );
        let reveal_status = MenuItem::with_id(
            MenuId::new(REVEAL_STATUS_ID),
            "Reveal Status File",
            true,
            None,
        );
        let login = MenuItem::with_id(MenuId::new(LOGIN_ID), "Sign In to Claude", true, None);
        let quit = MenuItem::with_id(MenuId::new(QUIT_ID), strings.exit, true, None);

        menu.append(&status_5h)
            .map_err(|e| format!("Unable to build menu: {e}"))?;
        menu.append(&status_7d)
            .map_err(|e| format!("Unable to build menu: {e}"))?;
        menu.append(&refresh_now)
            .map_err(|e| format!("Unable to build menu: {e}"))?;
        menu.append(&refresh_status)
            .map_err(|e| format!("Unable to build menu: {e}"))?;
        menu.append(&reveal_status)
            .map_err(|e| format!("Unable to build menu: {e}"))?;
        menu.append(&login)
            .map_err(|e| format!("Unable to build menu: {e}"))?;
        menu.append(&PredefinedMenuItem::separator())
            .map_err(|e| format!("Unable to build menu: {e}"))?;
        menu.append(&quit)
            .map_err(|e| format!("Unable to build menu: {e}"))?;

        self.status_5h = Some(status_5h);
        self.status_7d = Some(status_7d);
        self.refresh_now_item = Some(refresh_now);
        self.refresh_status_item = Some(refresh_status);
        self.reveal_status_item = Some(reveal_status);
        self.login_item = Some(login);
        self.quit_item = Some(quit);
        self.menu = Some(menu);
        Ok(())
    }

    fn build_tray(&mut self) -> Result<(), String> {
        self.build_menu()?;
        let icon = build_progress_icon(0.0)?;
        let tray = TrayIconBuilder::new()
            .with_menu(Box::new(
                self.menu.take().ok_or_else(|| "Menu missing".to_string())?,
            ))
            .with_tooltip("Claude Code Usage Monitor")
            .with_icon(icon)
            .build()
            .map_err(|e| format!("Unable to create tray icon: {e}"))?;
        self.tray = Some(tray);
        Ok(())
    }

    fn set_status_labels(
        &self,
        session_percent: f64,
        weekly_percent: f64,
        session_text: &str,
        weekly_text: &str,
    ) {
        if let Some(item) = &self.status_5h {
            item.set_text(format!(
                "{}  {}  {}",
                self.language.strings().session_window,
                progress_bar(session_percent, 10),
                session_text
            ));
        }
        if let Some(item) = &self.status_7d {
            item.set_text(format!(
                "{}  {}  {}",
                self.language.strings().weekly_window,
                progress_bar(weekly_percent, 10),
                weekly_text
            ));
        }
    }

    fn set_progress_icon(&self, session_percent: f64, weekly_percent: f64) {
        if let Some(tray) = &self.tray {
            if let Ok(icon) = build_progress_icon(session_percent) {
                let _ = tray.set_icon(Some(icon));
            }
            let _ = tray.set_tooltip(Some(&format!(
                "5h {:.0}%\n7d {:.0}%\n{}",
                session_percent,
                weekly_percent,
                status_path().display()
            )));
        }
    }

    fn set_updated_label(&self, updated_at_unix: u64, next_refresh_in: Option<Duration>) {
        if let Some(item) = &self.refresh_status_item {
            item.set_text(updated_menu_text(updated_at_unix, next_refresh_in));
        }
    }

    fn set_login_menu_state(&self, enabled: bool, text: &str) {
        if let Some(item) = &self.login_item {
            item.set_text(text);
            item.set_enabled(enabled);
        }
    }

    fn request_poll(&self) {
        let _ = self.poll_tx.send(PollRequest::PollNow);
    }

    fn reveal_status_file(&self) {
        let path = status_path();
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        if !path.exists() {
            let _ = std::fs::write(
                &path,
                "{\n  \"status\": \"pending\",\n  \"message\": \"Waiting for first successful poll\"\n}\n",
            );
        }

        let _ = Command::new("open")
            .args(["-R", &path.to_string_lossy()])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn();
    }

    fn handle_poll_message(&mut self, message: PollMessage) {
        match message {
            PollMessage::Usage {
                data,
                session_text,
                weekly_text,
                summary,
                next_refresh_in,
                should_notify_reset,
            } => {
                self.login_prompt_shown = false;
                self.set_login_menu_state(false, "Connected");
                self.set_updated_label(now_unix_secs(), Some(next_refresh_in));
                self.set_status_labels(
                    data.session.percentage,
                    data.weekly.percentage,
                    &session_text,
                    &weekly_text,
                );
                self.set_progress_icon(data.session.percentage, data.weekly.percentage);
                diagnose::log(format!("usage updated: {}", summary));
                if should_notify_reset {
                    notify(
                        self.language.strings().window_title,
                        "A Claude usage window has reset.",
                    );
                }
                if data.session.percentage >= 90.0 || data.weekly.percentage >= 90.0 {
                    notify(self.language.strings().window_title, &summary);
                }
            }
            PollMessage::Error { error, message } => {
                diagnose::log(format!("poll failed: {message}"));
                match error {
                    PollError::NoCredentials => {
                        self.set_updated_label(now_unix_secs(), Some(Duration::from_secs(5)));
                        self.set_status_labels(0.0, 0.0, "Sign in required", "Waiting for login");
                        self.set_login_menu_state(true, "Sign In to Claude");
                        self.set_progress_icon(0.0, 0.0);
                        if !self.login_prompt_shown {
                            self.login_prompt_shown = true;
                            show_login_dialog(
                                "Claude Code is not signed in on this Mac. Click Sign In to open a Terminal window and run `claude auth login`. The app will keep checking and connect automatically once login completes.",
                            );
                        }
                    }
                    PollError::TokenExpired => {
                        self.set_updated_label(now_unix_secs(), Some(Duration::from_secs(5)));
                        self.set_status_labels(0.0, 0.0, "Session expired", "Waiting for login");
                        self.set_login_menu_state(true, "Sign In to Claude");
                        self.set_progress_icon(0.0, 0.0);
                        if !self.login_prompt_shown {
                            self.login_prompt_shown = true;
                            show_login_dialog(
                                "Your Claude Code login has expired. Click Sign In to open a Terminal window and run `claude auth login`. The app will keep checking until the refreshed token is available.",
                            );
                        }
                    }
                    PollError::RequestFailed => {
                        self.set_updated_label(now_unix_secs(), Some(Duration::from_secs(30)));
                        self.set_status_labels(0.0, 0.0, "Network error", "Retrying automatically");
                        self.set_login_menu_state(false, "Connected");
                        self.set_progress_icon(0.0, 0.0);
                        notify(
                            self.language.strings().window_title,
                            &format!("Unable to refresh usage: {message}"),
                        );
                    }
                }
            }
        }
    }

    fn handle_menu(&mut self, menu_id: MenuId, event_loop: &ActiveEventLoop) {
        match menu_id.0.as_str() {
            REFRESH_NOW_ID => self.request_poll(),
            REVEAL_STATUS_ID => self.reveal_status_file(),
            LOGIN_ID => launch_claude_login(),
            QUIT_ID => event_loop.exit(),
            _ => {}
        }
    }
}

impl ApplicationHandler<UserEvent> for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.tray.is_some() {
            return;
        }

        if let Err(error) = self.build_tray() {
            diagnose::log_error("unable to start menu bar app", &error);
            notify("Claude Code Usage Monitor", &error);
            event_loop.exit();
            return;
        }

        let _ = self.poll_tx.send(PollRequest::PollNow);
    }

    fn user_event(&mut self, event_loop: &ActiveEventLoop, event: UserEvent) {
        match event {
            UserEvent::Menu(menu_id) => self.handle_menu(menu_id, event_loop),
            UserEvent::Poll(message) => self.handle_poll_message(message),
        }
    }

    fn window_event(
        &mut self,
        _event_loop: &ActiveEventLoop,
        _window_id: winit::window::WindowId,
        _event: winit::event::WindowEvent,
    ) {
    }
}

pub fn run() {
    diagnose::log(format!(
        "macOS menu bar app starting; settings_path={} status_path={}",
        settings_path().display(),
        status_path().display()
    ));

    let event_loop = EventLoop::<UserEvent>::with_user_event()
        .build()
        .expect("failed to create event loop");
    let proxy = event_loop.create_proxy();

    MenuEvent::set_event_handler(Some({
        let proxy = proxy.clone();
        move |event: MenuEvent| {
            let _ = proxy.send_event(UserEvent::Menu(event.id));
        }
    }));

    let mut app = App::new(proxy);
    event_loop.run_app(&mut app).expect("event loop failed");
}
