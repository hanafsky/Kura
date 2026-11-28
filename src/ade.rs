//! ADE integration: talk to the agent terminal kura runs in.
//!
//! - **herdr** (TUI): `HERDR_ENV=1` and `HERDR_SOCKET_PATH`. Requests go to
//!   the socket as NDJSON (one JSON object per line, answered by one line
//!   with the same `id`); see [`HerdrClient`].
//! - **herdr-core** (Herdr.app, `hc`): `HC_TERMINAL_ID`. Detected, but `Y`
//!   still uses the clipboard: `hc send` always presses Enter and `hc keys`
//!   only takes named keys (no `@`, no non-ASCII), so text cannot be put in
//!   an agent's input without submitting it.
//! - Neither: the clipboard ([`crate::clipboard`]).
//!
//! The environment is read once at start ([`Ade::from_env`]) and kept in
//! [`App::ade`]; tests build apps with [`Ade::None`], so they never reach a
//! real terminal.

use std::path::{Path, PathBuf};
use std::time::Duration;

use serde_json::{json, Value};

use crate::actions::target_paths;
use crate::app::App;
use crate::clipboard::{self, format_at_paths};
use crate::mode::Mode;

/// How long one socket request may take (each of connect / write / read),
/// so a stuck herdr cannot freeze kura.
pub const HERDR_TIMEOUT: Duration = Duration::from_secs(1);

/// The agent terminal kura runs in.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum Ade {
    /// Not inside a known agent terminal.
    #[default]
    None,
    /// TUI herdr, reached over its socket.
    Herdr(HerdrEnv),
    /// Herdr.app / herdr-core (`hc`).
    HerdrCore { terminal_id: String },
}

/// Where kura sits in herdr.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HerdrEnv {
    /// `HERDR_SOCKET_PATH`.
    pub socket: PathBuf,
    /// `HERDR_PANE_ID`: kura's own pane, never a send target.
    pub pane_id: Option<String>,
    /// `HERDR_WORKSPACE_ID`: targets are limited to this workspace.
    pub workspace_id: Option<String>,
    /// `HERDR_TAB_ID`: agents in this tab are listed first.
    pub tab_id: Option<String>,
}

/// The environment variables that decide [`Ade`].
#[derive(Debug, Default, Clone)]
pub struct AdeVars {
    pub herdr_env: Option<String>,
    pub herdr_socket: Option<String>,
    pub herdr_pane_id: Option<String>,
    pub herdr_workspace_id: Option<String>,
    pub herdr_tab_id: Option<String>,
    pub hc_terminal_id: Option<String>,
}

impl AdeVars {
    fn current() -> Self {
        let var = |k: &str| std::env::var(k).ok();
        AdeVars {
            herdr_env: var("HERDR_ENV"),
            herdr_socket: var("HERDR_SOCKET_PATH"),
            herdr_pane_id: var("HERDR_PANE_ID"),
            herdr_workspace_id: var("HERDR_WORKSPACE_ID"),
            herdr_tab_id: var("HERDR_TAB_ID"),
            hc_terminal_id: var("HC_TERMINAL_ID"),
        }
    }
}

/// Decide the agent terminal from the environment variables.
///
/// TUI herdr wins over herdr-core: when both are set, herdr runs inside an
/// `hc` terminal and kura sits in one of herdr's panes. Empty values count
/// as unset.
pub fn detect(vars: &AdeVars) -> Ade {
    fn set(v: &Option<String>) -> Option<String> {
        v.as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(String::from)
    }
    if cfg!(unix) && set(&vars.herdr_env).as_deref() == Some("1") {
        if let Some(socket) = set(&vars.herdr_socket) {
            return Ade::Herdr(HerdrEnv {
                socket: PathBuf::from(socket),
                pane_id: set(&vars.herdr_pane_id),
                workspace_id: set(&vars.herdr_workspace_id),
                tab_id: set(&vars.herdr_tab_id),
            });
        }
    }
    if let Some(terminal_id) = set(&vars.hc_terminal_id) {
        return Ade::HerdrCore { terminal_id };
    }
    Ade::None
}

