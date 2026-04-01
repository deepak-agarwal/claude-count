use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::Deserialize;

use crate::auth;
use crate::diagnose;
use crate::localization::Strings;
use crate::models::{UsageData, UsageSection};

const USAGE_URL: &str = "https://api.anthropic.com/api/oauth/usage";
const MESSAGES_URL: &str = "https://api.anthropic.com/v1/messages";
const MODEL_FALLBACK_CHAIN: &[&str] = &["claude-3-haiku-20240307", "claude-haiku-4-5-20251001"];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PollError {
    NoCredentials,
    TokenExpired,
    RequestFailed,
}

#[derive(Deserialize)]
struct UsageResponse {
    five_hour: Option<UsageBucket>,
    seven_day: Option<UsageBucket>,
}

#[derive(Deserialize)]
struct UsageBucket {
    utilization: f64,
    resets_at: Option<String>,
}

pub fn poll() -> Result<UsageData, PollError> {
    let mut creds = match read_credentials() {
        Some(c) => c,
        None => {
            if !auth::is_logged_in() {
                diagnose::log("poll failed: Claude auth status reports logged out");
            }
            diagnose::log("poll failed: no Claude credentials found");
            return Err(PollError::NoCredentials);
        }
    };

    if is_token_expired(creds.expires_at) {
        cli_refresh_token(&creds.path);

        match read_credentials_from_path(&creds.path) {
            Some(refreshed) => creds = refreshed,
            None => {
                diagnose::log("poll failed: credentials still unavailable after refresh attempt");
                return Err(PollError::NoCredentials);
            }
        }

        if is_token_expired(creds.expires_at) {
            diagnose::log("poll failed: token is still expired after refresh attempt");
            return Err(PollError::TokenExpired);
        }
    }

    fetch_usage_with_fallback(&creds.access_token)
}

fn cli_refresh_token(_path: &Path) {
    let Some(claude_path) = resolve_claude_path() else {
        diagnose::log("unable to find claude CLI for token refresh");
        return;
    };

    diagnose::log(format!(
        "attempting macOS Claude token refresh via {}",
        claude_path.display()
    ));

    let mut cmd = Command::new(&claude_path);
    cmd.arg("-p")
        .arg(".")
        .env_remove("CLAUDECODE")
        .env_remove("CLAUDE_CODE_ENTRYPOINT")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());

    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(error) => {
            diagnose::log_error("unable to spawn Claude token refresh", error);
            return;
        }
    };

    wait_for_refresh(&mut child);
}

fn resolve_claude_path() -> Option<PathBuf> {
    let candidates = vec![auth::resolve_claude_path()];

    candidates.into_iter().find(|candidate| {
        Command::new(candidate)
            .arg("--version")
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .is_ok()
    })
}

fn wait_for_refresh(child: &mut std::process::Child) {
    let start = std::time::Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) => {
                if start.elapsed() > Duration::from_secs(30) {
                    let _ = child.kill();
                    break;
                }
                std::thread::sleep(Duration::from_millis(500));
            }
            Err(_) => break,
        }
    }
}

fn build_agent() -> Result<ureq::Agent, PollError> {
    let tls = native_tls::TlsConnector::new().map_err(|_| PollError::RequestFailed)?;
    Ok(ureq::AgentBuilder::new()
        .timeout(Duration::from_secs(30))
        .tls_connector(std::sync::Arc::new(tls))
        .build())
}

fn fetch_usage_with_fallback(token: &str) -> Result<UsageData, PollError> {
    if let Some(data) = try_usage_endpoint(token) {
        if data.session.resets_at.is_none() || data.weekly.resets_at.is_none() {
            if let Ok(fallback) = fetch_usage_via_messages(token) {
                let mut merged = data;
                if merged.session.resets_at.is_none() {
                    merged.session.resets_at = fallback.session.resets_at;
                }
                if merged.weekly.resets_at.is_none() {
                    merged.weekly.resets_at = fallback.weekly.resets_at;
                }
                return Ok(merged);
            }
        }
        return Ok(data);
    }

    let result = fetch_usage_via_messages(token);
    if result.is_err() {
        diagnose::log("usage endpoint and Messages API fallback both failed");
    }
    result
}

