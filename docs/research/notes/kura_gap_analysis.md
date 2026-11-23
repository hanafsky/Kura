# kura gap analysis: current state vs. what an ADE-era two-pane filer needs

Scope: (a) an inventory of kura v0.2.0's features, architecture and defects, taken from its source; (b) the features that matter for developers working with AI coding agents, with notes on how yazi, lf, nnn, broot and superfile implement them; (c) cross-platform gaps for Linux/WSL; (d) gaps in testing and CI.
Code citations use the form `file:line` and refer to the repo at commit d0cd342 (clean tree). Most code facts were read directly from source. Where a claim was checked by running something, the note says so.

## Q1. What does kura do today, and what are its concrete bugs and limitations?

### Takeaway
kura is a small (~1,580 lines of Rust in 10 files) synchronous, single-threaded two-pane browser with a good media-preview layer. Its file-operation layer is unsafe. The worst defect, which I verified by experiment: pasting a file into the directory it was yanked from truncates the file to 0 bytes. Both panes open on the same cwd at startup, so `y` then `p` destroys data in the most obvious first-use flow. The app also has no move/cut, no undo or trash, no config, hard-coded keys, and errors are written with `eprintln!` behind the alternate screen.

### Cited Findings

**Architecture and inventory**
- Module layout: `main.rs` (terminal setup), `app.rs` (Pane/App state), `input.rs` (one 330-line event loop and key dispatcher), `ui.rs` (rendering), `actions.rs` (mark, copy, paste, delete), `fs_utils.rs` (sort, search, recursive copy), `media.rs` (image/video preview, ffmpeg, open), `graphics.rs` (protocol detection), `input_source.rs` (macOS IME switch), `mode.rs` (Mode enum). Line counts: input.rs 343, ui.rs 298, media.rs 293, others under 155. — kura repo `wc -l src/*.rs`
- Dependencies are limited to crossterm 0.29, ratatui 0.30, ratatui-image 11.1, image 0.25, unicode-width 0.1, and libc on unix only (`Cargo.toml:6-14`). There is no async runtime, watcher, git, config or clipboard crate. Cargo.lock contains both unicode-width 0.1.14 (kura) and 0.2.3 (pulled in transitively), so the crate is duplicated (`Cargo.lock:2431-2438`).
- State model: `Pane { items: Vec<fs::DirEntry>, selected, current_dir, marked: HashSet<usize> }` (`app.rs:8-13`). `App` holds both panes, the active pane, the mode, an internal `clipboard: Vec<PathBuf>`, the picker and the preview (`app.rs:37-48`). Modes are Filer, Visual, Viewer, ConfirmDelete, Search, Rename, Sort and Preview (`mode.rs:9-39`).
- Both panes start in the process cwd (`app.rs:52-55`). There are no CLI arguments, flags (`--help`, `--version`) or path arguments (`main.rs:21-45`).
- Event loop: it calls `terminal.draw` every iteration and polls with a 100 ms timeout (`input.rs:17-18`), so the UI redraws about 10 times a second even when idle. Only `Event::Key` is handled (`input.rs:19`). Mouse capture is turned on (`main.rs:26`), but mouse events are ignored. That disables the terminal's native text selection and gives nothing in return.
- Keys are hard-coded in `match` arms (`input.rs:244-336`), and there is no config file. Implemented keys: `j/k` with a numeric count prefix (`input.rs:28-39, 245-246`); `gg`/`G` (`input.rs:42-76`); `h/l`, which swap meaning per pane (`input.rs:294-301`); `V` visual line selection (`input.rs:302-308, 79-119`); `v` mark (`input.rs:327-329`); `y` yank to the internal clipboard (`input.rs:330-332`); `p` paste/copy (`input.rs:333-335`); `/` search (`input.rs:309-313`); `r` rename (`input.rs:314-323`); `s` sort popup (`input.rs:324-326`); `x` delete with confirmation and `X` delete without (`input.rs:247-280`); `Enter` opens a dir, the viewer or the preview (`input.rs:281-288`); `o` opens with the OS (`input.rs:289-293`); `q` quits (`input.rs:22-24`).
- Media preview is the strongest part. It detects Kitty, Sixel, iTerm2 or halfblocks through ratatui-image, re-probes Kitty under multiplexers such as herdr that answer DSR early, and can be overridden with `KURA_IMAGE_PROTOCOL` (`graphics.rs:1-41`). Images are decoded natively with an ffmpeg fallback for HEIC/AVIF (`media.rs:114-124`) and downscaled to 2048 px (`media.rs:15, 73-79`). Video previews get ffprobe metadata and a thumbnail from about 10% into the file, at most 10 s in (`media.rs:243-273`).
- The macOS input-source switch is correctly gated with `cfg(target_os = "macos")` and is a no-op elsewhere (`input_source.rs:6, 95-105`). It runs before raw mode on every start (`main.rs:23`). The previous input source is not restored on exit, and the feature cannot be disabled.