impl Ade {
    /// Detect from the process environment.
    pub fn from_env() -> Self {
        detect(&AdeVars::current())
    }
}

/// An agent pane that `@path` references can be sent to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Agent {
    pub pane_id: String,
    pub workspace_id: String,
    pub tab_id: String,
    /// Agent kind for display (`claude`, `codex`, ...).
    pub agent: Option<String>,
    /// Name given to the agent, if any.
    pub name: Option<String>,
    /// The agent's working directory: `@path` is made relative to it.
    pub cwd: Option<PathBuf>,
}

impl Agent {
    /// Parse one entry of `agent.list`. `None` without a pane id.
    fn from_json(v: &Value) -> Option<Self> {
        let s = |k: &str| v.get(k).and_then(Value::as_str).map(String::from);
        Some(Agent {
            pane_id: s("pane_id")?,
            workspace_id: s("workspace_id").unwrap_or_default(),
            tab_id: s("tab_id").unwrap_or_default(),
            agent: s("display_agent").or_else(|| s("agent")),
            name: s("name"),
            cwd: s("cwd").map(PathBuf::from),
        })
    }

    /// One line for the target dialog and the status line.
    pub fn label(&self) -> String {
        let kind = self.agent.as_deref().unwrap_or("agent");
        match &self.name {
            Some(name) if name != kind => format!("{kind} \"{name}\" ({})", self.pane_id),
            _ => format!("{kind} ({})", self.pane_id),
        }
    }

    /// Short name for result messages.
    fn short_name(&self) -> &str {
        self.name
            .as_deref()
            .or(self.agent.as_deref())
            .unwrap_or(&self.pane_id)
    }
}

/// Send targets among the listed agents: not kura's own pane, only in
/// kura's workspace (when known), agents in kura's tab first.
pub fn send_targets(agents: Vec<Agent>, env: &HerdrEnv) -> Vec<Agent> {
    let mut targets: Vec<Agent> = agents
        .into_iter()
        .filter(|a| env.pane_id.as_deref() != Some(a.pane_id.as_str()))
        .filter(|a| {
            env.workspace_id
                .as_deref()
                .is_none_or(|w| w == a.workspace_id)
        })
        .collect();
    // Stable: keeps herdr's order within each group.
    targets.sort_by_key(|a| env.tab_id.as_deref() != Some(a.tab_id.as_str()));
    targets
}

/// The text typed into the agent's input: `@path` references relative to
/// `base`, then a space so the next word does not stick to the last path.
pub fn send_text(paths: &[PathBuf], base: &Path) -> String {
    format!("{} ", format_at_paths(paths, base, clipboard::path_style()))
}

/// A connection-per-request client for the herdr socket API.
///
/// [`HerdrClient::request`] is generic, so other features (worktrees, ...)
/// can send their own methods.
pub struct HerdrClient {
    socket: PathBuf,
    timeout: Duration,
}

impl HerdrClient {
    pub fn new(socket: impl Into<PathBuf>) -> Self {
        Self::with_timeout(socket, HERDR_TIMEOUT)
    }

    pub fn with_timeout(socket: impl Into<PathBuf>, timeout: Duration) -> Self {
        Self {
            socket: socket.into(),
            timeout,
        }
    }