fn try_usage_endpoint(token: &str) -> Option<UsageData> {
    let agent = build_agent().ok()?;

    let resp = match agent
        .get(USAGE_URL)
        .set("Authorization", &format!("Bearer {token}"))
        .set("anthropic-beta", "oauth-2025-04-20")
        .call()
    {
        Ok(resp) => resp,
        _ => return None,
    };

    let response: UsageResponse = resp.into_json().ok()?;
    let mut data = UsageData::default();

    if let Some(bucket) = &response.five_hour {
        data.session.percentage = bucket.utilization;
        data.session.resets_at = parse_iso8601(bucket.resets_at.as_deref());
    }

    if let Some(bucket) = &response.seven_day {
        data.weekly.percentage = bucket.utilization;
        data.weekly.resets_at = parse_iso8601(bucket.resets_at.as_deref());
    }

    Some(data)
}

fn fetch_usage_via_messages(token: &str) -> Result<UsageData, PollError> {
    let agent = build_agent()?;

    for model in MODEL_FALLBACK_CHAIN {
        let body = serde_json::json!({
            "model": model,
            "max_tokens": 1,
            "messages": [{"role": "user", "content": "."}]
        });

        let response = match agent
            .post(MESSAGES_URL)
            .set("Authorization", &format!("Bearer {token}"))
            .set("anthropic-version", "2023-06-01")
            .set("anthropic-beta", "oauth-2025-04-20")
            .send_json(&body)
        {
            Ok(resp) => resp,
            Err(ureq::Error::Status(_code, resp)) => resp,
            Err(_) => continue,
        };

        let h5 = response.header("anthropic-ratelimit-unified-5h-utilization");
        let h7 = response.header("anthropic-ratelimit-unified-7d-utilization");
        let hs = response.header("anthropic-ratelimit-unified-status");

        if h5.is_some() || h7.is_some() || hs.is_some() {
            return Ok(parse_rate_limit_headers(&response));
        }
    }

    Err(PollError::RequestFailed)
}

fn parse_rate_limit_headers(response: &ureq::Response) -> UsageData {
    let mut data = UsageData::default();

    data.session.percentage =
        get_header_f64(response, "anthropic-ratelimit-unified-5h-utilization") * 100.0;
    data.session.resets_at = unix_to_system_time(get_header_i64(
        response,
        "anthropic-ratelimit-unified-5h-reset",
    ));

    data.weekly.percentage =
        get_header_f64(response, "anthropic-ratelimit-unified-7d-utilization") * 100.0;
    data.weekly.resets_at = unix_to_system_time(get_header_i64(
        response,
        "anthropic-ratelimit-unified-7d-reset",
    ));

    let overall_reset = get_header_i64(response, "anthropic-ratelimit-unified-reset");

    if data.session.percentage == 0.0 && data.weekly.percentage == 0.0 {
        let status = response.header("anthropic-ratelimit-unified-status");
        if status == Some("rejected") {
            let claim = response.header("anthropic-ratelimit-unified-representative-claim");
            match claim {
                Some("five_hour") => data.session.percentage = 100.0,
                Some("seven_day") => data.weekly.percentage = 100.0,
                _ => {}
            }
        }

        if data.session.resets_at.is_none() && overall_reset.is_some() {
            data.session.resets_at = unix_to_system_time(overall_reset);
        }
    }

    data
}

fn get_header_f64(response: &ureq::Response, name: &str) -> f64 {
    response
        .header(name)
        .and_then(|s| s.parse::<f64>().ok())
        .unwrap_or(0.0)
}

fn get_header_i64(response: &ureq::Response, name: &str) -> Option<i64> {
    response.header(name).and_then(|s| s.parse::<i64>().ok())
}

fn unix_to_system_time(unix_secs: Option<i64>) -> Option<SystemTime> {
    let secs = unix_secs?;
    if secs < 0 {
        return None;
    }
    Some(UNIX_EPOCH + Duration::from_secs(secs as u64))
}