**Data-loss and safety bugs (highest priority)**
- **Pasting into the source directory truncates files to 0 bytes.** `paste()` builds `dst = dst_dir.join(file_name)` and calls `fs::copy(src, &dst)` without checking whether `dst` exists or equals `src` (`actions.rs:40-49`). I verified this on this Mac (Darwin 27): `std::fs::copy("a.txt","a.txt")` returned `Ok(0)` and left `a.txt` at 0 bytes. Both panes start in the same cwd (`app.rs:54-55`), and yank and paste can happen in the same pane, so this is easy to hit. For a directory pasted into its own parent, `copy_dir_recursively(src, src)` copies every file onto itself and truncates the whole tree (`fs_utils.rs:73-85`). — verified by experiment in a scratchpad directory, not in the repo
- **Silent overwrite.** Pasting over an existing different file overwrites it, because `fs::copy` replaces the destination (`actions.rs:47`). Renaming onto an existing name also replaces it, since `fs::rename` replaces on Unix (`input.rs:194-198`). Neither path asks for confirmation or offers "keep both".
- **Recursive copy of a directory into its own subdirectory** never terminates: the copy creates `dst` inside `src` and then walks into it (`fs_utils.rs:73-85`). `path.is_dir()` follows symlinks (`fs_utils.rs:79`), so a symlink loop also recurses forever, and symlinks are copied as their targets' contents rather than as links.
- **`X` deletes permanently with no confirmation**, using `remove_dir_all` (`input.rs:264-280`, `actions.rs:56-70`). It is one Shift-key away from `x`. There is no trash and no undo.
- **In the confirm dialog, Enter means yes.** The prompt reads "Delete N item(s)? (y/N)" (`ui.rs:142`), which by convention means the default is No, but `KeyCode::Enter` runs the delete (`input.rs:219-223`). The dialog shows only a count, never the names of the files being deleted (`ui.rs:137-148`).
- **Marks are stored as indices.** `marked: HashSet<usize>` (`app.rs:12`) is cleared on every refresh (`app.rs:32`) and sort (`fs_utils.rs:52`). Any future auto-refresh or watcher would therefore silently mark different files. The `HashSet` iteration order also makes the order of copy and delete undefined (`actions.rs:22-26`).
- **`q` quits from every non-text mode** with no confirmation, including the ConfirmDelete dialog, the Sort popup, the Viewer and the Preview (`input.rs:21-24`).

**Silent errors and robustness**
- Errors are written with `eprintln!` while the TUI is in raw mode on the alternate screen (`actions.rs:45, 48, 61, 65`; `input.rs:197`). The message either corrupts the display or disappears when the screen is redrawn. There is no status or message line. Many errors are discarded entirely with `let _ =`: refresh (`app.rs:97, 136`; `actions.rs:52, 69`) and `open_external` (`input.rs:240, 291`).
- **A directory change commits before it is known to succeed.** `on_enter` and `on_left` assign `pane.current_dir = path` and then call `refresh()` while ignoring its error (`app.rs:95-97, 134-136`). Entering a directory without permission leaves the title showing the new path while the list still shows the old directory's entries. Any operation then acts on paths that don't match the title.
- **A single unreadable entry fails the whole listing.** `read_dir(...).collect::<Result<Vec<_>,_>>()?` (`app.rs:17, 28`).
- **No panic hook.** A panic, for example from `pane.items[pane.selected]` (`input.rs:194`) or the `unwrap` at `ui.rs:80`, leaves the terminal in raw mode on the alternate screen. Clean restore only happens when `run_app` returns normally (`main.rs:33-40`).
- **Refresh resets the cursor to 0 and clears marks** (`app.rs:31-32`). After a paste, delete or rename the cursor jumps to the top (rename re-finds the new name: `input.rs:199-207`). `h` to the parent does not put the cursor back on the directory just left (`app.rs:93-99`).
- **Sort does not persist.** `refresh()` always re-sorts by raw `file_name()`, which is a case-sensitive byte order with no directories-first grouping (`app.rs:29`). Choosing "Modified" in the `s` popup is lost on the next refresh. The sort comparators call `metadata()` inside the comparison, which costs O(n log n) syscalls (`fs_utils.rs:28-46`). Directions are inconsistent: Modified and Created sort ascending (oldest first) while Size sorts descending (`fs_utils.rs:37, 46`).
- **Hidden files are always shown** and only coloured red (`ui.rs:202-203`). There is no toggle and no .gitignore filtering.