    /// Send one request and return its `result`. An error response becomes
    /// its message; a stuck server fails after the timeout.
    #[cfg(unix)]
    pub fn request(&self, method: &str, params: Value) -> Result<Value, String> {
        use std::io::{BufRead, BufReader, Write};
        use std::os::unix::net::UnixStream;
        use std::sync::atomic::{AtomicU64, Ordering};
        use std::time::Instant;

        static NEXT_ID: AtomicU64 = AtomicU64::new(1);
        let id = format!("kura-{}", NEXT_ID.fetch_add(1, Ordering::Relaxed));
        let io_err = |e: std::io::Error| match e.kind() {
            std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut => {
                "herdr did not respond".to_string()
            }
            _ => format!("herdr: {e}"),
        };

        let stream = UnixStream::connect(&self.socket).map_err(io_err)?;
        stream
            .set_write_timeout(Some(self.timeout))
            .map_err(io_err)?;
        let mut line = json!({ "id": id, "method": method, "params": params }).to_string();
        line.push('\n');
        (&stream).write_all(line.as_bytes()).map_err(io_err)?;

        // Set once: macOS refuses socket options after the peer closed, and
        // the reply may already be buffered by then. Lines for other ids
        // are skipped until the deadline.
        stream
            .set_read_timeout(Some(self.timeout))
            .map_err(io_err)?;
        let deadline = Instant::now() + self.timeout;
        let mut reader = BufReader::new(&stream);
        loop {
            if Instant::now() >= deadline {
                return Err("herdr did not respond".into());
            }
            let mut buf = String::new();
            if reader.read_line(&mut buf).map_err(io_err)? == 0 {
                return Err("herdr closed the connection".into());
            }
            let Ok(resp) = serde_json::from_str::<Value>(&buf) else {
                continue;
            };
            if resp.get("id").and_then(Value::as_str) != Some(id.as_str()) {
                continue;
            }
            if let Some(err) = resp.get("error") {
                let msg = err
                    .get("message")
                    .and_then(Value::as_str)
                    .unwrap_or("unknown error");
                return Err(format!("herdr: {msg}"));
            }
            return resp
                .get("result")
                .cloned()
                .ok_or_else(|| "herdr: response without result".to_string());
        }
    }

    #[cfg(not(unix))]
    pub fn request(&self, _method: &str, _params: Value) -> Result<Value, String> {
        Err("herdr sockets need a Unix system".into())
    }

    /// `agent.list`: every agent pane herdr knows.
    pub fn agent_list(&self) -> Result<Vec<Agent>, String> {
        let result = self.request("agent.list", json!({}))?;
        let agents = result
            .get("agents")
            .and_then(Value::as_array)
            .ok_or("herdr: unexpected agent.list result")?;
        Ok(agents.iter().filter_map(Agent::from_json).collect())
    }

    /// `pane.send_text`: type `text` into the pane without pressing Enter.
    pub fn send_text(&self, pane_id: &str, text: &str) -> Result<(), String> {
        self.request(
            "pane.send_text",
            json!({ "pane_id": pane_id, "text": text }),
        )
        .map(|_| ())
    }
}

/// `Y`: send the marked entries (or the cursor entry) as `@path`
/// references to an agent pane in herdr; elsewhere copy them to the
/// clipboard. With several agents a dialog asks which one.
pub fn send_at_paths(app: &mut App) {
    let Ade::Herdr(env) = app.ade.clone() else {
        clipboard::copy_at_paths(app);
        return;
    };
    let paths = target_paths(app);
    if paths.is_empty() {
        return;
    }
    let agents = match HerdrClient::new(&env.socket).agent_list() {
        Ok(agents) => agents,
        Err(e) => {
            app.notify_error(format!("Send failed: {e}"));
            return;
        }
    };
    let mut targets = send_targets(agents, &env);
    match targets.len() {
        0 => {
            if clipboard::copy_paths(app, &paths) {
                app.notify_info(format!(
                    "No agent pane found; copied {} path(s)",
                    paths.len()
                ));
            }
        }
        1 => send_to(app, &env, &targets.remove(0), &paths),
        _ => {
            app.mode = Mode::SendTarget {
                targets,
                selected: 0,
                paths,
            }
        }
    }
}

/// Enter in the target dialog: send to the highlighted agent.
pub fn confirm_send_target(app: &mut App) {
    let Mode::SendTarget {
        targets,
        selected,
        paths,
    } = std::mem::replace(&mut app.mode, Mode::Filer)
    else {
        return;
    };
    let (Ade::Herdr(env), Some(agent)) = (app.ade.clone(), targets.get(selected)) else {
        return;
    };
    send_to(app, &env, agent, &paths);
}