struct Credentials {
    access_token: String,
    expires_at: Option<i64>,
    path: PathBuf,
}

fn read_credentials() -> Option<Credentials> {
    if let Some(creds) = read_keychain_credentials() {
        return Some(creds);
    }

    let cred_path = auth::credentials_path()?;
    read_credentials_from_path(&cred_path)
}

fn read_credentials_from_path(path: &Path) -> Option<Credentials> {
    let content = match std::fs::read_to_string(path) {
        Ok(content) => content,
        Err(error) => {
            if diagnose::is_enabled() {
                diagnose::log_error(
                    &format!("unable to read credentials at {}", path.display()),
                    error,
                );
            }
            return None;
        }
    };

    parse_credentials(&content, path.to_path_buf())
}

fn read_keychain_credentials() -> Option<Credentials> {
    for service in ["Claude Code-credentials", "Claude Code"] {
        let output = Command::new("security")
            .args(["find-generic-password", "-s", service, "-w"])
            .output()
            .ok()?;

        if !output.status.success() {
            continue;
        }

        let content = decode_keychain_payload(&output.stdout)?;
        if let Some(creds) =
            parse_credentials(&content, PathBuf::from(format!("keychain:{service}")))
        {
            return Some(creds);
        }
    }

    None
}

fn decode_keychain_payload(bytes: &[u8]) -> Option<String> {
    let text = String::from_utf8_lossy(bytes).trim().to_string();
    if text.is_empty() {
        return None;
    }

    if text.starts_with('{') {
        return Some(text);
    }

    let hex = text
        .strip_prefix("0x")
        .or_else(|| text.strip_prefix("0X"))
        .unwrap_or(&text);

    if !hex.len().is_multiple_of(2) || !hex.chars().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }

    let mut decoded = Vec::with_capacity(hex.len() / 2);
    for pair in hex.as_bytes().chunks_exact(2) {
        let pair_str = std::str::from_utf8(pair).ok()?;
        decoded.push(u8::from_str_radix(pair_str, 16).ok()?);
    }

    String::from_utf8(decoded).ok()
}

fn parse_credentials(content: &str, path: PathBuf) -> Option<Credentials> {
    let json: serde_json::Value = serde_json::from_str(content).ok()?;
    let oauth = json.get("claudeAiOauth")?;
    let access_token = oauth
        .get("accessToken")
        .and_then(|value| value.as_str())?
        .to_string();
    let expires_at = oauth.get("expiresAt").and_then(|value| value.as_i64());

    Some(Credentials {
        access_token,
        expires_at,
        path,
    })
}

fn is_token_expired(expires_at: Option<i64>) -> bool {
    let Some(exp) = expires_at else { return false };
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64;
    now >= exp
}

fn parse_iso8601(s: Option<&str>) -> Option<SystemTime> {
    let s = s?;
    let (datetime_part, offset_secs) = split_iso8601_offset(s)?;

    let formats = ["%Y-%m-%dT%H:%M:%S%.f", "%Y-%m-%dT%H:%M:%S"];
    for fmt in &formats {
        if let Ok(secs) = parse_datetime_to_unix(datetime_part, fmt) {
            let secs = secs as i64 - offset_secs;
            if secs < 0 {
                return None;
            }
            return Some(UNIX_EPOCH + Duration::from_secs(secs as u64));
        }
    }
    None
}

fn split_iso8601_offset(s: &str) -> Option<(&str, i64)> {
    if let Some(datetime) = s.strip_suffix('Z') {
        return Some((datetime, 0));
    }

    let timezone_idx = s
        .char_indices()
        .skip_while(|(idx, _)| *idx < 19)
        .find_map(|(idx, ch)| (ch == '+' || ch == '-').then_some((idx, ch)));

    let Some((idx, sign)) = timezone_idx else {
        return Some((s, 0));
    };

    let datetime = &s[..idx];
    let offset = &s[idx + 1..];
    let (hours, minutes) = offset.split_once(':')?;
    let hours: i64 = hours.parse().ok()?;
    let minutes: i64 = minutes.parse().ok()?;
    let offset_secs = hours
        .checked_mul(3_600)?
        .checked_add(minutes.checked_mul(60)?)?;

    let adjusted = match sign {
        '+' => offset_secs,
        '-' => -offset_secs,
        _ => return None,
    };

    Some((datetime, adjusted))
}

