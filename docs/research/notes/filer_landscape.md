# Dual-pane and TUI file manager landscape, 2024 to late 2026 (for kura's positioning)

Research date: 2026-11-23. Star counts and release dates come from the GitHub REST API (`gh api repos/<owner>/<repo>` and `/releases/latest`), queried 2026-11-23. Items marked "(background knowledge, not re-verified)" come from prior knowledge and were not checked against a source in this session.

## Q1. Paradigm, language, activity and 2024–2026 features of each file manager

### Takeaway
The terminal space is dominated by Miller-column and single-list managers: yazi (43.6k stars, released 2026-09-01), superfile (24.1k), nnn (22.1k), ranger (17.5k), broot (13.2k) and lf (9.6k). Among orthodox two-pane TUIs, the established options are mc and far2l (C/C++), which are mature but slow-moving. In 2026 a wave of small, new two-pane TUIs appeared. One of them, **norte** (Rust, created 2026-07), already brands itself "the open-source file commander for the agent era" and ships an MCP server. This competes directly with kura's planned positioning, although it is tiny so far (7 stars).

### Cited Findings

**Summary table (GitHub API, 2026-11-23)**

| Project | Paradigm | Lang | Stars | Latest release | Last push |
|---|---|---|---|---|---|
| yazi (sxyazi/yazi) | Miller columns (parent/current/preview) + tabs | Rust | 43,599 | v26.9.1, 2026-09-01 | 2026-11-22 |
| superfile (yorukot/superfile) | Multi-panel (several file panels side by side) + sidebar + preview | Go | 24,118 | v1.6.1, 2026-11-20 | 2026-11-21 |
| nnn (jarun/nnn) | Single list + contexts (4 tabs) | C | 22,141 | v5.3, 2026-08-15 | 2026-11-22 |
| ranger | Miller columns | Python | 17,467 | tags only, latest v1.9.4 (date not checked) | 2026-09-09 |
| broot (Canop/broot) | Tree + panels (up to 2 panels + preview) | Rust | 13,188 | v1.62.0, 2026-11-01 | 2026-11-12 |
| lf (gokcehan/lf) | Miller columns | Go | 9,601 | r42, 2026-07-31 | 2026-11-19 |
| xplr (sayanarijit/xplr) | Single list, programmable layout | Rust | 4,853 | v1.2.2, 2026-10-25 | 2026-11-17 |
| Double Commander | Orthodox two-pane (GUI) | Pascal | 4,836 | v1.2.11, 2026-11-08 | 2026-11-22 |
| joshuto (kamiyaa/joshuto) | Miller columns (ranger-like) + tabs | Rust | 3,744 | tags only, latest v0.9.9 (date unknown) | last commit 2026-10-26 |
| Far Manager (FarGroup) | Orthodox two-pane (Windows console) | C++ | 2,267 | CI build v3.0.6756.5045, 2026-11-21 | 2026-11-21 |
| far2l (elfmz/far2l) | Orthodox two-pane (Linux/macOS port of Far) | C++ | 2,244 | v_2.9.1, 2026-10-07 | 2026-10-27 |
| Midnight Commander (MidnightCommander/mc) | Orthodox two-pane | C | 1,057 (GitHub repo only created 2025-02-27) | 4.8.33, tag dated 2025-01-23; 4.8.34 NEWS page exists but release not confirmed | 2026-11-22 |

