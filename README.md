# kura

A simple terminal-based two-pane file manager written in Rust using ratatui and crossterm.

## Features

- Dual-pane directory listing
- Vim-like key bindings
- Image preview in the opposite pane (png, jpeg, gif, bmp, tiff, webp; heic/avif via ffmpeg), rendered with the Kitty / Sixel / iTerm2 graphics protocols when available
- Video preview: a thumbnail plus duration / codec / resolution info (requires `ffmpeg` and `ffprobe`)
- Syntax highlighting in the text viewer and in the preview of the opposite pane (`P`), which shows images and videos in place too
- Rendered Markdown (headings, lists, quotes, tables, links, highlighted code blocks) in the text viewer and the preview (`P`); `m` switches to the source

## Supported Platforms

| OS | Architecture | Release asset target |
| --- | --- | --- |
| macOS | Apple Silicon (arm64) | `aarch64-apple-darwin` |
| macOS | Intel (x86_64) | `x86_64-apple-darwin` |
| Linux / WSL2 | x86_64 | `x86_64-unknown-linux-musl` |
| Linux / WSL2 | aarch64 | `aarch64-unknown-linux-musl` |

The Linux binaries are statically linked against musl, so they run on any distribution and under WSL2.

Native Windows is **not supported**. On Windows, use kura inside WSL2.

## Installation

### Prebuilt binaries (GitHub Releases)