fn send_to(app: &mut App, env: &HerdrEnv, agent: &Agent, paths: &[PathBuf]) {
    let base = agent
        .cwd
        .as_deref()
        .unwrap_or(&app.launch_dir)
        .to_path_buf();
    let text = send_text(paths, &base);
    match HerdrClient::new(&env.socket).send_text(&agent.pane_id, &text) {
        Ok(()) => {
            app.current_pane_mut().marked.clear();
            app.notify_info(format!(
                "Sent {} path(s) to {}",
                paths.len(),
                agent.short_name()
            ));
        }
        Err(e) => app.notify_error(format!("Send failed: {e}")),
    }
}

#[cfg(test)]
pub mod tests {
    use super::*;

    fn vars(pairs: &[(&str, &str)]) -> AdeVars {
        let get = |k: &str| {
            pairs
                .iter()
                .find(|(name, _)| *name == k)
                .map(|(_, v)| v.to_string())
        };
        AdeVars {
            herdr_env: get("HERDR_ENV"),
            herdr_socket: get("HERDR_SOCKET_PATH"),
            herdr_pane_id: get("HERDR_PANE_ID"),
            herdr_workspace_id: get("HERDR_WORKSPACE_ID"),
            herdr_tab_id: get("HERDR_TAB_ID"),
            hc_terminal_id: get("HC_TERMINAL_ID"),
        }
    }

    #[test]
    fn detects_tui_herdr() {
        let ade = detect(&vars(&[
            ("HERDR_ENV", "1"),
            ("HERDR_SOCKET_PATH", "/s/herdr.sock"),
            ("HERDR_PANE_ID", "w1:p1"),
            ("HERDR_WORKSPACE_ID", "w1"),
            ("HERDR_TAB_ID", "w1:t1"),
        ]));
        assert_eq!(
            ade,
            Ade::Herdr(HerdrEnv {
                socket: PathBuf::from("/s/herdr.sock"),
                pane_id: Some("w1:p1".into()),
                workspace_id: Some("w1".into()),
                tab_id: Some("w1:t1".into()),
            })
        );
    }

    #[test]
    fn detects_herdr_core() {
        assert_eq!(
            detect(&vars(&[("HC_TERMINAL_ID", "t7")])),
            Ade::HerdrCore {
                terminal_id: "t7".into()
            }
        );
    }

    #[test]
    fn tui_herdr_wins_over_herdr_core() {
        let ade = detect(&vars(&[
            ("HERDR_ENV", "1"),
            ("HERDR_SOCKET_PATH", "/s/herdr.sock"),
            ("HC_TERMINAL_ID", "t7"),
        ]));
        assert!(matches!(ade, Ade::Herdr(_)));
    }

    #[test]
    fn herdr_needs_env_flag_and_socket() {
        assert_eq!(detect(&vars(&[])), Ade::None);
        assert_eq!(detect(&vars(&[("HERDR_ENV", "1")])), Ade::None);
        assert_eq!(
            detect(&vars(&[("HERDR_ENV", "1"), ("HERDR_SOCKET_PATH", "")])),
            Ade::None
        );
        assert_eq!(
            detect(&vars(&[("HERDR_ENV", "0"), ("HERDR_SOCKET_PATH", "/s")])),
            Ade::None
        );
        // Without a socket, an hc terminal around herdr is still found.
        assert_eq!(
            detect(&vars(&[("HERDR_ENV", "1"), ("HC_TERMINAL_ID", "t7")])),
            Ade::HerdrCore {
                terminal_id: "t7".into()
            }
        );
        assert_eq!(detect(&vars(&[("HC_TERMINAL_ID", " ")])), Ade::None);
    }