Sources: [yazi](https://github.com/sxyazi/yazi), [superfile](https://github.com/yorukot/superfile), [nnn](https://github.com/jarun/nnn), [ranger](https://github.com/ranger/ranger), [broot](https://github.com/Canop/broot), [lf](https://github.com/gokcehan/lf), [xplr](https://github.com/sayanarijit/xplr), [doublecmd](https://github.com/doublecmd/doublecmd), [joshuto](https://github.com/kamiyaa/joshuto), [FarManager](https://github.com/FarGroup/FarManager), [far2l](https://github.com/elfmz/far2l), [mc](https://github.com/MidnightCommander/mc)

**yazi**
- Release cadence is calendar-versioned and frequent: v25.5.31, v25.12.29, v26.1.4, v26.1.22, v26.5.6, v26.8.15 and v26.9.1 — [releases](https://github.com/sxyazi/yazi/releases)
- Added in the 2026 releases (v26.x): drag and drop, a trash bin, bulk create, a help menu that works as a command palette, input history, automatic dark/light theme switching, a custom VFS provider (`archive://` is no longer built in and is now registered by plugins), H/M/L motions, a dynamic keymap Lua API, Kitty graphics over shared memory, image preview inside Zellij via the Kitty protocol, custom tab names, mouse drag-resizing of panes, a multi-file spotter, the `ya.exec()` API and `ya exec` subcommand ("execute an action and await its result"), and an SFTP VFS with certificate authentication — [CHANGELOG](https://github.com/sxyazi/yazi/blob/main/CHANGELOG.md)
- Added in the 2025 releases: remote file management, a virtual file system, per-task progress in the task manager, a `bulk_rename` action that always uses the editor, `key-*` DDS events for intercepting keys, redesigned tabs, video cover preview, the `ya pkg` package manager, the `ya.emit()` API, enhanced fzf integration, and "Use Yazi in Helix directly without Zellij or tmux" — [CHANGELOG](https://github.com/sxyazi/yazi/blob/main/CHANGELOG.md)

**superfile**
- v1.6.1 (2026-11-20) added native trash on Linux, macOS and Windows, multi-file `--chooser-file` picking, directory item counts, an updated cd-on-quit shell function, and Rosé Pine themes. v1.6.0 shipped 2026-06-07 and v1.5.0 on 2026-01-13 — [release v1.6.1](https://github.com/yorukot/superfile/releases/tag/v1.6.1)
- Its UI has several side-by-side file panels plus a sidebar and preview, so it is the closest popular modern TUI to a multi-pane commander (background knowledge, not re-verified).

**nnn**
- v5.3 (2026-08-15) added a native preview pane (`P`), better fuzzy-filter scoring and ranking, lazy-loaded plugins, a `stats` plugin, a `tnp` plugin that opens files in a tmux Neovim pane, and Kitty drag-and-drop support — [release v5.3](https://github.com/jarun/nnn/releases/tag/v5.3)

**lf**
- r42 (2026-07-31): the client/server connection on Windows now uses Unix domain sockets exclusively, the server socket fallback moved to `/tmp/lf-<uid>/lf.sock`, and the ruler is now a file (`rulerfile`) — [release r42](https://github.com/gokcehan/lf/releases/tag/r42)

**broot**
- v1.62.0 (2026-11-01): the client/server features (`--listen`, `--send`, `--get-root`) now work on Windows, and verb configuration validation and escaping were reworked. v1.60.0 (2026-09-05) added preview wrapping. v1.60.1 made git statuses and diff previews more reliable — [CHANGELOG](https://github.com/Canop/broot/blob/main/CHANGELOG.md)

**Midnight Commander**
- The last confirmed stable release is 4.8.33 (23 January 2025). A NEWS-4.8.34 wiki page lists removal of the PCRE/PCRE2 search engines, a switch to clang-format, SquashFS in extfs, and fixes for a mouse-drag segfault and a `vfs_parse_ls_lga` buffer overflow. Whether 4.8.34 has actually shipped is unconfirmed — [NEWS-4.8.34](https://midnight-commander.org/wiki/NEWS-4.8.34?version=18); [Wikipedia](https://en.wikipedia.org/wiki/Midnight_Commander)

**far2l / Far Manager**
- far2l v_2.9.1 was released 2026-10-07. Far Manager 3 publishes continuous CI builds, the latest on 2026-11-21 — GitHub API ([far2l](https://github.com/elfmz/far2l/releases), [FarManager](https://github.com/FarGroup/FarManager/releases)). I did not extract far2l's 2025–2026 feature list (gap).

**GUI dual-pane managers (macOS / desktop)**
- **Marta** (marta.sh, Yan Zhulanow) is a native macOS "two-panel file commander". Its newest changelog entry is 0.8.2 (25 Jul 2024). A "back on track" blog post says development resumed after a pause and that the app will become paid after beta — [changelog](https://marta.sh/changelog/); [back on track](https://marta.sh/blog/back-on-track/). The date of that post was not confirmed. Activity looks low.
- **ForkLift** (BinaryNights) is a proprietary, macOS-only dual-pane manager with tabs, folder sync, SFTP/WebDAV/S3 and batch rename. Wikipedia lists 4.4.5 (13 Jan 2026) as stable, while download sites list 4.7.7, so the version sources disagree — [Wikipedia](https://en.wikipedia.org/wiki/ForkLift_(file_manager)); [TechSpot](https://www.techspot.com/downloads/6698-forklift.html); [BinaryNights downloads](https://binarynights.com/downloads)
- **Double Commander** is a cross-platform GUI two-pane manager (Total Commander-like) and is actively released: v1.2.11 on 2026-11-08 — [GitHub](https://github.com/doublecmd/doublecmd)

**New two-pane / commander-style projects, 2025–2026 (all small)**
- **norte** (compilando/norte; Rust; created 2026-07-08; 7 stars; v0.3.0-alpha.10). README: "a two-pane, orthodox file manager built for how people work now: a person at the keyboard, and an AI agent that also needs to touch files." It has a headless core with TUI (`ntc`), GUI and CLI clients over a versioned protocol. `norte mcp serve` lets "Claude Code, Codex or any MCP client work on your files — inside a scope you grant", and every agent change is journaled in a hash-chained journal. It also has seven keymap presets (including `vim`), sandboxed WASM plugins, SFTP/FTP/S3 and archives as directories. Platforms: Linux, with a Windows preview — [README](https://github.com/compilando/norte); [site](https://getnorte.dev)
- **che** (aroum/che; Rust; created 2026-07-25; 41 stars; v26.11.21 released 2026-11-21) is an "async I/O" terminal manager with "true dual panel mode" inspired by TC/DC/MC — [GitHub](https://github.com/aroum/che)
- **pelorus** (mogglemoss/pelorus; Go/Charm; created 2026-04; 5 stars) is a dual-pane TUI with vim keys. It has a background job queue with progress and ETA, fuzzy filter, a frecency jump list, `$EDITOR` bulk rename, git status glyphs, a Chroma/Glamour preview, and SFTP and Tailscale panes. Last pushed 2026-04-15, so it may be dormant — [GitHub](https://github.com/mogglemoss/pelorus)
- **Directory Navigator** (deemoun; Rust + Turbo Vision; created 2026-10-20; 14 stars; on crates.io) is a DOS Navigator-style two-panel manager with archives, trash, directory compare and mouse support — [GitHub](https://github.com/deemoun/Directory-Navigator)
- **rcmd** (jaroslavpachola/rcmd; Rust ratatui/egui; created 2026-07; 2 stars) is an mc replacement with a viewer/editor and archive and FTP/SFTP panels — [GitHub](https://github.com/jaroslavpachola/rcmd)
- **XeFM** (crftwr/xefm; Python; created 2025-09; 15 stars) is a dual-pane manager with both a desktop GUI and a TUI. It is extensible in Python and offers "ask an AI coding agent to write [an extension] with the XeFM skill" — [GitHub](https://github.com/crftwr/xefm)
- **termide** (Rust; created 2025-11; 189 stars) is an all-in-one TUI workspace with an editor, a dual-pane file manager with SFTP/FTP, a terminal, git, and a built-in coding agent with MCP servers — [GitHub](https://github.com/termide/termide)
- Others: rust-commander (crates.io, mc-inspired) — [crates.io](https://crates.io/crates/rust-commander); FileManagerTUI (tree+preview "dual pane", vim keys) — [GitHub](https://github.com/NguyenSiTrung/FileManagerTUI); MTN2 (Delphi, Far/DN-style, Windows/Linux) — [GitHub](https://github.com/Laex/MTN2)

**Identifying the "martha" and "folklyft" the user mentioned**
- "martha" is almost certainly **Marta** (marta.sh), a macOS "two-panel file commander" by Yan Zhulanow. The spelling is one letter off, and the paradigm matches kura's two-pane framing — [Introducing Marta](https://marta.sh/blog/introducing-marta); [WeAreDevelopers showcase](https://www.wearedevelopers.com/magazine/760-tool-showcase-marta-sh-a-fast-native-fully-customisable-and-extensible-two-panel-file-commander)
- "folklyft" is most likely **ForkLift** (BinaryNights), the macOS dual-pane manager. A web search for "folklyft" or "folklift" found nothing exact, and the closest result was ForkLift — [Wikipedia](https://en.wikipedia.org/wiki/ForkLift_(file_manager)). A GitHub repository search for `folklift`, `folklyft` and `folk lyft` returned no file-manager repositories (gh search, 2026-11-23). In Japanese, ForkLift is written フォークリフト, which plausibly produced the romanized misspelling. Marta and ForkLift are also the two best-known macOS two-pane managers and are often mentioned together.

### Inferences
- The popularity gap is large. Miller-column and list managers (yazi, superfile, nnn) have tens of thousands of stars, while every new orthodox two-pane TUI from 2026 has under 200.
- kura (Rust + ratatui + Vim keys + two-pane) shares its stack and paradigm with che, rcmd, Directory Navigator and especially norte. "Rust two-pane TUI" is not unique in itself.

### Gaps
- Release dates for ranger v1.9.4 and joshuto v0.9.9 were not checked (both projects push commits but use tags only).
- far2l's 2025–2026 feature list and Far Manager's recent changes were not extracted.
- Whether mc 4.8.34 has shipped is unconfirmed.
- The ForkLift version conflict (4.4.5 vs 4.7.7) is unresolved.

## Q2. Is yazi "a tree/Miller-column viewer, not a two-pane filer"?

### Takeaway
Broadly true, though "tree" is inaccurate: yazi is a Miller-column manager (parent/current/preview, after ranger). It has no native orthodox two-pane mode. It has built-in tabs with cross-tab copy/paste, plugins that toggle or maximize panes, and adjustable layout ratios, which approximate but do not reproduce a commander's always-visible source/destination pair.

### Cited Findings
- yazi uses a three-pane layout (parent, current, preview) inspired by ranger — [tecmint](https://www.tecmint.com/yazi-terminal-file-manager-linux/); [itsfoss](https://itsfoss.com/yazi.md)
- A GitHub discussion titled "Migrating from vifm, 2 pane workflow" (#429) suggests that yazi's multi-tabs give similar functionality to two-pane workflows (copy/move/paste across tabs) — [discussion #429](https://www.github.com/sxyazi/yazi/discussions/429)
- Tabs are first-class: redesigned tabs in 2025, custom tab names in 2026, and `tab_swap` cycling — [CHANGELOG](https://github.com/sxyazi/yazi/blob/main/CHANGELOG.md)
- Official plugins include `toggle-pane.yazi` (show, hide or maximize the parent/current/preview panes). Community plugins include `projects.yazi` (save and restore all tabs) and `close-and-restore-tab.yazi` — [yazi-rs/plugins](https://github.com/yazi-rs/plugins); [resources](https://yazi-rs.github.io/docs/next/resources)
- The layout ratio can be changed dynamically via `rt.mgr.ratio` (2025) — [CHANGELOG](https://github.com/sxyazi/yazi/blob/main/CHANGELOG.md)
- Comparison sites list yazi's dual-pane view as "Not verified" — [itechguides yazi vs far2l](https://www.itechguides.com/compare/yazi-vs-far2l/)

### Inferences
- kura's claim holds if "two-pane" means two simultaneously visible, independent directory listings with F5/F6-style copy and move between them. Say "Miller-column" rather than "tree" to stay accurate.
- yazi's Lua layout API could in principle be used to build a pseudo two-pane plugin. I found no well-known plugin that does it, and that absence is itself a signal.

### Gaps
- I did not exhaustively search yazi's plugin ecosystem for a "dual-pane" plugin. None appeared in the searches.

## Q3. Which file managers target AI agents / ADE workflows (MCP, IPC, path-sending)?

### Takeaway
Agent integration is happening mostly as community glue around yazi, using its IPC and plugins, and inside the ADEs themselves. Only one file manager, norte, makes "two-pane + agents (MCP)" its core identity. Herdr's maintainer explicitly declined to build a file panel and points users at external TUIs (yazi, lf, ranger). The awesome-herdr catalog lists 9 file-manager launchers, all for yazi, nnn, Fresh or simple sidebars, and none is a two-pane commander. That leaves a visible empty slot for a two-pane filer inside ADEs, but norte is aiming at the agent-era two-pane identity from the MCP side.

### Cited Findings
**ADEs**
- Herdr issue #131, "Add an independent files panel to the sidebar", opened 2026-05-17, was closed as not planned the same day. Maintainer: "i don't want to rush into making herdr feel like a tui ide or file manager" and "closing this for now since i don't want to accept a built-in files sidebar in the current product shape." The maintainer suggested custom keybindings that launch yazi, lf, ranger, nvim or fzf from the pane's cwd — [herdr#131](https://github.com/herdrdev/herdr/issues/131)
- awesome-herdr, "Yazi, nnn and other terminal file managers" (9 projects): herdr-yazi (35★), herdr-fresh, herdr-flist (a directory sidebar with git markers), herdr-yazi-windows, scp-explorer, herdr-yazi-links, herdr-vista, herdr-yazi-explorer, herdr-nnn (0★). None is two-pane — [catalog](https://github.com/yigitkonur/awesome-herdr/blob/main/catalog/06-worktrees-and-terminal.md)
- awesome-herdr also lists 9 "path hints" tools that open files agents mention (e.g. termscope 71★, herdr-peek, herdr-lens, herdr-shelf, which lists files a Claude Code session sent), plus categories for in-app file explorers (13) and Markdown/file previewers (14). This shows demand for "see and act on what the agent touched" — [catalog](https://github.com/yigitkonur/awesome-herdr/blob/main/catalog/06-worktrees-and-terminal.md); [awesome-herdr](https://github.com/yigitkonur/awesome-herdr)
- Orca (stablyai/orca), an MIT desktop ADE, has a built-in file explorer on the right, renders Markdown, lets you drag files or images into an agent's prompt, and gives each agent its own git worktree — [Better Stack guide](https://betterstack.com/community/guides/ai/orca-ai/); [README](https://github.com/stablyai/orca)

**Agent-oriented integrations by file manager**
- yazi plugins: `herdr-yazi-agent` (Ctrl+L puts the path into the agent prompt in a herdr pane, without pressing Enter) — [GitHub](https://github.com/timofey-TK/herdr-yazi-agent); `agent-tui.yazi` (launches Claude Code or Codex with the selected files, passing the path list via the clipboard; needs Yazi ≥ 26.8) — [GitHub](https://github.com/tr1v3r/agent-tui.yazi); `yazi-claude-ide` (reports the hovered file to Claude Code via its undocumented `/ide` protocol) — [GitHub](https://github.com/FunnyQ/yazi-claude-ide); `yazi-herdr-annotate` (comment on a file and send the review to the agent) — [GitHub](https://github.com/evilsquid888/yazi-herdr-annotate)
- yazi IPC primitives: DDS pub/sub, `ya emit` / `ya pub` with receiver checks, `ya.emit()`, and the 2026 `ya exec` subcommand that runs an action and awaits its result — [CHANGELOG](https://github.com/sxyazi/yazi/blob/main/CHANGELOG.md)
- norte: `norte mcp serve` for Claude Code, Codex or any MCP client, with scoped grants, user-written policy rules, and a per-agent-session journal. The README says about four in five commits are co-authored by Claude — [README](https://github.com/compilando/norte)
- termide bundles a coding agent and MCP servers with a dual-pane file manager in one TUI — [GitHub](https://github.com/termide/termide)
- XeFM ships an "agent skill" so AI coding agents can write extensions — [GitHub](https://github.com/crftwr/xefm)
- lf has a client/server socket (`lf -remote`) for external control, hardened in r42 — [release r42](https://github.com/gokcehan/lf/releases/tag/r42)
- broot has client/server control (`--listen`, `--send`, `--get-root`, now on Windows too) and user-defined verbs — [CHANGELOG](https://github.com/Canop/broot/blob/main/CHANGELOG.md)
- nnn's `tnp` plugin opens files in a tmux Neovim pane, which is multiplexer-pane awareness but not agent-specific — [release v5.3](https://github.com/jarun/nnn/releases/tag/v5.3)
- superfile's `--chooser-file` multi-select writes the chosen paths to a file (picker mode) — [release v1.6.1](https://github.com/yorukot/superfile/releases/tag/v1.6.1)
- xplr exposes a message-pipe API for external control and Lua config (background knowledge, not re-verified)
- A generic search found many filesystem MCP servers (e.g. Anthropic's reference Filesystem MCP), but none built on yazi or superfile — [contextbolt](https://contextbolt.com/blog/filesystem-mcp-server/)

### Inferences
- "Two-pane filer that lives inside ADEs" is not yet occupied in the Herdr ecosystem: every launcher plugin targets yazi or nnn. Still, the slot is being filled with yazi plugins, so kura needs an equally low-friction herdr/Orca plugin and agent hook to compete.
- norte is the closest conceptual competitor, but its angle is "governed MCP access to files". kura's angle could be "the human's two-pane control surface beside agents": cwd-following, a send-to-agent prompt, and reviewing changed files and worktrees in the other pane.
- Agent-side demand clusters on three things: sending paths into an agent's prompt, previewing files an agent mentions or changed, and following the agent pane's cwd or worktree.

### Gaps
- Orca's plugin/extension model and whether it can host a TUI file manager pane were not examined.
- No usage numbers (installs) exist for the herdr plugins beyond stars.

## Q4. Common features in modern TUI file managers that kura may lack

### Takeaway
The 2025–2026 baseline is: async operations with a progress/task view, trash, `$EDITOR` bulk rename, fuzzy and zoxide jump, image and syntax-highlighted preview, archives and SFTP as virtual directories, git status, mouse support (including drag and drop), themes with auto dark/light switching, a command palette, a plugin/Lua API, cd-on-quit shell integration, and an IPC/CLI for external control. Even tiny 2026 two-pane newcomers (pelorus, Directory Navigator, norte) ship most of these.

### Cited Findings
- yazi: trash, drag and drop, bulk create, command palette, auto theme, VFS/SFTP, per-task progress, editor bulk rename, fzf/zoxide, Kitty and Überzug++ images, mouse pane resize, and the `ya pkg` plugin manager — [CHANGELOG](https://github.com/sxyazi/yazi/blob/main/CHANGELOG.md)
- superfile: native trash on all OSes, chooser-file picking, cd-on-quit for bash/zsh/fish/PowerShell, and multiple themes — [release v1.6.1](https://github.com/yorukot/superfile/releases/tag/v1.6.1)
- nnn: native preview pane, fuzzy ranking, plugins, and Kitty drag and drop — [release v5.3](https://github.com/jarun/nnn/releases/tag/v5.3)
- broot: git status and diff preview, preview wrapping, clipboard verbs, trash verbs on Linux, and nerdfont icons — [CHANGELOG](https://github.com/Canop/broot/blob/main/CHANGELOG.md)
- pelorus (a two-pane reference point): background job queue with speed and ETA, multi-select, fuzzy filter, frecency jump list, bookmarks and marks, `fd`-backed recursive search, Chroma syntax preview, Glamour Markdown, git glyphs plus diff preview, `$EDITOR` bulk rename, Quick Look, archives as directories, SFTP and Tailscale — [README](https://github.com/mogglemoss/pelorus)
- Directory Navigator: archives (zip, tar, 7z, rar), AES zip creation, trash, directory compare, bookmarks, mouse support, color schemes and a command line — [README](https://github.com/deemoun/Directory-Navigator)
- norte: seven keymap presets, a command palette, WASM plugins, SFTP/FTP/S3 and archives as directories, an image and Markdown viewer, an embedded shell, and a journal (undo/audit) — [README](https://github.com/compilando/norte)
- XeFM: syntax-highlighted, image, Markdown, JSON and CSV viewers; directory compare and sync; Spotlight search; Finder tags — [README](https://github.com/crftwr/xefm)

### Inferences
- From the commit log in the brief, kura already has image and video preview (commit 1798f55) and a macOS input-source switch. I did not audit kura's code, so the "lacks" list should be checked against the repo. Likely gaps relative to the baseline: trash, async operations with progress, `$EDITOR` bulk rename, fuzzy/zoxide jump, archive/SFTP VFS, git status, config/keymap/theme files, a plugin system, cd-on-quit, and an external control socket/CLI. The last of these matters most for the ADE thesis.

### Gaps
- No feature-by-feature verification of kura's current code was done here (out of scope).

## Q5. What users praise and complain about in orthodox dual-pane TUIs (mc, far2l), 2025–2026

### Takeaway
I found little fresh 2025–2026 user discussion. Searches for Reddit/HN threads returned mostly listicles and AlternativeTo pages, and the HN Algolia API call failed in this environment. The documented, recurring points are the strengths (works over SSH and in a plain console, a built-in editor and viewer, an mc-style VFS) and the friction (F-key reliance clashes with terminal emulators, and the old-fashioned look and feel compared with yazi-style managers).

### Cited Findings
- mc works in a regular console, over SSH and in an X terminal. In a terminal emulator, "you'll have trouble using the function keys, which will be intercepted by the terminal itself" (TechRadar, older article, date not confirmed) — [TechRadar](https://techradar.com/news/software/applications/6-best-orthodox-file-managers-for-linux-605506/3)
- mc has a dual-pane layout, a built-in editor and group rename — [Wikipedia](https://en.wikipedia.org/wiki/Midnight_Commander)
- mc's development pace: one stable release in ~22 months (4.8.33, Jan 2025), with 4.8.34 still on a NEWS page — [NEWS-4.8.34](https://midnight-commander.org/wiki/NEWS-4.8.34?version=18)
- Many 2026 newcomers frame themselves as "the X that Y should have become" or as modern rebuilds (rcmd as an "mc replacement", Directory Navigator "rebuilt for modern computers", che taking TC/DC/MC ideas with async I/O, and Dualion, a GUI, as "the dual-pane file manager Total Commander should have become"). This implies perceived dissatisfaction with legacy commanders' UX — [rcmd](https://github.com/jaroslavpachola/rcmd); [Directory Navigator](https://github.com/deemoun/Directory-Navigator); [che](https://github.com/aroum/che); [Dualion](https://github.com/Halusak/dualion)
- The yazi/vifm discussion (#429) shows users who want a two-pane workflow being redirected to tabs — [discussion #429](https://www.github.com/sxyazi/yazi/discussions/429)

### Inferences
- The F-key clash with terminal emulators and multiplexers matters for kura. Inside an ADE or multiplexer such as Herdr, function and Alt keys are even more contested, so Vim-style bindings are a plausible advantage over mc/far2l-style F-key commanders.

### Gaps
- No direct, dated Reddit or HN user quotes from 2025–2026 about mc or far2l were retrieved. A report writer should not claim specific user sentiment beyond the points above.
