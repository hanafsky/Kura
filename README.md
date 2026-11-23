# kura

A simple terminal-based two-pane file manager written in Rust using ratatui and crossterm.

## Features

- Dual-pane directory listing
- Vim-like key bindings
- Image preview in the opposite pane (png, jpeg, gif, bmp, tiff, webp; heic/avif via ffmpeg), rendered with the Kitty / Sixel / iTerm2 graphics protocols when available
- Video preview: a thumbnail plus duration / codec / resolution info (requires `ffmpeg` and `ffprobe`)

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

Video previews and HEIC/AVIF images need `ffmpeg` (`brew install ffmpeg`).

The graphics protocol is detected automatically and shown in the preview footer. To force one, set `KURA_IMAGE_PROTOCOL` to `kitty`, `sixel`, `iterm2`, or `halfblocks`.