    pub fn agent(pane: &str, tab: &str, name: Option<&str>) -> Agent {
        Agent {
            pane_id: pane.into(),
            workspace_id: pane.split(':').next().unwrap().into(),
            tab_id: tab.into(),
            agent: Some("claude".into()),
            name: name.map(String::from),
            cwd: None,
        }
    }

    fn env() -> HerdrEnv {
        HerdrEnv {
            socket: PathBuf::from("/s"),
            pane_id: Some("w1:p1".into()),
            workspace_id: Some("w1".into()),
            tab_id: Some("w1:t2".into()),
        }
    }

    #[test]
    fn targets_skip_own_pane_and_other_workspaces() {
        let agents = vec![
            agent("w1:p1", "w1:t2", None),
            agent("w2:p1", "w2:t1", None),
            agent("w1:p5", "w1:t1", None),
            agent("w1:p3", "w1:t2", None),
        ];
        let ids: Vec<_> = send_targets(agents, &env())
            .into_iter()
            .map(|a| a.pane_id)
            .collect();
        // Own tab first, then the rest of the workspace.
        assert_eq!(ids, ["w1:p3", "w1:p5"]);
    }

    #[test]
    fn targets_without_location_keep_every_other_agent() {
        let env = HerdrEnv {
            socket: PathBuf::from("/s"),
            pane_id: None,
            workspace_id: None,
            tab_id: None,
        };
        let agents = vec![agent("w1:p1", "w1:t1", None), agent("w2:p1", "w2:t1", None)];
        assert_eq!(send_targets(agents, &env).len(), 2);
    }

    #[test]
    fn parses_agent_list_entries() {
        let v = json!({
            "pane_id": "w1:p2", "workspace_id": "w1", "tab_id": "w1:t1",
            "agent": "claude", "display_agent": "Claude Code", "name": "fixer",
            "cwd": "/proj", "agent_status": "idle", "focused": false,
            "terminal_id": "x", "revision": 0
        });
        let a = Agent::from_json(&v).unwrap();
        assert_eq!(a.agent.as_deref(), Some("Claude Code"));
        assert_eq!(a.cwd, Some(PathBuf::from("/proj")));
        assert_eq!(a.label(), "Claude Code \"fixer\" (w1:p2)");
        assert_eq!(a.short_name(), "fixer");
        assert!(Agent::from_json(&json!({ "agent": "claude" })).is_none());
        let bare = agent("w1:p3", "w1:t1", None);
        assert_eq!(bare.label(), "claude (w1:p3)");
    }

    #[test]
    fn send_text_is_relative_with_a_trailing_space() {
        let paths = vec![PathBuf::from("/proj/src/a.rs"), PathBuf::from("/proj/b c")];
        assert_eq!(send_text(&paths, Path::new("/proj")), "@src/a.rs @\"b c\" ");
    }

    /// A fake herdr: answers each request line with `reply(request)`
    /// (`None`: never answers) and records the requests.
    #[cfg(unix)]
    pub struct FakeHerdr {
        pub socket: PathBuf,
        pub requests: std::sync::Arc<std::sync::Mutex<Vec<Value>>>,
        _dir: tempfile::TempDir,
    }

    #[cfg(unix)]
    impl FakeHerdr {
        pub fn start(reply: impl Fn(&Value) -> Option<Value> + Send + 'static) -> Self {
            use std::io::{BufRead, BufReader, Write};
            use std::os::unix::net::UnixListener;
            // Short path: Unix socket paths are limited to ~104 bytes.
            let dir = tempfile::Builder::new()
                .prefix("kh")
                .tempdir_in("/tmp")
                .unwrap();
            let socket = dir.path().join("s");
            let listener = UnixListener::bind(&socket).unwrap();
            let requests = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
            let log = requests.clone();
            std::thread::spawn(move || {
                for stream in listener.incoming() {
                    let Ok(mut stream) = stream else { return };
                    let mut line = String::new();
                    if BufReader::new(&stream).read_line(&mut line).is_err() {
                        continue;
                    }
                    let req: Value = serde_json::from_str(&line).unwrap();
                    log.lock().unwrap().push(req.clone());
                    match reply(&req) {
                        // An array is written as several lines.
                        Some(Value::Array(lines)) => {
                            for l in lines {
                                let _ = writeln!(stream, "{l}");
                            }
                        }
                        Some(resp) => {
                            let _ = writeln!(stream, "{resp}");
                        }
                        // Hold the connection open without answering.
                        None => std::thread::sleep(Duration::from_secs(5)),
                    }
                }
            });
            FakeHerdr {
                socket,
                requests,
                _dir: dir,
            }
        }