**Viewer and search limitations**
- The viewer reads the whole file with `fs::read_to_string` (`app.rs:137`). Large files are loaded fully into memory, and Enter on a non-UTF-8 file that isn't media silently does nothing.
- The viewer re-wraps the entire file on every frame (`ui.rs:61-93`), and frames are drawn every 100 ms. The scroll offset is a `u16` (`mode.rs:18`, `input.rs:230-231`), so `G` on a file over 65,535 lines wraps around (`input.rs:67-68`). `G` uses the logical line count while the renderer scrolls by wrapped rows, so the two disagree on long-line files (`input.rs:67` vs `ui.rs:94-96`). There is no syntax highlighting, no search inside the viewer, and Esc does not close it (only Enter: `input.rs:232`).
- Search is incremental substring matching that starts after the current cursor (`fs_utils.rs:56-70`). Every keystroke can therefore jump past a match that was already selected. There is no `n`/`N`, no fuzzy matching and no recursive search.
- In Preview mode only Enter, Esc and `o` work (`input.rs:235-243`). You cannot `j`/`k` through images with the preview following the cursor.

**Performance**
- Everything is synchronous on the UI thread: `read_dir`, recursive copy and delete (`actions.rs`), image decode (`media.rs:114-124`), and blocking `ffprobe`/`ffmpeg` processes (`media.rs:89-105, 183-195`). A large copy or a slow video thumbnail freezes the UI.
- `draw_pane` calls `path.is_dir()` (a stat) and `e.metadata()` for every entry on every frame (`ui.rs:199-216`). Large directories therefore cost about 2N syscalls per 100 ms tick.

**Missing features compared with typical filers** (absent from the source): move/cut, mkdir/touch, copying a path to the system clipboard, opening in $EDITOR, shell or command execution, cd-on-exit, bookmarks, tabs, history, toggling the hidden filter, file size/mtime columns, a status bar showing count/size/permissions, git status, file watching, trash, undo, archives, bulk rename, config, keymap help.

**Linting and tests (verified by running them with `CARGO_TARGET_DIR` set to a scratch directory)**
- `cargo clippy --all-targets`: 1 warning, `unnecessary cast usize -> usize` at `ui.rs:96`.
- `cargo fmt --check`: fails, with diffs at `graphics.rs:21`, `input.rs:10` and `input.rs:279`.
- `cargo test`: 2 tests, both pure-function tests in `graphics.rs:135-151`. Nothing in `actions`, `fs_utils`, `app` or `input` is tested.

### Inferences
- The paste-onto-self truncation should block release. It should be fixed (refuse when `src == dst`, or auto-rename to `name (copy)`) before any new feature ships.
- `input.rs` mixes key parsing, mode logic and file effects in one function. Before adding keymaps, IPC or a watcher, the most useful refactor is a `Command` enum (input → Command → App::apply). The same commands could then be bound in config, sent over IPC/MCP, and unit-tested without a terminal.
- Storing marks and selection by path (`HashSet<PathBuf>`, cursor restored by name) is a prerequisite for auto-refresh. Without it every watcher event would destroy the user's selection.

### Gaps
- I did not test behaviour on Linux or WSL hardware. The claims about the Linux `fs::copy` self-truncation come from reading the code, not from a test (only macOS was tested).
- I did not measure performance on large directories. The syscall-per-frame cost is inferred from the code.

