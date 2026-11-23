# kura

A simple terminal-based two-pane file manager written in Rust using ratatui and crossterm.

## Features

- Dual-pane directory listing
- Vim-like key bindings
- Image preview in the opposite pane (png, jpeg, gif, bmp, tiff, webp; heic/avif via ffmpeg), rendered with the Kitty / Sixel / iTerm2 graphics protocols when available
- Video preview: a thumbnail plus duration / codec / resolution info (requires `ffmpeg` and `ffprobe`)

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
- `G`: Go to the bottom of the file list or text viewer (equivalent to a large prefix then `j`).
- `V`: Enter visual multi-selection mode (like Vim's Visual Line mode). Use `j`/`k` to select a range of entries; press `Esc` or `V` again to exit visual mode, leaving marked entries.

- **Left pane:**
  - `h`: Go to the parent directory
  - `l`: Switch to the right pane

- **Right pane:**
  - `h`: Switch to the left pane
  - `l`: Go to the parent directory

- `Enter`: Enter the selected directory, open the text viewer for a text file, or show an image / video preview in the opposite pane. Press `Enter` (or `Esc` in preview) again to return to the file manager.
- `o`: Open the selected entry with the default application (`open` on macOS), e.g. play a video in QuickTime. Works in the file list and in preview.
  On WSL the path is converted with `wslpath -w` and opened by `explorer.exe`. Set `KURA_OPENER` (e.g. `KURA_OPENER=code`) to use another program as `<opener> <path>`.
- `v`: Toggle selection of the current entry in file manager mode. Selected entries are marked with a `*` symbol.
- `y`: Copy the current entry or all marked entries to the clipboard.
- `p`: Paste copied entries into the current directory.
- `/`: Enter search mode. Type a query to jump to matching entries; press Enter or Esc to exit search.
- `r`: Rename the selected file; an inline prompt appears at the bottom.
- `s`: Open a sort popup to choose ordering (modified/created/size/alphabetical).
- `x`: Delete the selected entry or all marked entries, prompting for confirmation.
- `X`: Delete the selected entry or all marked entries without confirmation.
- `q`: Quit the application

## Color Map

- **Blue**: Directories
- **Red**: Hidden items (files or directories starting with `.`)
- **Green**: Executable files
- Others: Default color

## Usage

```bash
cargo run
```

Video previews and HEIC/AVIF images need `ffmpeg` (see [ffmpeg (optional)](#ffmpeg-optional)).

The graphics protocol is detected automatically and shown in the preview footer. To force one, set `KURA_IMAGE_PROTOCOL` to `kitty`, `sixel`, `iterm2`, or `halfblocks`.