Binaries are published on the [Releases page](https://github.com/hanafsky/Kura/releases). Each release has a `kura-<target>.tar.gz` archive and a matching `kura-<target>.tar.gz.sha256` checksum for every target in the table above.

Download with `curl`, verify the checksum, and extract:

```bash
# Pick your target from the table above
TARGET=aarch64-apple-darwin

curl -LO "https://github.com/hanafsky/Kura/releases/latest/download/kura-${TARGET}.tar.gz"
curl -LO "https://github.com/hanafsky/Kura/releases/latest/download/kura-${TARGET}.tar.gz.sha256"

# Verify the checksum (should print "kura-<target>.tar.gz: OK")
shasum -a 256 -c "kura-${TARGET}.tar.gz.sha256"    # macOS
# sha256sum -c "kura-${TARGET}.tar.gz.sha256"      # Linux / WSL2

# Extract; the archive contains a kura-<target>/ directory
tar -xzf "kura-${TARGET}.tar.gz"

# Put the binary somewhere on your PATH
mkdir -p ~/.local/bin
mv "kura-${TARGET}/kura" ~/.local/bin/
```

Make sure `~/.local/bin` (or whichever directory you chose) is on your `PATH`.

#### macOS: binaries downloaded with a browser

Files downloaded with `curl` are not quarantined, so the steps above need nothing extra. If you downloaded the archive with a web browser instead, macOS Gatekeeper may refuse to run `kura` because it is unsigned. Remove the quarantine attribute from the extracted binary:

```bash
xattr -d com.apple.quarantine ~/.local/bin/kura
```

(If it prints `No such xattr`, the attribute was not set and you can ignore it.)

### Build from source with Cargo

With a Rust toolchain installed (e.g. via [rustup](https://rustup.rs/)):

```bash
cargo install --git https://github.com/hanafsky/Kura
```

This installs `kura` into `~/.cargo/bin`.

### ffmpeg (optional)

Video previews and HEIC/AVIF images need `ffmpeg` and `ffprobe` (both ship with the `ffmpeg` package):

```bash
# macOS (Homebrew)
brew install ffmpeg

# Debian / Ubuntu / WSL2
sudo apt install ffmpeg
```

## Key Bindings

- `j` / `k`: Move down / up within the current pane (or scroll down / up in text viewer mode). Supports numeric prefixes (e.g., `4j` moves down 4 lines, `3k` moves up 3 lines). In text viewer mode, relative line numbers are shown beside each line to indicate how many lines to skip with a numeric prefix.
- `gg`: Go to the top of the file list or text viewer (equivalent to `0` prefix then `j`).
- `G`: Go to the bottom of the file list or text viewer (equivalent to a large prefix then `j`). In the text viewer the last line stays on the bottom row when the terminal is resized, until you scroll up again.
- `PageDown` / `PageUp`, `Ctrl-f` / `Ctrl-b`: Scroll the text viewer down / up one screen. `Ctrl-d` / `Ctrl-u` scroll half a screen. A numeric prefix scrolls that many (half) screens (e.g. `3` then `PageDown`). Like `j`, scrolling stops at the last line, and reaching it keeps the last line at the bottom on resize like `G`.
- `V`: Enter visual multi-selection mode (like Vim's Visual Line mode). Use `j`/`k` to select a range of entries; press `Esc` or `V` again to exit visual mode, leaving marked entries.

- **Left pane:**
  - `h`: Go to the parent directory
  - `l`: Switch to the right pane

- **Right pane:**
  - `h`: Switch to the left pane
  - `l`: Go to the parent directory

- `Enter`: Enter the selected directory, open the text viewer for a text file, or show an image / video preview in the opposite pane. Press `Enter` (or `Esc` in preview) again to return to the file manager.
  The text viewer highlights source code; the language is detected from the file name / extension or the first line (shebang such as `#!/usr/bin/env python3`) and shown at the bottom left of the frame. Only the first 2000 lines are highlighted (the rest is shown plain) so large files open without a pause. Files that are not valid UTF-8 cannot be opened in the viewer, and text containing NUL bytes is shown without highlighting.
  In the preview, `j` / `k` (with a count, e.g. `3j`) show the next / previous image or video in the list, skipping folders and other files; the cursor moves along, and the status line tells you when there is no more. Images load in the background ("Loading…" meanwhile), so keys never wait for a large image or a video thumbnail; the previous and next ones are loaded ahead, and up to 5 previews (about 128 MB) are kept until the preview is closed.
- `m`: Switch a Markdown file (`.md`, `.markdown`) between the rendered view and its highlighted source, in the text viewer or in the preview (`P`). Markdown opens rendered; the frame's bottom left shows which view is on. In the preview the choice is kept as the cursor moves to other files, and the status line shows `No Markdown shown` when there is nothing to switch.
  The rendered view keeps the `#` marks of headings (levels 1–3 are coloured), shows list items with `•` / `◦` / `▪` or their number and indents nested lists, prefixes quotes with `│`, draws tables with box lines and columns aligned by display width (a table wider than the pane wraps), shows links as underlined text followed by the URL (images as `[image: alt]`), and highlights fenced code blocks by their language. Long lines wrap between words (Japanese between characters, without starting a row with `、` / `。`) and keep the list indent or quote bar.
- `P`: Toggle a preview of the entry under the cursor in the opposite pane. It follows the cursor as you move (`j` / `k`, search, …) while all file-list keys keep working; press `P` again to close it. Text files are shown with syntax highlighting (only the first 256 KiB is read); directories and binary files show a short note instead. Images and videos are shown in place too (a video shows its thumbnail and info). They load in the background like the `Enter` preview ("Loading…" meanwhile), so moving the cursor quickly never blocks keys; the images / videos before and after the cursor are loaded ahead, and up to 5 previews (about 128 MB, shared with the `Enter` preview) are kept until `P` is closed. `Enter` on an image switches to the `Enter` preview, which steps through images and videos only with `j` / `k`; closing it returns to the `P` preview. While the preview is open, `D` only shows `Close the preview first` in the status line, since the other pane's cursor is hidden.
- `o`: Open the selected entry with the default application (`open` on macOS), e.g. play a video in QuickTime. Works in the file list and in preview.
  On WSL the path is converted with `wslpath -w` and opened by `explorer.exe`. Set `KURA_OPENER` to use another program; it is split on whitespace and run as `<program> <args...> <path>` (e.g. `KURA_OPENER="code -r"` → `code -r <path>`).
- `e`: Open the selected file or directory in your editor (`$VISUAL`, else `$EDITOR`, else `vi`). The value is split on whitespace like `KURA_OPENER` (e.g. `EDITOR="code -w"`). kura leaves the screen while the editor runs and comes back, refreshed, when it exits.
- `v`: Toggle selection of the current entry in file manager mode. Selected entries are marked with a `*` symbol. Marks and the cursor follow the files themselves, so they stay put when the list is refreshed or re-sorted (e.g. after paste, delete, rename or sorting).
- `y`: Copy the current entry or all marked entries to the clipboard.
- `p`: Paste copied entries into the current directory.
- `Y`: Copy the current entry or all marked entries to the system clipboard as `@path` references for coding agents such as Claude Code (e.g. `@src/main.rs @"docs/my notes.md"`). Paths are relative to the directory kura was started in (paths outside it stay absolute), separated by spaces; a path containing spaces is quoted as `@"..."`.
  The text is sent with the OSC 52 escape sequence (so it works through tmux, herdr, or ssh). Because kura cannot tell whether the terminal accepted OSC 52, it is also written with `pbcopy` (macOS), `wl-copy` / `xclip` (Linux), or `clip.exe` (WSL), unless kura runs over ssh (`SSH_CONNECTION` / `SSH_TTY` / `SSH_CLIENT` set), where only OSC 52 is used. Inside tmux, enable `set -g set-clipboard on` in `~/.tmux.conf`. `KURA_CLIPBOARD` accepts `auto` (default, as above), `osc52` (OSC 52 only), or `native` (native tools only); set `KURA_AT_PATH=absolute` to always copy absolute paths.
  Inside [herdr](https://herdr.dev) (`HERDR_ENV=1` with `HERDR_SOCKET_PATH`), `Y` instead types the references into an agent pane through herdr's socket, followed by a space and without pressing Enter. Targets are the agents in kura's workspace other than kura's own pane; with several, a dialog asks which one (`j`/`k`, `Enter`: send, `Esc` / `q`: cancel). Paths are relative to that agent's working directory. Without an agent pane, `Y` copies to the clipboard as above. In Herdr.app (herdr-core, `hc`) `Y` uses the clipboard.
- `M`: Toggle showing only entries with git changes (and directories containing them) in the current pane.
- `.`: Toggle showing hidden entries (names starting with `.`) in the current pane (shown by default). While they are hidden the pane title shows `[no hidden]`.
- `i`: Toggle hiding entries ignored by git (`.gitignore` etc.) in the current pane (only inside a git repository). While active the pane title shows `[no ignored]`. It uses the git status (`--ignored`) read when the directory was loaded, so toggling does not run git. Inside an ignored directory (such as `target/`) nothing is hidden, since everything there is ignored. `.git` is not an ignored entry, so `i` does not hide it (`.` does).
  `.`, `i` and `M` can be combined. Marks on entries hidden by a filter are cleared, so hidden entries are never acted on.
- `D`: Show the diff between the files under the cursors of the left and right panes (`git diff --no-index`), e.g. the same file in the main checkout and in an agent's worktree. Removed lines are red, added lines green and hunk headers (`@@`) cyan. It scrolls like the text viewer (`j` / `k`, numeric prefixes, `gg` / `G`, `PageDown` / `PageUp`, `Ctrl-f` / `Ctrl-b` / `Ctrl-d` / `Ctrl-u`); `Enter`, `q` or `Esc` returns to the file list. If only the active pane's cursor is on a file inside a git repository (e.g. the other pane's cursor is on a directory), or both panes point to the same file inside a repository (e.g. right after starting kura), that file's changes against `HEAD` (`git diff HEAD`) are shown instead. When there is nothing to diff (the active cursor is on a directory, both panes point to the same file outside a repository, no differences), the reason is shown in the status line.
- `/`: Enter search mode. Type a query to jump to matching entries (case-insensitive substring). The whole list is searched from the cursor, wrapping around at the end; press Enter or Esc to exit search.
- `n` / `N`: Jump to the next / previous match of the last search (wrapping around).
- `R`: Reload both panes from disk (cursor and marks stay on the same files). Useful with `KURA_WATCH=off` or when a change was missed.
- `A`: List the files the coding agent (Claude Code) changed in the current pane, newest first (see [Files changed by the agent](#files-changed-by-the-agent)). In the list, `j` / `k` (with numeric prefixes), `gg` / `G` move, `Enter` goes to the file in its directory (closing the list), `e` opens it in your editor, `Y` copies it as an `@path` like `Y` in the file list, `R` re-reads the log, and `A`, `q` or `Esc` returns to the file list.
- `r`: Rename the selected file; an inline prompt appears at the bottom.
- `s`: Open a sort popup to choose ordering (modified/created/size/alphabetical). The chosen order is kept per pane, also when moving to another directory. The last row, `Directories first: on/off`, toggles listing directories before files (on by default).
- `x`: Move the selected entry or all marked entries to the OS trash, after confirmation (`y` confirms; `n` / `Esc` / `Enter` / `q` cancel). If the trash is not available (e.g. on WSL or some Linux setups), an error is shown in the status line and nothing is deleted.
  kura cannot list or restore items in the trash. On macOS, open the Trash in Finder and drag the item back (or use "Put Back" where it is shown).
- `X`: Permanently delete the selected entry or all marked entries from disk, after a "Confirm Permanent Deletion" dialog. This cannot be undone.
- `q`: Quit the application

## Color Map

- **Blue**: Directories
- **Red**: Hidden items (files or directories starting with `.`)
- **Green**: Executable files
- Others: Default color
- **Gray background, bold**: Changed in the last 10 seconds (by kura or any other program, e.g. a coding agent)
- **Underlined**: Changed since kura started

Inside a git repository, a status symbol is shown before each name: `M` modified, `A` added, `D` deleted (deleted files have no row; their directory gets `+`), `R` renamed, `U` conflicted, `?` untracked, `!` ignored, `+` directory containing changes. The current branch is shown in the status line. The status is read again whenever the listing is reloaded (moving, paste, delete, rename, or a change detected by file watching).

## Usage

```bash
cargo run
```

Video previews and HEIC/AVIF images need `ffmpeg` (see [ffmpeg (optional)](#ffmpeg-optional)).

The graphics protocol is detected automatically and shown in the preview footer. To force one, set `KURA_IMAGE_PROTOCOL` to `kitty`, `sixel`, `iterm2`, or `halfblocks`.

### Command-line arguments

```
kura [OPTIONS] [LEFT] [RIGHT]
```

- `LEFT` / `RIGHT`: Directories for the left / right pane. With one path, it is used for the left pane; omitted panes start in the current directory.
- `--last-dir-path <FILE>`: On exit, write the active pane's directory to `FILE` (see below).
- `-h`, `--help` / `-V`, `--version`

### cd on exit

A program cannot change its parent shell's directory, so use a wrapper function: it runs kura with `--last-dir-path` and then `cd`s into the directory that was active when you quit.

```bash
# ~/.bashrc or ~/.zshrc
k() {
    local tmp dir
    tmp="$(mktemp)" || return
    command kura --last-dir-path "$tmp" "$@"
    dir="$(cat -- "$tmp")"
    rm -f -- "$tmp"
    if [ -n "$dir" ] && [ -d "$dir" ] && [ "$dir" != "$PWD" ]; then
        cd -- "$dir" || return
    fi
}
```

```fish
# ~/.config/fish/functions/k.fish
function k
    set -l tmp (mktemp); or return
    command kura --last-dir-path $tmp $argv
    set -l dir (cat -- $tmp)
    rm -f -- $tmp
    if test -n "$dir"; and test -d "$dir"; and test "$dir" != "$PWD"
        cd $dir
    end
end
```

### File watching

The directories shown in the two panes are watched, and the listing updates by itself when files change (cursor and marks stay on the same files). Set `KURA_WATCH` to choose how:

- unset: OS file events
- `poll`: re-scan every second, for filesystems without events (e.g. WSL2 `/mnt/c`, network drives)
- `off`: no watching (press `R` to reload)

### Files changed by the agent

`A` lists the files a Claude Code session edited. kura cannot see this by itself: a `PostToolUse` hook appends the absolute path of each file the agent edits (`Edit`, `Write`, `NotebookEdit`) to a log, one path per line, and kura reads that log. Add the hook to `~/.claude/settings.json` (all projects) or to a project's `.claude/settings.json`. It needs [`jq`](https://jqlang.org/) (`brew install jq`, `apt install jq`; on Windows `winget install jqlang.jq`) and a POSIX shell:

```json
{
  "hooks": {
    "PostToolUse": [
      {
        "matcher": "Edit|Write|NotebookEdit",
        "hooks": [
          {
            "type": "command",
            "command": "f=\"${KURA_AGENT_LOG:-${XDG_STATE_HOME:-$HOME/.local/state}/kura/agent-changes.log}\"; mkdir -p \"$(dirname \"$f\")\" && jq -r '.tool_input.file_path // .tool_input.notebook_path // empty' >> \"$f\""
          }
        ]
      }
    ]
  }
}
```

- The log is `$XDG_STATE_HOME/kura/agent-changes.log`, by default `~/.local/state/kura/agent-changes.log`. It is one file for all projects, kept outside the project so that it does not show up as an untracked file. Set `KURA_AGENT_LOG` to use another file; the hook and kura must see the same value.
- kura lists only the files under the directory it was started in, newest first and each once. Only the last 1 MiB of the log is read; delete the file to start over.
- While the list is open, the log is watched (unless `KURA_WATCH=off`; press `R` then), so new edits show up by themselves. Files changed in the last 10 seconds and since kura started are highlighted like in the file list.
- Files that no longer exist stay in the list, dimmed and marked `(deleted)`; `Enter` on one goes to the directory it was in.
- Only edits made with the file tools are logged. Files changed by shell commands the agent runs (`mv`, `rm`, `sed -i`, formatters, …) are not.

## Herdr plugin

kura can be opened as a pane of [Herdr](https://herdr.dev/) (the TUI terminal workspace manager for coding agents), next to your agents, in the directory of the focused pane.

Install the `kura` binary first ([Installation](#installation)). The plugin does not build kura; it runs `kura` from the `PATH` of the Herdr server (if you installed with Cargo, make sure `~/.cargo/bin` is on it). Then install the plugin (Herdr 0.9.5 or later):

```bash
herdr plugin install hanafsky/Kura
```

It provides two actions:

| Action | Opens kura |
| --- | --- |
| `kura.open` | in a split to the right of the focused pane |
| `kura.open-popup` | in a popup (90% of the terminal) over the layout |

Bind them in `~/.config/herdr/config.toml`, for example to `prefix+f`:

```toml
[[keys.command]]
key = "prefix+f"
type = "plugin_action"
command = "kura.open"
description = "open kura"

[[keys.command]]
key = "prefix+shift+f"
type = "plugin_action"
command = "kura.open-popup"
description = "open kura (popup)"
```

You can also run them from the shell with `herdr plugin action invoke kura.open`. Quitting kura (`q`) closes its pane.

Notes on the popup:

- A popup receives every key, including `Esc` and the Herdr prefix, so you cannot switch panes while it is open. `Esc` does not close it; quit kura with `q`.
- A popup has no pane ID (no `HERDR_PANE_ID`) and is not part of the layout. Use the split when you want kura to stay open beside your agents.
- It does not open while another Herdr popup or modal (Settings, Copy mode) is open.

If nothing opens, check `herdr plugin log list --plugin kura`. To try a local checkout, use `herdr plugin link /path/to/Kura` instead of `install`.