## Q2. Which features matter for agent-era (ADE) developer workflows?

### Takeaway
ADEs such as Herdr (a terminal multiplexer) and Orca (a desktop app) run several agents in parallel, often in separate git worktrees. They expose agent state and, in Herdr's case, a socket API. A filer "for ADEs" earns its place by doing five things the agent pane can't. (1) Show live what the agents are changing: a file watcher plus git status and diff. (2) Move between worktrees quickly. (3) Hand file references to the agent: copy `@path`, or write it into the agent's pane through Herdr's API. (4) Make edits safe: trash and undo. (5) Be drivable by agents themselves through a CLI or socket and, optionally, MCP.

### Cited Findings

**The ADE context**
- Herdr: "a terminal runtime for coding agents", written in Rust and Apache-2.0. It keeps terminals running in a background server, marks each pane as working, blocked or idle, and supports Claude Code, Codex, Cursor, OpenCode and others. It installs via curl, Homebrew or mise, and has a Windows beta. — [herdr GitHub](https://github.com/ogulcancelik/herdr)
- Herdr's socket API is newline-delimited JSON over a Unix socket (a named pipe on Windows). Methods include `pane.send_text`, `pane.send_keys`, `pane.send_input`, `pane.read`, `pane.list`, `pane.current`, `events.subscribe` and `events.wait`. CLI equivalents include `herdr pane run <pane> "<cmd>"`, `herdr pane read <pane> --source recent`, `herdr api snapshot` and `herdr api schema --json`. Panes receive the environment variables `HERDR_SOCKET_PATH`, `HERDR_ENV=1`, `HERDR_WORKSPACE_ID`, `HERDR_TAB_ID` and `HERDR_PANE_ID`. — [Herdr socket API docs](https://herdr.dev/docs/socket-api)
- Thoughtworks Radar lists Herdr as a terminal multiplexer and runtime for managing multiple coding-agent sessions. — [Thoughtworks Radar](https://www.thoughtworks.com/radar/tools/herdr)
- Orca (stablyai/orca, MIT) describes itself as an ADE for parallel agents. It runs each agent in an isolated git worktree, lets you "review diffs and leave notes on exact changed lines", and lets you "drag files from the explorer" into an agent's prompt. It opens images, PDFs and Markdown in the workspace, and runs on macOS, Windows and Linux plus SSH hosts. — [Orca GitHub](https://github.com/stablyai/orca); [Better Stack guide](https://betterstack.com/community/guides/ai/orca-ai/)
- Claude Code's `@` file references take relative or absolute paths, one file or several ("@file1.js and @file2.js"). Directory references expand to listings. Images can be passed as a plain path. Parallel sessions use `claude --worktree <name>`. "Claude reads files fresh on each tool call", so it picks up edits made in other applications. — [Claude Code common workflows](https://code.claude.com/docs/en/common-workflows)

**File watching / auto-refresh**
- lf has an opt-in `watch` option backed by fsnotify that refreshes file information automatically (FUSE is not supported), plus a `period` option for polling. — [lf doc.md](https://github.com/gokcehan/lf/blob/master/doc.md)
- The Rust standard is the `notify` crate (v9.0.0): FSEvents or kqueue on macOS, inotify on Linux, and a cross-platform PollWatcher fallback. It documents several limitations. **On WSL2, changes made by Windows apps under `/mnt/c` may not produce inotify events.** NFS mounts may emit nothing. inotify `max_user_watches` limits apply. Editors that replace files can break watches. Debouncing is handled by `notify-debouncer-mini` or `notify-debouncer-full`. — [docs.rs notify](https://docs.rs/notify/latest/notify/)

**Git status / diff / worktrees**
- yazi has no built-in git status. The official `git.yazi` plugin shows status as a linemode sign and is registered as a fetcher. Third-party plugins such as vcs-files.yazi list changed files. — [yazi-rs/plugins](https://github.com/yazi-rs/plugins); [yazi resources](https://yazi-rs.github.io/docs/next/resources)
- broot has git built in. `:gf` shows per-file status letters, the branch and ahead/behind counts. `:gs` limits the tree to files `git status` would show. `.gitignore` is respected, and the `alth`/`alti` keys toggle hidden and ignored files. — [broot](https://dystroy.org/broot/)
- Orca's core value is worktree isolation combined with diff review. — [Orca GitHub](https://github.com/stablyai/orca)

**Driving the filer from outside (IPC)**
- lf: `lf -remote 'send <id> <cmd>'` sends a command to one client or all clients. `$id` holds the client's id inside shell commands. `query` returns internal state. — [lf doc.md](https://github.com/gokcehan/lf/blob/master/doc.md)
- yazi DDS: a client-server design with no extra server process. `ya emit` sends a keymap action to the current instance and `ya emit-to` targets a specific one. `ya pub`/`ya pub-to` send messages. `--local-events`/`--remote-events` print events as one JSON line each on stdout (the docs mention Neovim integration as a use). `--client-id` names an instance. Built-in kinds include `cd`, `hover`, `rename`, `bulk-rename`, `@yank`, `move`, `trash` and `delete`. — [yazi DDS docs](https://yazi-rs.github.io/docs/dds)

**Shell integration / cd-on-exit / picker**
- lf has `-print-last-dir` and `-last-dir-path <path>`, plus lfcd wrapper scripts. — [lf doc.md](https://github.com/gokcehan/lf/blob/master/doc.md)
- nnn lists "cd on quit (easy shell integration)", a file-picker mode, plugins, FreeDesktop trash integration, contexts (tabs), bookmarks and a batch renamer. — [nnn GitHub](https://github.com/jarun/nnn)
- broot uses `alt-enter` to return to the shell in the selected directory, and `:e` opens the editor. — [broot](https://dystroy.org/broot/)
- superfile ships a `cd_on_quit` folder and customizable hotkeys with a vim preset. Its Windows support is "not fully supported yet". — [superfile GitHub](https://github.com/yorukot/superfile)

**Safe operations**
- lf binds no delete key by default "to protect new users", and its docs give a trash example. — [lf doc.md](https://github.com/gokcehan/lf/blob/master/doc.md)
- yazi has a built-in trash bin plus task progress and cancellation. — [yazi GitHub](https://github.com/sxyazi/yazi)
- The `trash` crate (v5.4.0) moves files to the OS trash on macOS, Windows and Linux. Linux defaults to the freedesktop Trash spec 1.0, with an optional XDG portal backend that can't trash directories or symlinks. Listing and restoring (`os_limited`) are available only on Windows and freedesktop Linux, **not macOS**. WSL isn't mentioned. — [docs.rs trash](https://docs.rs/trash/latest/trash/)

**Performance, fuzzy finding, preview**
- yazi does all I/O asynchronously with multithreaded CPU work, task priorities and cancellation, preloading, built-in code highlighting, scrollable previews (video, PDF, archive, code, directory), tabs, bulk rename, archive extraction, fzf/zoxide/ripgrep/fd integration and a plugin package manager. — [yazi GitHub](https://github.com/sxyazi/yazi)
- broot searches without blocking: each keystroke interrupts the current search. Sizes, dates and counts are computed in the background. — [broot](https://dystroy.org/broot/)
- nucleo, the fuzzy matcher behind Helix, uses fzf-style scoring and claims to be about 6× faster than skim and the fuzzy-matcher crate. — [nucleo GitHub](https://github.com/helix-editor/nucleo)
- lf's previewer is an external script that receives the path, width, height, x, y and mode, with optional preloading and caching. — [lf doc.md](https://github.com/gokcehan/lf/blob/master/doc.md)

**Clipboard**
- OSC 52 is widely supported. tmux's `set-clipboard` defaults to `external`, which blocks applications inside tmux from setting the clipboard; it has to be set to `on` for an app in a pane to reach the outer terminal. Windows Terminal's OSC 52 tracking issue was closed as fixed. — [tmux wiki Clipboard](https://github.com/tmux/tmux/wiki/Clipboard); [microsoft/terminal#2946](https://github.com/microsoft/terminal/issues/2946)

### Inferences
- A suggested priority for the report writer, from the evidence above:
  - **P0 (safety, prerequisites):** fix paste onto self and overwrites; add a confirm/collision dialog; use trash by default with `X` behind a config option; a status/message line in place of eprintln; a panic hook; path-based marks and cursor; refresh that keeps the cursor; a `Command` abstraction; CI with fmt, clippy and tests.
  - **P1 (the ADE core):** notify-based auto-refresh with debouncing and highlighting of recently changed files (mtime within N seconds, "changed since session start"); per-file git status from `git status --porcelain=v2` via subprocess, or gix; a worktree list and switcher (`git worktree list --porcelain`); a diff preview for the selected file in the opposite pane (`git diff` output, possibly piped through delta if present); "copy as @path" in relative or absolute form via OSC 52 plus native fallbacks (pbcopy, wl-copy/xclip, clip.exe); sending a path to an agent pane with `herdr pane run`/`pane.send_text` when `HERDR_ENV=1`; opening in `$EDITOR`, or `nvim --server $NVIM --remote` when inside nvim; cd-on-exit (`--print-last-dir` / `--last-dir-path`).
  - **P2:** a `kura` CLI/socket (an lf `-remote` / yazi `ya emit` analogue) so agents can tell kura to "reveal this file"; an MCP server exposing reveal, list_changed and selection. MCP is the agent-native analogue of yazi's DDS. Exposing "what the human selected" to an agent is a differentiator I found in no surveyed filer. Also: a config file and keymap (TOML), toggling .gitignore and hidden files (`ignore` crate), fuzzy jump with nucleo, zoxide integration, bookmarks, syntax-highlighted preview (syntect), Markdown preview, async listing and previews on a worker thread.
  - **P3:** tabs, bulk rename via $EDITOR, archives, mouse support, a plugin system.
- Two-pane layouts suit worktree comparison: left pane on the main worktree, right pane on an agent's worktree, with a diff between the files under the two cursors. None of the surveyed tools offers this as a single action. yazi and lf are single-pane Miller-column designs; broot has panels. That is plausible positioning for kura.
- kura already handles the herdr Kitty-graphics quirk (`graphics.rs:3-8`), so the author already uses Herdr. Integrating with Herdr's socket API is a natural next step.

### Gaps
- I did not fetch the Herdr plugin-marketplace docs. Whether Herdr offers a native "file-manager pane" or file-reference hook is unknown.
- I found no public example of a TUI file manager exposing an MCP server. I did not search exhaustively, so this is a market-gap claim with limited confidence.
- I did not verify superfile's feature list beyond its README landing page; that page did not cover preview, trash or zoxide.
- I did not fetch docs.rs pages for gix, git2, ignore or syntect. Crate recommendations for those (gix status maturity compared with shelling out to `git`) are unverified here.

## Q3. Cross-platform gaps for Linux and WSL

### Takeaway
The code mostly gates platform-specific behaviour correctly. The two real gaps are opening files (`xdg-open` doesn't reach Windows apps from WSL) and the release matrix, which ships no Apple Silicon or Linux ARM binaries. Image previews should work in Windows Terminal through Sixel. File watching has a documented blind spot on `/mnt/c` under WSL2.

### Cited Findings
- The input-source switch is cfg-gated, with a no-op elsewhere (`input_source.rs:6, 95-105`). Kitty re-probing uses libc `poll`/`read` and is `#[cfg(unix)]`, falling back to `false` on other platforms (`graphics.rs:62-118`). Executable detection is cfg-gated for unix and windows (`ui.rs:205-217`). — kura source
- `open_external` uses `open` on macOS and `xdg-open` on everything else (`media.rs:276-280`). There is no WSL detection, and failures are ignored (`input.rs:291`). — kura source
- wslu's `wslview` opens WSL files and folders in Windows apps. A Launchpad bug reports that it fails for PNG and JPEG files on Windows 11 / Ubuntu 22.04. Alternatives include `cmd.exe /C start` and `explorer.exe`. — [wslview man page](https://manpages.ubuntu.com/manpages/noble/man1/wslview.1.html); [Launchpad bug 2069513](https://bugs.launchpad.net/ubuntu/+source/wslu/+bug/2069513); [DEV article](https://dev.to/anrodriguez/open-images-or-any-file-from-wsl2-directly-in-windows-5bf2)
- Windows Terminal supports Sixel from 1.22: Preview on 2024-08-27, stable on 2025-02-05. yazi lists Windows Terminal ≥ v1.22.10352.0 as supporting image preview. — [WT Preview 1.22 blog](https://devblogs.microsoft.com/commandline/windows-terminal-preview-1-22-release); [WT 1.23 Preview / 1.22 stable blog](https://devblogs.microsoft.com/commandline/windows-terminal-preview-1-23-release/); [yazi GitHub](https://github.com/sxyazi/yazi)
- WSL2: notify may miss changes made by Windows apps under `/mnt/c`. — [docs.rs notify](https://docs.rs/notify/latest/notify/)
- The release workflow builds `x86_64-unknown-linux-gnu`, `x86_64-apple-darwin` and `x86_64-pc-windows-msvc` only (`.github/workflows/release.yml:15-21`). There is no `aarch64-apple-darwin` build, even though `macos-latest` runners are arm64 and the macOS target is cross-built for Intel. There is also no aarch64 Linux or musl static build. — kura source
- The ffmpeg hint says `brew install ffmpeg` (`media.rs:253`; `README.md:52`), which is macOS-specific wording on Linux and WSL. — kura source
- The internal clipboard (`app.rs:42`) never reaches the OS clipboard. Under tmux, OSC 52 needs `set-clipboard on`. — [tmux wiki Clipboard](https://github.com/tmux/tmux/wiki/Clipboard)

### Inferences
- The opener should be resolved in this order: an env or config override, then on macOS `open`, then under WSL (detected from `WSL_DISTRO_NAME` or `/proc/sys/kernel/osrelease` containing "microsoft") `wslview` if present, otherwise `explorer.exe` with a `wslpath -w` translated path, then on Linux `xdg-open`.
- The `trash` crate's freedesktop backend should work inside WSL's Linux filesystem, and items go to `~/.local/share/Trash` rather than the Windows Recycle Bin. That is acceptable, but it should be documented.
- The README says "requires ffmpeg" and the Windows build exists, but the user's stated target is macOS and Linux/WSL. The Windows build could be kept as best-effort or dropped.

### Gaps
- I did not confirm whether wslu is still maintained in 2026. The search found no deprecation notice for `wslview` itself.
- Sixel performance in Windows Terminal through WSL, and how ratatui-image's detection behaves there, was not tested.
- Clipboard fallbacks on Wayland and X11 (wl-copy, xclip) are not covered by fetched sources. They are standard practice but uncited here.

## Q4. Testing and CI gaps

### Takeaway
The only CI is a tag-triggered release workflow built on archived actions. Nothing runs fmt, clippy or tests on pushes or PRs, and only two trivial tests exist.

### Cited Findings
- `release.yml` triggers only on `v*` tags and `workflow_dispatch` (`.github/workflows/release.yml:3-7`). It uses `actions/checkout@v3` (`:25`), `actions-rs/toolchain@v1` (`:28`), and `actions/create-release@v1` plus `actions/upload-release-asset@v1` (`:78, :88`). The actions-rs and create/upload-release actions are unmaintained (common knowledge; not re-verified via a fetched source in this session). The build is not cached, does not run `cargo test`, and publishes no checksums. — kura source
- Test status: 2 unit tests, both passing (`graphics.rs:135-151`). `cargo fmt --check` fails and clippy reports 1 warning. — verified locally (see Q1)

### Inferences
- Minimal CI: a `ci.yml` on push and PR that runs `cargo fmt --check`, `cargo clippy -- -D warnings` and `cargo test` on ubuntu-latest and macos-latest. Release should move to `dtolnay/rust-toolchain` plus `softprops/action-gh-release` or cargo-dist, with aarch64-apple-darwin, x86_64/aarch64 linux-musl and checksums. Homebrew tap and `cargo install` instructions would help adoption.
- Testability depends on the Command refactor and on making file operations pure functions (`plan_paste(src, dst) -> Vec<Op>`). Tests should be written with `tempfile` for paste onto self, overwrites, recursive copy into a child, and symlink loops. UI snapshot tests can use ratatui's `TestBackend`.

### Gaps
- I did not fetch the official deprecation notices for actions-rs/toolchain or actions/create-release in this session. The report writer should mark that claim as background knowledge.
