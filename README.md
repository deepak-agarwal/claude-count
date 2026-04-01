# Claude Count

Claude Count is a small macOS menu bar app that shows your current Claude usage.

## What It Does

- Shows your current `5h` usage
- Shows your current `7d` usage
- Refreshes automatically in the menu bar
- Prompts you to sign in with Claude Code if needed

## Requirements

- macOS
- Claude Code installed
- Signed in with `claude auth login`

## Install

1. Download the latest `.dmg` from the Releases page.
2. Open the `.dmg`.
3. Drag `Claude Count.app` into `Applications`.
4. Open the app.

If macOS blocks the downloaded app, developer users can build and install it with Homebrew instead:

```bash
brew tap deepak-agarwal/claude-count https://github.com/deepak-agarwal/claude-count
brew install --HEAD deepak-agarwal/claude-count/claude-count
claude-count
```

## Use

Once the app is running, it lives in the macOS menu bar.

- `Refresh` checks usage immediately
- `Sign In to Claude` opens the Claude Code login flow if you are signed out
- `Quit` exits the app

## License

MIT