fn parse_datetime_to_unix(s: &str, _fmt: &str) -> Result<u64, ()> {
    let (date_str, time_str) = s.split_once('T').ok_or(())?;
    let date_parts: Vec<&str> = date_str.split('-').collect();
    if date_parts.len() != 3 {
        return Err(());
    }

    let year: u64 = date_parts[0].parse().map_err(|_| ())?;
    let month: u64 = date_parts[1].parse().map_err(|_| ())?;
    let day: u64 = date_parts[2].parse().map_err(|_| ())?;

    let time_base = time_str.split('.').next().unwrap_or(time_str);
    let time_parts: Vec<&str> = time_base.split(':').collect();
    if time_parts.len() != 3 {
        return Err(());
    }

    let hour: u64 = time_parts[0].parse().map_err(|_| ())?;
    let min: u64 = time_parts[1].parse().map_err(|_| ())?;
    let sec: u64 = time_parts[2].parse().map_err(|_| ())?;

    let mut days: u64 = 0;
    for y in 1970..year {
        days += if is_leap(y) { 366 } else { 365 };
    }

    let month_days = [0, 31, 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31];
    for m in 1..month {
        days += month_days[m as usize];
        if m == 2 && is_leap(year) {
            days += 1;
        }
    }
    days += day - 1;

    Ok(days * 86_400 + hour * 3_600 + min * 60 + sec)
}

fn is_leap(y: u64) -> bool {
    (y.is_multiple_of(4) && !y.is_multiple_of(100)) || y.is_multiple_of(400)
}

pub fn format_line(section: &UsageSection, strings: Strings) -> String {
    let pct = format!("{:.0}%", section.percentage);
    let cd = format_countdown(section.resets_at, strings);
    if cd.is_empty() {
        pct
    } else {
        format!("{pct} \u{00b7} {cd}")
    }
}

fn format_countdown(resets_at: Option<SystemTime>, strings: Strings) -> String {
    let reset = match resets_at {
        Some(t) => t,
        None => return String::new(),
    };

    let remaining = match reset.duration_since(SystemTime::now()) {
        Ok(d) => d,
        Err(_) => return strings.now.to_string(),
    };

    format_countdown_from_secs(remaining.as_secs(), strings)
}

pub fn time_until_display_change(resets_at: Option<SystemTime>) -> Option<Duration> {
    let reset = resets_at?;
    let remaining = reset.duration_since(SystemTime::now()).ok()?;
    Some(time_until_display_change_from_secs(remaining.as_secs()))
}

fn format_countdown_from_secs(total_secs: u64, strings: Strings) -> String {
    let total_mins = total_secs / 60;
    let total_hours = total_secs / 3_600;
    let total_days = total_secs / 86_400;

    if total_days >= 1 {
        format!("{total_days}{}", strings.day_suffix)
    } else if total_hours >= 1 {
        format!("{total_hours}{}", strings.hour_suffix)
    } else if total_mins >= 1 {
        format!("{total_mins}{}", strings.minute_suffix)
    } else {
        format!("{total_secs}{}", strings.second_suffix)
    }
}

fn time_until_display_change_from_secs(total_secs: u64) -> Duration {
    let total_mins = total_secs / 60;
    let total_hours = total_secs / 3_600;
    let total_days = total_secs / 86_400;

    let current_bucket_start = if total_days >= 1 {
        total_days * 86_400
    } else if total_hours >= 1 {
        total_hours * 3_600
    } else if total_mins >= 1 {
        total_mins * 60
    } else {
        total_secs
    };

    Duration::from_secs(total_secs.saturating_sub(current_bucket_start) + 1)
}