        /// Answers like herdr: `agent.list` returns `agents`, the rest `ok`.
        pub fn with_agents(agents: Value) -> Self {
            Self::start(move |req| {
                let id = req["id"].clone();
                Some(match req["method"].as_str() {
                    Some("agent.list") => {
                        json!({"id": id, "result": {"type": "agent_list", "agents": agents}})
                    }
                    _ => json!({"id": id, "result": {"type": "ok"}}),
                })
            })
        }

        pub fn env(&self) -> HerdrEnv {
            HerdrEnv {
                socket: self.socket.clone(),
                pane_id: Some("w1:p1".into()),
                workspace_id: Some("w1".into()),
                tab_id: Some("w1:t1".into()),
            }
        }
    }

    #[cfg(unix)]
    #[test]
    fn request_returns_the_result_of_the_matching_id() {
        let fake = FakeHerdr::start(|req| {
            // Lines for other ids (or not JSON) come first and are skipped.
            Some(json!([
                {"id": "other", "result": {"type": "ok"}},
                "not json",
                {"id": req["id"], "result": {"type": "pong"}},
            ]))
        });
        let client = HerdrClient::new(&fake.socket);
        let result = client.request("ping", json!({})).unwrap();
        assert_eq!(result["type"], "pong");
        let req = &fake.requests.lock().unwrap()[0];
        assert_eq!(req["method"], "ping");
        assert!(req["id"].as_str().unwrap().starts_with("kura-"));
    }

    #[cfg(unix)]
    #[test]
    fn request_reports_error_responses() {
        let fake = FakeHerdr::start(|req| {
            Some(
                json!({"id": req["id"], "error": {"code": "not_found", "message": "pane not found"}}),
            )
        });
        let err = HerdrClient::new(&fake.socket)
            .send_text("w9:p9", "@a ")
            .unwrap_err();
        assert_eq!(err, "herdr: pane not found");
    }

    #[cfg(unix)]
    #[test]
    fn request_times_out_when_herdr_does_not_answer() {
        let fake = FakeHerdr::start(|_| None);
        let start = std::time::Instant::now();
        let err = HerdrClient::with_timeout(&fake.socket, Duration::from_millis(200))
            .request("ping", json!({}))
            .unwrap_err();
        assert_eq!(err, "herdr did not respond");
        assert!(start.elapsed() < Duration::from_secs(2));
    }

    #[cfg(unix)]
    #[test]
    fn request_fails_without_a_socket() {
        let err = HerdrClient::new("/nonexistent/kura-test.sock")
            .request("ping", json!({}))
            .unwrap_err();
        assert!(err.starts_with("herdr: "), "{err}");
    }

    #[cfg(unix)]
    #[test]
    fn agent_list_and_send_text_use_the_herdr_methods() {
        let fake = FakeHerdr::with_agents(json!([
            {"pane_id": "w1:p2", "workspace_id": "w1", "tab_id": "w1:t1", "agent": "claude"}
        ]));
        let client = HerdrClient::new(&fake.socket);
        let agents = client.agent_list().unwrap();
        assert_eq!(agents[0].pane_id, "w1:p2");
        client.send_text("w1:p2", "@a ").unwrap();
        let reqs = fake.requests.lock().unwrap();
        assert_eq!(reqs[1]["method"], "pane.send_text");
        assert_eq!(
            reqs[1]["params"],
            json!({"pane_id": "w1:p2", "text": "@a "})
        );
    }
}
