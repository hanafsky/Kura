# kura

A simple terminal-based two-pane file manager written in Rust using ratatui and crossterm.

## Features

- Dual-pane directory listing
- Vim-like key bindings
- Image viewer for common formats (png, jpeg, tiff, etc)

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

- `Enter`: Enter the selected directory, open text viewer if a text file is selected, open image viewer in the opposite pane if an image file is selected, or close viewer and return to file manager
- `v`: Toggle selection of the current entry in file manager mode. Selected entries are marked with a `*` symbol.
- `y`: Copy the current entry or all marked entries to the clipboard.
- `p`: Paste copied entries into the current directory.
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