pub fn is_past_reset(data: &UsageData) -> bool {
    let now = SystemTime::now();
    let past = |s: &UsageSection| matches!(s.resets_at, Some(t) if now.duration_since(t).is_ok());
    past(&data.session) || past(&data.weekly)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::localization::LanguageId;

    #[test]
    fn decodes_plain_json_keychain_payload() {
        let payload = br#"{"claudeAiOauth":{"accessToken":"token"}}"#;
        assert_eq!(
            decode_keychain_payload(payload).as_deref(),
            Some(r#"{"claudeAiOauth":{"accessToken":"token"}}"#)
        );
    }

    #[test]
    fn decodes_hex_keychain_payload() {
        let payload =
            b"7b22636c6175646541694f61757468223a7b22616363657373546f6b656e223a22746f6b656e227d7d";
        assert_eq!(
            decode_keychain_payload(payload).as_deref(),
            Some(r#"{"claudeAiOauth":{"accessToken":"token"}}"#)
        );
    }

    #[test]
    fn rejects_invalid_hex_keychain_payload() {
        assert!(decode_keychain_payload(b"abc").is_none());
        assert!(decode_keychain_payload(b"zz").is_none());
    }

    #[test]
    fn parses_credentials_with_expected_shape() {
        let path = PathBuf::from("test");
        let creds = parse_credentials(
            r#"{"claudeAiOauth":{"accessToken":"abc","expiresAt":123}}"#,
            path.clone(),
        )
        .expect("credentials should parse");

        assert_eq!(creds.access_token, "abc");
        assert_eq!(creds.expires_at, Some(123));
        assert_eq!(creds.path, path);
    }

    #[test]
    fn rejects_credentials_without_access_token() {
        assert!(parse_credentials(r#"{"claudeAiOauth":{}}"#, PathBuf::from("test")).is_none());
    }

    #[test]
    fn parses_iso8601_timestamps_with_fractional_seconds() {
        let parsed =
            parse_iso8601(Some("2026-04-01T12:34:56.789Z")).expect("timestamp should parse");
        let unix = parsed
            .duration_since(UNIX_EPOCH)
            .expect("timestamp should be after epoch")
            .as_secs();

        assert_eq!(unix, 1_775_046_896);
    }

    #[test]
    fn parses_iso8601_timestamps_with_offsets() {
        let plus_offset = parse_iso8601(Some("2026-04-01T12:34:56+02:30"))
            .expect("timestamp with positive offset should parse");
        let minus_offset = parse_iso8601(Some("2026-04-01T12:34:56-02:30"))
            .expect("timestamp with negative offset should parse");

        assert_eq!(
            plus_offset
                .duration_since(UNIX_EPOCH)
                .expect("timestamp should be after epoch")
                .as_secs(),
            1_775_037_896
        );
        assert_eq!(
            minus_offset
                .duration_since(UNIX_EPOCH)
                .expect("timestamp should be after epoch")
                .as_secs(),
            1_775_055_896
        );
    }

    #[test]
    fn formats_countdown_by_bucket() {
        let strings = LanguageId::English.strings();
        assert_eq!(format_countdown_from_secs(2 * 86_400 + 42, strings), "2d");
        assert_eq!(format_countdown_from_secs(3_600 + 9, strings), "1h");
        assert_eq!(format_countdown_from_secs(61, strings), "1m");
        assert_eq!(format_countdown_from_secs(59, strings), "59s");
    }

    #[test]
    fn computes_next_display_change_boundary() {
        assert_eq!(
            time_until_display_change_from_secs(2 * 86_400 + 42),
            Duration::from_secs(43)
        );
        assert_eq!(
            time_until_display_change_from_secs(3_600 + 9),
            Duration::from_secs(10)
        );
        assert_eq!(
            time_until_display_change_from_secs(61),
            Duration::from_secs(2)
        );
        assert_eq!(
            time_until_display_change_from_secs(0),
            Duration::from_secs(1)
        );
    }

    #[test]
    fn leap_year_logic_matches_gregorian_rules() {
        assert!(is_leap(2024));
        assert!(!is_leap(2100));
        assert!(is_leap(2400));
    }
}
