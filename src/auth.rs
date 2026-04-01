use std::path::PathBuf;
use std::process::Command;

use serde::Deserialize;

use crate::diagnose;

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AuthStatus {
    pub logged_in: bool,
    #[allow(dead_code)]
    #[serde(default)]
    pub auth_method: Option<String>,
    #[allow(dead_code)]
    #[serde(default)]
    pub api_provider: Option<String>,
}

pub fn credentials_path() -> Option<PathBuf> {
    Some(dirs::home_dir()?.join(".claude").join(".credentials.json"))
}

pub fn resolve_claude_path() -> PathBuf {
    if let Some(path) = std::env::var_os("PATH") {
        for entry in std::env::split_paths(&path) {
            let candidate = entry.join("claude");
            if candidate.exists() {
                return candidate;
            }
        }
    }

    if let Some(home) = dirs::home_dir() {
        for candidate in [
            home.join(".local/bin/claude"),
            home.join(".npm-global/bin/claude"),
        ] {
            if candidate.exists() {
                return candidate;
            }
        }
    }

    PathBuf::from("claude")
}

pub fn status() -> Result<AuthStatus, String> {
    let output = Command::new(resolve_claude_path())
        .args(["auth", "status"])
        .output()
        .map_err(|e| format!("Unable to run `claude auth status`: {e}"))?;

    if !output.status.success() && output.stdout.is_empty() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        return Err(if stderr.is_empty() {
            format!("`claude auth status` exited with {}", output.status)
        } else {
            stderr
        });
    }

    let stdout = String::from_utf8(output.stdout)
        .map_err(|e| format!("Unable to decode `claude auth status` output: {e}"))?;
    serde_json::from_str(&stdout)
        .map_err(|e| format!("Unable to parse `claude auth status` output: {e}"))
}

pub fn is_logged_in() -> bool {
    match status() {
        Ok(status) => status.logged_in,
        Err(error) => {
            diagnose::log_error("auth status check failed", error);
            false
        }
    }
}

pub fn launch_login() {
    let command = format!("\"{}\" auth login", resolve_claude_path().display());
    let script = format!(
        "tell application \"Terminal\"\nactivate\ndo script \"{}\"\nend tell",
        escape_applescript(&command)
    );
    let _ = Command::new("osascript")
        .arg("-e")
        .arg(script)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn();
}

fn escape_applescript(value: &str) -> String {
    value.replace('\\', "\\\\").replace('"', "\\\"")
}
