# ADEs, agent terminal workspaces, and how third-party TUIs plug in (as of 2026-11-23)

## Herdr: what it is, its architecture, plugin system, socket/CLI API, agent detection, graphics, and IME

### Takeaway
Herdr (github.com/ogulcancelik/herdr, herdr.dev) is a single Rust binary that runs as a background server with an attachable client inside your existing terminal. It is the ADE best suited to kura. Its plugin v1 is "any argv command plus a TOML manifest". A plugin can declare panes (overlay, popup, split, tab or zoomed), actions, event hooks and link handlers, and the whole herdr CLI/socket (NDJSON over a Unix socket) is the plugin API. kura could ship as a herdr plugin pane today and drive agents with `pane.send_text` / `agent.prompt` / `events.subscribe`. **Note:** the user has v0.7.4 installed, but upstream is at v0.9.5 (2026-10-14), and v0.9.2 and v0.9.4 changed Kitty graphics behaviour.

### Cited Findings
- **Identity:** The repo is github.com/ogulcancelik/herdr. It calls itself "the runtime your coding agents live on". It is a single Rust binary with no Electron that runs in your existing terminal. Terminals keep running in a background server when the client closes or SSH drops. Install with `curl -fsSL https://herdr.dev/install.sh | sh`, `brew install herdr` or `mise`. — [herdr README](https://raw.githubusercontent.com/ogulcancelik/herdr/master/README.md)
- **License conflict:** The README says Apache-2.0. The 0.1.0 crate metadata said AGPL-3.0-or-later, and a search summary mentions dual licensing. — [README](https://raw.githubusercontent.com/ogulcancelik/herdr/master/README.md); [docs.rs herdr 0.1.0](https://docs.rs/crate/herdr/0.1.0)
- **Model:** Workspaces contain tabs, and tabs contain panes. Panes are real terminal processes, and agent state (working / blocked / idle; blue dot when done in the background) is shown per pane. — [Better Stack guide](https://betterstack.com/community/guides/ai/herdr-ai-agent/); [README](https://raw.githubusercontent.com/ogulcancelik/herdr/master/README.md)
- **Remote and persistence:** SSH machines can be combined into one window. Restarts restore the layout and resume supported agent sessions, but the original processes do not survive. — [README](https://raw.githubusercontent.com/ogulcancelik/herdr/master/README.md) → [session-state docs](https://herdr.dev/docs/session-state/)
- **Unverified:** a third-party write-up claims a native desktop app on a new "herdr-core" runtime was announced on 2026-10-29. I did not confirm this from the project's own release notes, which do not mention it. — search summary of [flaviocopes.com/herdr](https://flaviocopes.com/herdr/); [herdr releases](https://github.com/ogulcancelik/herdr/releases)

**Plugin system (plugin v1)** — [herdr.dev/docs/plugins](https://herdr.dev/docs/plugins/)
- A plugin can be a Bash script, JS, Lua, a Rust binary, or any argv command. There is no plugin SDK and no WASM: "the full Herdr CLI is the plugin API".
- The manifest is `herdr-plugin.toml`, with required fields `id`, `name`, `version` and `min_herdr_version`, plus optional `description` and `platforms` (linux/macos/windows). Commands are argv arrays and are not run through a shell.
- **Manifest sections:**
  - `[[build]]` (runs at GitHub install)
  - `[[startup]]` (one-shot after session restore and socket readiness)
  - `[[actions]]` (`id`, `title`, `contexts` such as `["workspace"]`, `command`)
  - `[[events]]` (`on = "worktree.created"`, `command`)
  - `[[panes]]` (`id`, `title`, `placement`, `command`, optional `width`/`height`)
  - `[[link_handlers]]` (`id`, `title`, `pattern` Rust regex, `action`)
- **Pane placements:**
  - `overlay` (default): a temporary zoomed overlay that restores focus and zoom on close.
  - `popup`: session-modal and receives all input including Escape. It has no pane ID and emits no pane events. It returns `ui_busy` when another modal is open.
  - `split`, `tab`, `zoomed`: these become ordinary panes that `pane.move`, `pane.swap`, `pane.resize` and `pane.zoom` can control.
- **Opening a plugin pane:** `herdr plugin pane open --plugin <id> --entrypoint <pane>` or the socket method `plugin.pane.open`, which can override the placement.
- **Keybindings** go in the user config, not the manifest: `[[keys.command]] key="prefix+l" type="plugin_action" command="<plugin.id.action>"`.
- **Injected env:**
  - `HERDR_SOCKET_PATH`, `HERDR_BIN_PATH`, `HERDR_ENV=1`
  - `HERDR_PLUGIN_ID`, `_ROOT`, `_CONFIG_DIR`, `_STATE_DIR`, `_CONTEXT_JSON`
  - `HERDR_WORKSPACE_ID`, `HERDR_TAB_ID`, `HERDR_PANE_ID`
  - `HERDR_PLUGIN_EVENT_JSON` (event hooks), `HERDR_PLUGIN_ENTRYPOINT_ID` (panes)
  - `HERDR_PLUGIN_CLICKED_URL` (link handlers)
- **Link handlers:** Ctrl+click on a matching URL runs a plugin action instead of the browser. This requires `ui.mouse_capture`.
- **Plugin CLI:** `herdr plugin install owner/repo[/subdir]`, `update`, `link` (local dev), `list`, `action list|invoke`, `pane open`, `log list`, `unlink`, `uninstall`.
- **Not in v1:** runtime action registration and native non-terminal plugin UI. There is no storage API, and plugins are not sandboxed.
- **Marketplace:** public GitHub repos tagged `herdr-plugin` that contain a parseable manifest are indexed (refreshed every 30 minutes). The examples repo `ogulcancelik/herdr-plugin-examples` contains agent-telegram-notify, github-link-preview and dev-layout-bootstrap.

**Socket / CLI API** — [herdr.dev/docs/socket-api](https://herdr.dev/docs/socket-api/)
- **Socket:** `~/.config/herdr/herdr.sock`. Named sessions use `~/.config/herdr/sessions/<name>/herdr.sock`. The path is resolved from `--session`, then `HERDR_SOCKET_PATH`, then `HERDR_SESSION`.
- **Protocol:** newline-delimited JSON of the form `{"id","method","params"}`. The schema is available via `herdr api schema [--json]`, and `herdr api snapshot` returns the session snapshot.
- **Pane methods:**
  - `pane.split` (e.g. `{"direction":"right","ratio":0.333,"env":{...}}`), `swap`, `move`, `zoom`, `layout`, `neighbor`, `resize`, `list`, `current`, `get`, `rename`
  - `pane.send_text`, `pane.send_keys` (key names like `enter`, `esc`, `ctrl+h`), `pane.send_input` (backs `herdr pane run w1:p2 "npm test"`)
  - `pane.read` with `--source visible|recent|recent-unwrapped|detection`
  - `pane.report_agent`, `report_agent_session`, `report_metadata`, `close`, `wait_for_output`
  - Pane IDs look like `w1:p1`.
- **Agent methods:** `agent.list`, `get`, `read`, `explain`, `send_keys`, `prompt` (accepts `wait:{until,timeout_ms}`), `wait` (`herdr agent wait w1:p1 --until done|blocked`), `rename`, `focus`, `start`, `view.set`, `view.clear`.
- **Events:**
  - `events.subscribe` keeps the connection open and pushes events.
  - Event types include `pane.agent_status_changed`, `pane.output_matched`, `workspace.*`, `tab.*`, `pane.*`, `layout.updated` and `worktree.*`.
  - A slow subscriber gets `events_lost`; it should resubscribe and call `session.snapshot`.
- **Notifications:** `notification.show {title, body, sound}`, or the CLI `herdr notification show "…" --body "…"`. The reply's `reason` is shown, disabled, rate_limited or busy.
- **Worktrees:** `worktree.list`, `create {workspace_id, branch, focus}`, `open`, `remove`. `remove` runs `git worktree remove` and does not delete the branch.
- **Other:** `client.window_title.set`, `workspace.report_metadata`, and `plugin.*` methods.
- **Agent status semantics:** `pane.report_agent state ∈ idle|working|blocked|done|unknown` feeds waits, notifications and rollups. `pane.report_metadata` is display-only.

**Agent detection** — [herdr.dev/docs/integrations](https://herdr.dev/docs/integrations/)
- 18 integrations, including Claude Code, Codex, Copilot CLI, Devin, Kimi, Droid, OpenCode, Kilo, Hermes, Qoder, Qwen, Letta (experimental), Cursor Agent, MastraCode, Antigravity, Grok, Pi and OMP.
- Most agents get lifecycle state from herdr's "screen manifest detection". Hooks mainly report session identity for native restore.
- Pi, OMP, Kimi, OpenCode, Kilo and MastraCode report state directly through hooks. Devin uses screen manifest plus OSC detection.
- Third-party agents can integrate via [add-herdr-support docs](https://herdr.dev/docs/add-herdr-support/). The socket also exposes `server.agent_manifests` and `server.reload_agent_manifests` — [socket-api](https://herdr.dev/docs/socket-api/)

**Kitty graphics, keyboard and IME** — [herdr.dev/docs/configuration](https://herdr.dev/docs/configuration/); [herdr releases](https://github.com/ogulcancelik/herdr/releases)
- **Graphics config:** `[terminal] kitty_graphics` (images render by default in compatible outer terminals). Apps integrate by emitting the standard Kitty graphics protocol in pane output. The legacy key `experimental.kitty_graphics` is still accepted.
- **v0.9.2 (2026-09-29), BREAKING:**
  - The herdr-specific `pane.graphics.*` API now returns `unknown_method`. Apps write standard Kitty graphics, which herdr renders natively.
  - Local Ghostty receives images via temporary files.
  - Off-screen images stay loaded.
- **v0.9.4 (2026-10-13):**
  - Kitty keyboard: key releases and F1/F2/F4 fixed.
  - "Turning off Kitty graphics makes panes stop advertising image support".
  - Link-handling plugins get first pick of clicks.
  - `herdr plugin update` added.
- **v0.9.5 (2026-10-14):** prefix shortcuts work again with macOS Chinese, Japanese and Korean IMEs and with Ghostty `text:` keybinds.
- **CJK IME options:**
  - `[experimental] reveal_hidden_cursor_for_cjk_ime = true` makes macOS IME candidate windows follow agent TUIs that hide the cursor.
  - `cjk_ime_agents = ["claude","pi","codex"]` limits this to listed agents.
  - `switch_ascii_input_source_in_prefix = true` switches to an ASCII input source during prefix mode (macOS/Windows).
- **Notifications:** `[ui.toast] delivery = "herdr"|"terminal"|"system"|"off"`. "terminal" is an outer-terminal notification that also works over SSH. The docs do not say whether it uses OSC 9 or 777.

### Inferences
- **Plugin pane:** kura can be packaged as a herdr plugin (a repo with `herdr-plugin.toml` and the `herdr-plugin` topic). A `[[panes]]` entry with `placement="split"` (sidebar filer) or `"popup"` (quick picker) would launch `kura`. An `[[actions]]` entry bound to `prefix+f` would open it.
- **Detecting herdr:** inside herdr, kura can check `HERDR_ENV=1` and `HERDR_PANE_ID` and talk NDJSON to `HERDR_SOCKET_PATH` directly from Rust. No dependency is needed beyond serde_json and UnixStream.
- **Concrete feature ideas:**
  - "Send path to agent": `pane.neighbor` or `agent.list` to find the target, then `pane.send_text` with `@path`, or `agent.prompt`.
  - "Open the worktree of agent X": `worktree.list` and `agent.get` (cwd).
  - Mark files as touched when `pane.agent_status_changed → done` fires.
  - A `[[events]] on="worktree.created"` hook could open kura at the new worktree.
- **Popup caveat:** a popup has no pane ID and swallows Escape. kura's Vim-style Esc handling works there, but a popup-mode kura cannot be targeted by pane APIs.
- **Version gap:** the local v0.7.4 is two minor versions behind. The DSR/DA-vs-Kitty-reply ordering quirk the user saw may already be fixed or changed by the 0.9.2 graphics rewrite and the 0.9.4 change to "advertising image support". Re-test after upgrading before working around it in kura. This is an inference: the changelog does not mention query ordering.

### Gaps
- The full list of `[[events]] on=` names for plugins is not documented on the plugins page; only `worktree.created` is shown. Socket event types are listed, but the mapping to plugin hooks is unclear.
- The changelogs for v0.7.x and v0.8.x, and their exact Kitty query behaviour, were not retrieved.
- Whether herdr passes through or handles OSC 7, OSC 8, OSC 52 and Sixel is not documented on the pages fetched.
- I could not verify the desktop/browser app client or "herdr-core" from primary sources.
- The license is inconsistent across sources.

## Orca: which Orca it is, plus its features and extension points

### Takeaway
"Orca" is almost certainly **stablyai/orca** (orca.dev). It is an MIT-licensed, cross-platform (macOS/Windows/Linux) desktop ADE that runs any CLI agent in a terminal per git worktree. It has a built-in GUI file explorer, rich previews, diffs, an in-app browser, an `orca` CLI, a plugin marketplace with sidebar panels, and a mobile companion app. Orca already has its own GUI file explorer, so kura's role there would be "a terminal TUI running inside an Orca terminal pane" rather than a native plugin.

### Cited Findings
- **Basics:** free, open-source (MIT) desktop app. "Works with any CLI agent — if it runs in a terminal, it runs in Orca". It names around 40 agents (Claude Code, Codex, Cursor, Copilot CLI, OpenCode, Amp, Goose, Kilocode and more). Each agent can run in Chat UI or as the raw CLI. — [Orca README](https://raw.githubusercontent.com/stablyai/orca/main/README.md); [Better Stack guide](https://betterstack.com/community/guides/ai/orca-ai/)
- **Worktrees and CLI:**
  - Each task gets a worktree, which can be local, on a Remote Orca Server, on an SSH host, or on a cloud VM.
  - "Anything you can do in Orca, your agents can do too" via the CLI, e.g. `orca worktree create`, orchestration, automations and computer use.
  - Docs: [cli/overview](https://orca.dev/docs/cli/overview)
  - Source: [README](https://raw.githubusercontent.com/stablyai/orca/main/README.md)
- **Layout:** a project and agent sidebar, a central terminal and file-tab area, a file explorer, and an in-app browser. Panes can hold agents, terminals, browsers and diffs side by side. — [Better Stack](https://betterstack.com/community/guides/ai/orca-ai/); [README](https://raw.githubusercontent.com/stablyai/orca/main/README.md)
- **File explorer** — [orca.dev/docs/editing/file-explorer](https://orca.dev/docs/editing/file-explorer)
  - Updates in real time and colours files by git status.
  - Right-click actions: discard, stage, rename, Copy Path, Copy Relative Path, and Copy (puts the file on the OS clipboard).
  - Reveal in Finder, and Find in Folder.
  - Dropping files onto an agent terminal pastes their paths at the prompt in the pane under the cursor. For SSH worktrees the file is uploaded first.
  - The docs do not say whether paths are inserted as `@mentions` or raw.
- **Previews and review:** rich previews for images, PDFs and Markdown — [README](https://raw.githubusercontent.com/stablyai/orca/main/README.md). Inline diff comments are sent back to the agent — [Paradev comparison](https://paradev.com/compare/orca-vs-conductor) (vendor page, updated 2026-11-05).
- **Plugins:** 441 plugins. Categories include Themes (74), Languages (40), Editors (5) and Commands & panels (258). Panels live in the left or right sidebar. Kube Panel can "open a read-only k9s terminal". No file-manager plugin is listed, and the page does not show an authoring API or manifest. — [orca.dev/plugins](https://orca.dev/plugins)
- **Mobile:** iOS and Android companion app for monitoring and steering agents. — [README](https://raw.githubusercontent.com/stablyai/orca/main/README.md)

### Inferences
- **Fit:** kura in Orca is mainly a TUI in an Orca terminal. Value it could add over Orca's GUI explorer: keyboard-only two-pane operations, worktree-to-worktree copy and compare, and image preview (if Orca's terminal supports Kitty graphics, which is unknown).
- **Plugin option:** a "panel" plugin that launches a terminal TUI exists in principle (the k9s example). An Orca panel hosting kura might be possible, but the authoring API is unverified.

### Gaps
- Orca's terminal engine (xterm.js? Ghostty-based?) is not documented on the pages fetched, so Kitty graphics, Sixel and OSC support are unknown.
- The plugin authoring API and manifest are not found.
- Whether Orca exposes a socket or IPC for a TUI to call (beyond the `orca` CLI) is unknown.

## Other ADEs and agent workspaces (2025–2026): how they host or talk to TUIs

### Takeaway
The category has consolidated around "worktree per agent + terminal panes". Vendors include Superset (renamed Paradev in Oct 2026), Conductor (being shut down after joining Meta), Orca, Emdash and Vibe Kanban (reported sunsetting). Most are GUI desktop apps. Herdr and tmux/Zellij-style tools are the terminal-native ones where a TUI file manager is a first-class citizen. Claude Code's own extension surfaces are hooks, MCP, and the IDE lock-file/WebSocket protocol. The IDE protocol is the most direct way for a non-editor tool to "@mention a file" into a running Claude Code session.

### Cited Findings
- **Paradev (ex-Superset):** Superset was renamed Paradev in October 2026. Orca is MIT and cross-platform. Conductor is proprietary and macOS-only, exposes an API and an MCP server, and "Joining Meta (announced November 4, 2026)". Conductor is closed to new users, Conductor Cloud shut down on 2026-11-20, and the local Mac app winds down after early 2027. — [Paradev: Orca vs Conductor](https://paradev.com/compare/orca-vs-conductor) (vendor-authored; treat with care)
- **Vibe Kanban:** reported as sunsetting (Bloop), continuing as an unmaintained community project. Its model is board cards mapped to agent runs. — [Superset vs Vibe Kanban](https://superset.sh/compare/superset-vs-vibe-kanban) (vendor source, unverified independently)
- **Other names:**
  - Emdash is positioned around issue-tracker intake. Other names in aggregator lists include JetBrains Air, Google Antigravity, ZCode, Podium and Letta ADE. — [Morph comparison](https://www.morphllm.com/comparisons/superset-vs-conductor-vs-emdash); [SourceForge ADE list](https://sourceforge.net/software/agent-development-environments-ade/)
  - The ADE definition is "a task with an agent, an environment, a branch, and a result". — [superset.sh what-is-an-ADE](https://superset.sh/compare/what-is-an-agent-development-environment)
- **Claude Code hooks:** 33 events, including `SessionStart`, `UserPromptSubmit`, `Stop`, `PreToolUse`/`PostToolUse` (`tool_input.file_path` for file tools), `Notification`, `CwdChanged`, `FileChanged`, `WorktreeCreate`/`WorktreeRemove` and `MessageDisplay`.
  - Handler types: `command` (JSON on stdin), `http` (POST), `mcp_tool`, `prompt` and `agent`.
  - Every event carries `session_id`, `transcript_path` and `cwd`.
  - Hooks can return `additionalContext`, capped at 10,000 characters. The docs show no external "push prompt into a live session" API.
  - Source: [code.claude.com/docs/en/hooks](https://code.claude.com/docs/en/hooks)
- **Claude Code IDE protocol** (as implemented by claudecode.nvim):
  - The IDE runs a WebSocket JSON-RPC MCP server on 127.0.0.1.
  - It writes `~/.claude/ide/<port>.lock` (0600, includes an auth token). Claude Code connects with header `x-claude-code-ide-authorization`.
  - Env vars: `ENABLE_IDE_INTEGRATION`, `CLAUDE_CODE_SSE_PORT`, `FORCE_CODE_TERMINAL`.
  - The IDE broadcasts `at_mentioned` and `selection_changed` notifications and serves tools such as `openDiff` (blocks until accept/reject) and `saveDocument`.
  - File explorers (nvim-tree, neo-tree, oil, mini.files, netrw, snacks) send files via `:ClaudeCodeTreeAdd` / `send_at_mention`, with an optional line range.
  - Source: [claudecode.nvim README](https://raw.githubusercontent.com/coder/claudecode.nvim/main/README.md)
- **Zellij:**
  - WASM plugins receive `zellij pipe` / `zellij action pipe` messages (name, payload, args) through a `pipe` lifecycle method.
  - Plugins can block, unblock and write to the CLI's stdout, and message each other with `pipe_message_to_plugin`.
  - `zellij action pipe --popup` exists.
  - Source: [zellij.dev plugin-pipes](https://zellij.dev/documentation/plugin-pipes)

### Inferences
- **Claude Code as an "IDE" target:** kura could implement the Claude Code IDE protocol (lock file plus WebSocket MCP). Pressing a key in kura would then send `at_mentioned` for the selected files or line range into whichever Claude Code session connects (`/ide`). This works in any ADE because it bypasses the terminal entirely. **Risk:** it is an undocumented, reverse-engineered protocol owned by Anthropic.
- **Agent-agnostic fallbacks:**
  - Herdr `pane.send_text "@path "`.
  - A bracketed-paste of the path into the agent pane, mirroring Orca's drag-and-drop "paste paths" behaviour.
  - OSC 52 copy of the path.
- **Hooks feed kura:** a `PostToolUse` hook on `Edit|Write` can append `tool_input.file_path` to a file or socket that kura watches. That gives a "files the agent just changed" virtual directory/filter, which is a natural two-pane feature: left pane = agent-changed list, right pane = worktree.

### Gaps
- Not researched in this pass, so marked unknown: Warp agent mode and its extension model, Zed agent panel/ACP, Cursor/Windsurf terminal specifics, Ghostty's 2026 features (e.g. scripting API), Crystal, Claude Squad and other tmux-based managers, and OpenAI Codex CLI hooks/IDE protocol.
- Paradev's and Superset's own extension points were not examined.
- Official Anthropic documentation of the IDE lock-file protocol was not found; it is known via third-party implementations only.

## Common integration primitives (worktrees, OSC, Kitty graphics, keyboard, MCP, send-path-to-agent)

### Takeaway
Git worktrees per agent are universal (herdr `worktree.*`, Orca, Superset/Paradev, Conductor, Claude Code `WorktreeCreate` hooks). Images inside multiplexers should go through the standard Kitty graphics protocol; herdr dropped its custom API in favour of this in v0.9.2. Capability detection must follow the Kitty spec: send the `a=q` query, then DA1, and treat "DA1 reply without a graphics reply" as unsupported. The spec says terminals must answer queries immediately, so herdr answering after DSR/DA is a herdr-side ordering violation.

### Cited Findings
- **Kitty spec on detection:** detect support by sending the query action followed by a primary device attributes (DA1) request. "If you get back an answer for the device attributes without getting back an answer for the query action the terminal emulator does not support the graphics protocol". Terminals "must reply to query actions immediately without processing other input". — [Kitty graphics protocol](https://sw.kovidgoyal.net/kitty/graphics-protocol/)
- **Unicode placeholders (`U=1`):** the image is uploaded with `q=2` and shown wherever `U+10EEEE` plus diacritics are printed. This lets images work inside multiplexers like tmux without escape passthrough. — [Kitty graphics protocol](https://sw.kovidgoyal.net/kitty/graphics-protocol/)
- **Herdr graphics:**
  - Renders standard Kitty graphics emitted by pane apps and stops advertising image support when it is disabled. — [herdr releases](https://github.com/ogulcancelik/herdr/releases)
  - Has a `terminal.kitty_graphics` toggle. — [herdr config](https://herdr.dev/docs/configuration/)
- **Kitty keyboard protocol inside herdr:** key-release and F-key fixes in v0.9.4. — [herdr releases](https://github.com/ogulcancelik/herdr/releases)
- **Worktree primitives:**
  - herdr: `worktree.create/open/remove` and `worktree.*` events. — [socket-api](https://herdr.dev/docs/socket-api/)
  - Claude Code: `WorktreeCreate`/`WorktreeRemove` hooks. — [hooks](https://code.claude.com/docs/en/hooks)
  - Orca: worktree model and `orca worktree create`. — [Orca README](https://raw.githubusercontent.com/stablyai/orca/main/README.md)
- **Send-file-to-agent patterns:**
  - Orca: drag-and-drop pastes paths into the agent terminal. — [Orca file-explorer docs](https://orca.dev/docs/editing/file-explorer)
  - claudecode.nvim: `at_mentioned` over the IDE WebSocket. — [claudecode.nvim](https://raw.githubusercontent.com/coder/claudecode.nvim/main/README.md)
  - herdr: `pane.send_text` / `agent.prompt`. — [socket-api](https://herdr.dev/docs/socket-api/)
- **Notifications:** herdr exposes `notification.show` and toast delivery "terminal" for outer-terminal notifications that work over SSH. — [socket-api](https://herdr.dev/docs/socket-api/); [config](https://herdr.dev/docs/configuration/)

### Inferences
- **Robust detection for kura:**
  - Send the Kitty `a=q` query, then DA1, and wait for the DA1 reply plus a short grace window (or keep reading until a timeout). Do not stop at the first DSR reply.
  - Alternatively, when `HERDR_ENV=1` is set, trust herdr's config (images on by default) or let the user force a protocol via config.
  - Consider Unicode-placeholder mode for tmux and other multiplexers.
- **Minimal "ADE-aware" layer, ordered by portability:**
  1. OSC 52 copy of the path, and OSC 8 hyperlinks in the listing.
  2. "Send @path to the agent pane" via a herdr socket when present, else via bracketed paste or tmux `send-keys`.
  3. An optional Claude Code IDE-protocol server.
  4. A herdr plugin manifest that registers kura as a split/popup pane plus a `worktree.created` hook.

### Gaps
- OSC 7, 9 and 777 support across herdr and Orca was not verified from primary sources.
- Sixel support in herdr and Orca is unknown.
- An MCP server exposing file-manager operations was not searched in detail.

## Existing file managers and pickers built for agent workflows

### Takeaway
I found no dedicated "file manager for agents" product. The closest are editor-side bridges: claudecode.nvim's explorer-to-`@mention` integration, and Yazi's `ya pub-to` event bus, which could be scripted to forward a path. Orca's GUI explorer, with drag-to-prompt and git-status colouring, is the de-facto reference feature set. This gap is an opportunity for kura.

### Cited Findings
- **claudecode.nvim:** `:ClaudeCodeTreeAdd` adds the file under the cursor in nvim-tree, neo-tree, oil.nvim, mini.files, netrw or the Snacks explorer to Claude's context. It fires a `ClaudeCodeSendComplete` event with `file_path`, `start_line` and `end_line`. — [claudecode.nvim README](https://raw.githubusercontent.com/coder/claudecode.nvim/main/README.md)
- **Yazi:** `ya pub-to` can publish JSON events such as the hovered path from a keymap. No ready-made yazi→Claude Code bridge was found in search. — search over [yazi docs/resources](https://yazi-rs.github.io/docs/resources) and [yazi discussions #848](https://github.com/sxyazi/yazi/discussions/848)
- **Orca:** no file-manager plugins are listed in its 441-plugin marketplace. — [orca.dev/plugins](https://orca.dev/plugins)
- **Herdr:** its example plugins are notify, link-preview and layout-bootstrap. None is a file manager. — [herdr plugins docs](https://herdr.dev/docs/plugins/)

### Inferences
- **Positioning:** "the two-pane filer of the ADE ecosystem" is currently unclaimed. A herdr marketplace plugin, `kura` tagged `herdr-plugin`, would likely be the first file-manager plugin there.
- **Differentiating features grounded in the primitives above:**
  - Worktree-aware pane pairing: left = main checkout, right = agent worktree, with a diff/compare mark.
  - An "agent-changed files" view fed by hooks or `pane.agent_status_changed`.
  - A one-key `@path` send to the focused or neighbouring agent.
  - Kitty image preview that works inside herdr.

### Gaps
- "Review agent changes" diff TUIs and lazygit integration in ADEs were not researched in this pass.
- File-picker MCP servers were not surveyed.
