![macOS](https://img.shields.io/badge/platform-macOS-black)
[![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)](https://opensource.org/licenses/MIT)

# Claude Code Usage Monitor

A lightweight macOS menu bar monitor for people already using Claude Code.

It reads your local Claude credentials, polls your current usage windows, writes the latest snapshot to disk, and shows the current usage in the macOS menu bar.

## What It Does

- Tracks your current **5h** Claude usage window
- Tracks your current **7d** Claude usage window
- Shows live usage in the macOS menu bar
- Refreshes in the background on a configurable interval
- Stores the latest snapshot at `~/Library/Application Support/ClaudeCodeUsageMonitor/status.json`
- Includes menu actions for refresh, sign in, reveal status file, and quit
- Uses native macOS notifications for reset, error, and high-usage events
- Writes diagnostics to a temp log when launched with `--diagnose`
- Detects Claude login state and keeps checking until the user completes sign-in

## Requirements

- macOS
- Claude Code installed and authenticated

The app reads your Claude credentials from:

```text
~/.claude/.credentials.json
```

## Login Flow

If Claude Code is not signed in, the app will:

- Show `Claude Sign In` in the menu bar
- Open a small macOS dialog that offers to sign in
- Launch `claude auth login` in Terminal when the user clicks `Sign In`
- Keep re-checking Claude auth state and attach automatically when login completes

The app does not implement its own auth system. It reuses Claude Code's existing login session and token storage.

## Menu Actions

- `Refresh` triggers an immediate usage poll
- `Updated ...` shows the last successful update time and the next automatic refresh
- `Reveal Status File` opens the latest status snapshot in Finder
- `Sign In to Claude` launches `claude auth login` in Terminal when needed
- `Quit` exits the menu bar app

## Run

From the project root:

```bash
cargo run --release
```

For diagnostics:

```bash
cargo run --release -- --diagnose
```

This writes a log file to:

```text
$TMPDIR/claude-code-usage-monitor.log
```

## Settings

Settings are stored at:

```text
~/Library/Application Support/ClaudeCodeUsageMonitor/settings.json
```

Current supported settings:

```json
{
  "poll_interval_ms": 900000,
  "language": "en"
}
```

## Privacy And Security

This project is open source.

What the app reads:

- Your local Claude Code OAuth credentials from `~/.claude/.credentials.json`

What the app sends over the network:

- Requests to Anthropic endpoints to read your usage and rate-limit information

What the app stores locally:

- Polling settings
- The most recent usage snapshot
- Current login state in the status snapshot

What it does not do:

- It does not upload your credentials anywhere else
- It does not use a separate backend service
- It does not collect analytics
- It does not upload your project files
- It does not store a separate copy of your Claude token

## Notes

- If your Claude token is expired, the app will prompt the user to run `claude auth login` and will reconnect automatically after login succeeds
- The menu bar title shows `5h% / 7d%`

## License

MIT
