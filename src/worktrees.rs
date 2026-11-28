//! Git worktrees of the active pane's repository (`W`), listed in the
//! active pane; `Enter` opens the chosen one there.
//!
//! The list comes from `git worktree list --porcelain` everywhere. Inside
//! an agent terminal each worktree also shows the agents working in it,
//! matched by their working directory:
//!
//! - herdr: `agent.list` over the socket ([`HerdrClient`]), from every
//!   workspace (herdr opens each worktree in its own workspace).
//! - herdr-core (`hc`): `hc ls --json`; terminals that report an agent
//!   `status` count as agents, plain shells do not.
//!
//! Parsing and matching are plain functions here.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use serde_json::Value;

use crate::ade::{Ade, HerdrClient};
use crate::app::{fs_error_message, App};
use crate::mode::Mode;
use crate::watch;

/// How long `hc ls` may take before kura gives up on it.
const HC_TIMEOUT: Duration = Duration::from_secs(1);

/// One worktree from `git worktree list --porcelain`.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Worktree {
    pub path: PathBuf,
    /// Commit checked out (full hash).
    pub head: Option<String>,
    /// Branch name without `refs/heads/`; `None` when detached or bare.
    pub branch: Option<String>,
    pub detached: bool,
    pub bare: bool,
    pub locked: bool,
    pub prunable: bool,
    /// Agents whose working directory lies in this worktree (labels).
    pub agents: Vec<String>,
}

impl Worktree {
    /// What is checked out: the branch, `(detached abc1234)` or `(bare)`.
    pub fn checkout(&self) -> String {
        if self.bare {
            return "(bare)".into();
        }
        match (&self.branch, &self.head) {
            (Some(b), _) => b.clone(),
            (None, Some(h)) => format!("(detached {})", &h[..h.len().min(7)]),
            (None, None) => "(detached)".into(),
        }
    }
}

/// Parse `git worktree list --porcelain`: one block of `key value` lines
/// per worktree, blocks separated by blank lines.
pub fn parse_porcelain(out: &str) -> Vec<Worktree> {
    let mut list = Vec::new();
    let mut cur: Option<Worktree> = None;
    for line in out.lines().map(|l| l.trim_end_matches('\r')) {
        let (key, value) = line.split_once(' ').unwrap_or((line, ""));
        if key == "worktree" {
            list.extend(cur.take());
            cur = Some(Worktree {
                path: PathBuf::from(value),
                ..Worktree::default()
            });
            continue;
        }
        let Some(wt) = cur.as_mut() else {
            continue;
        };
        match key {
            "" => list.extend(cur.take()),
            "HEAD" => wt.head = Some(value.to_string()),
            "branch" => wt.branch = Some(value.strip_prefix("refs/heads/").unwrap_or(value).into()),
            "detached" => wt.detached = true,
            "bare" => wt.bare = true,
            "locked" => wt.locked = true,
            "prunable" => wt.prunable = true,
            _ => {}
        }
    }
    list.extend(cur);
    list
}

/// The worktree that contains `path`: the deepest one, since worktrees can
/// sit inside the main one (e.g. Claude Code's `.claude/worktrees/`). Both
/// sides are compared in every spelling ([`watch::dir_keys`]), so
/// `/tmp` and `/private/tmp` match on macOS.
pub fn containing(roots: &[PathBuf], path: &Path) -> Option<usize> {
    let keys = watch::dir_keys(path);
    roots
        .iter()
        .enumerate()
        .filter_map(|(i, root)| {
            watch::dir_keys(root)
                .iter()
                .filter(|r| keys.iter().any(|k| k.starts_with(r)))
                .map(|r| r.components().count())
                .max()
                .map(|depth| (i, depth))
        })
        .max_by_key(|&(_, depth)| depth)
        .map(|(i, _)| i)
}

/// An agent and where it works.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentCwd {
    pub label: String,
    pub cwd: PathBuf,
}

/// Put each agent on the worktree it works in.
pub fn assign_agents(worktrees: &mut [Worktree], agents: &[AgentCwd]) {
    let roots: Vec<PathBuf> = worktrees.iter().map(|w| w.path.clone()).collect();
    for agent in agents {
        if let Some(i) = containing(&roots, &agent.cwd) {
            worktrees[i].agents.push(agent.label.clone());
        }
    }
}

/// Agents in `hc ls --json` output: terminals with an agent `status`
/// (shells have none), except kura's own terminal.
pub fn hc_agents(ls: &Value, own_terminal: &str) -> Vec<AgentCwd> {
    let Some(terminals) = ls.get("terminals").and_then(Value::as_array) else {
        return Vec::new();
    };
    terminals
        .iter()
        .filter(|t| t.get("status").is_some())
        .filter_map(|t| {
            let s = |k: &str| t.get(k).and_then(Value::as_str);
            let address = s("address")?;
            if address == own_terminal {
                return None;
            }
            let kind = s("agent").unwrap_or("agent");
            let label = match s("name") {
                Some(name) if name != kind => format!("{kind} \"{name}\" ({address})"),
                _ => format!("{kind} ({address})"),
            };
            Some(AgentCwd {
                label,
                cwd: PathBuf::from(s("cwd")?),
            })
        })
        .collect()
}

/// The agents of the terminal kura runs in, with their working directory.
fn ade_agents(ade: &Ade) -> Result<Vec<AgentCwd>, String> {
    match ade {
        Ade::None => Ok(Vec::new()),
        Ade::Herdr(env) => Ok(HerdrClient::new(&env.socket)
            .agent_list()?
            .into_iter()
            .filter(|a| env.pane_id.as_deref() != Some(a.pane_id.as_str()))
            .filter_map(|a| {
                Some(AgentCwd {
                    label: a.label(),
                    cwd: a.cwd?,
                })
            })
            .collect()),
        Ade::HerdrCore { terminal_id } => {
            let out = run_with_timeout(Command::new("hc").args(["ls", "--json"]), HC_TIMEOUT)?;
            let ls: Value =
                serde_json::from_str(&out).map_err(|e| format!("hc ls: unexpected output: {e}"))?;
            Ok(hc_agents(&ls, terminal_id))
        }
    }
}

/// Run a command and return its stdout, giving up after `timeout`.
fn run_with_timeout(cmd: &mut Command, timeout: Duration) -> Result<String, String> {
    let name = cmd.get_program().to_string_lossy().into_owned();
    let mut child = cmd
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| format!("{name}: {e}"))?;
    // Read in a thread so a full pipe cannot block the child.
    let mut stdout = child.stdout.take().expect("piped");
    let reader = std::thread::spawn(move || {
        let mut s = String::new();
        stdout.read_to_string(&mut s).map(|_| s)
    });
    let deadline = Instant::now() + timeout;
    loop {
        match child.try_wait() {
            Ok(Some(status)) if status.success() => break,
            Ok(Some(status)) => return Err(format!("{name} failed ({status})")),
            Ok(None) if Instant::now() >= deadline => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!("{name} did not respond"));
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(10)),
            Err(e) => return Err(format!("{name}: {e}")),
        }
    }
    match reader.join() {
        Ok(Ok(s)) => Ok(s),
        Ok(Err(e)) => Err(format!("{name}: {e}")),
        Err(_) => Err(format!("{name}: output could not be read")),
    }
}

/// `git worktree list --porcelain` in `dir`.
fn git_worktrees(dir: &Path) -> Result<Vec<Worktree>, String> {
    let out = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(["worktree", "list", "--porcelain"])
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .map_err(|e| format!("git: {e}"))?;
    if !out.status.success() {
        return Err("Not in a git repository".into());
    }
    Ok(parse_porcelain(&String::from_utf8_lossy(&out.stdout)))
}

/// `path` with the home directory shown as `~`.
pub fn display_path(path: &Path, home: Option<&Path>) -> String {
    match home.and_then(|h| path.strip_prefix(h).ok()) {
        Some(rest) if rest.as_os_str().is_empty() => "~".into(),
        Some(rest) => format!("~/{}", rest.display()),
        None => path.display().to_string(),
    }
}

impl App {
    /// Read the worktrees (and their agents) of the active pane's
    /// repository. `None` (after reporting why) if there are none.
    fn read_worktrees(&mut self) -> Option<Vec<Worktree>> {
        let dir = self.current_pane().current_dir.clone();
        let mut list = match git_worktrees(&dir) {
            Ok(list) if !list.is_empty() => list,
            Ok(_) => {
                self.notify_error("No worktrees found");
                return None;
            }
            Err(e) => {
                self.notify_error(e);
                return None;
            }
        };
        match ade_agents(&self.ade) {
            Ok(agents) => assign_agents(&mut list, &agents),
            // The list is still useful without the agents.
            Err(e) => self.notify_error(format!("Cannot list agents: {e}")),
        }
        Some(list)
    }

    /// The listed worktree that holds the active pane's directory.
    pub fn current_worktree(&self, items: &[Worktree]) -> Option<usize> {
        let roots: Vec<PathBuf> = items.iter().map(|w| w.path.clone()).collect();
        containing(&roots, &self.current_pane().current_dir)
    }

    /// `W`: list the worktrees in the active pane, the cursor on the one
    /// the pane is in.
    pub fn open_worktrees(&mut self) {
        let Some(items) = self.read_worktrees() else {
            return;
        };
        let selected = self.current_worktree(&items).unwrap_or(0);
        self.mode = Mode::Worktrees { items, selected };
    }

    /// `R` in the list: read it again, keeping the cursor on the same
    /// worktree (or row).
    pub fn reload_worktrees(&mut self) {
        let Some(new_items) = self.read_worktrees() else {
            return;
        };
        if let Mode::Worktrees { items, selected } = &mut self.mode {
            let cursor = items.get(*selected).map(|w| w.path.clone());
            let found = cursor.and_then(|c| new_items.iter().position(|w| w.path == c));
            *selected = found
                .unwrap_or(*selected)
                .min(new_items.len().saturating_sub(1));
            *items = new_items;
        }
    }

    /// Move the cursor of the list `n` rows down / up (clamped).
    pub fn move_worktree_cursor(&mut self, n: usize, down: bool) {
        if let Mode::Worktrees { items, selected } = &mut self.mode {
            let to = if down {
                selected.saturating_add(n)
            } else {
                selected.saturating_sub(n)
            };
            *selected = to.min(items.len().saturating_sub(1));
        }
    }

    /// `Enter`: close the list and show the chosen worktree in the active
    /// pane.
    pub fn open_selected_worktree(&mut self) {
        let Mode::Worktrees { items, selected } = &self.mode else {
            return;
        };
        let Some(path) = items.get(*selected).map(|w| w.path.clone()) else {
            return;
        };
        if let Err(e) = self.current_pane_mut().change_dir(path.clone()) {
            self.notify_error(fs_error_message(&e, &path));
            return;
        }
        self.mode = Mode::Filer;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::command::Command as Cmd;
    use crate::mode::PaneType;
    use ratatui_image::picker::Picker;
    use serde_json::json;
    use std::fs;

    const PORCELAIN: &str = "\
worktree /repo
HEAD 1111111111111111111111111111111111111111
branch refs/heads/main

worktree /repo/.claude/worktrees/fix
HEAD 2222222222222222222222222222222222222222
branch refs/heads/fix
locked reason here

worktree /wt/detached
HEAD 3333333333333333333333333333333333333333
detached
prunable gitdir file points to non-existent location

";

    #[test]
    fn parses_porcelain_blocks() {
        let list = parse_porcelain(PORCELAIN);
        assert_eq!(list.len(), 3);
        assert_eq!(list[0].path, PathBuf::from("/repo"));
        assert_eq!(list[0].branch.as_deref(), Some("main"));
        assert_eq!(list[0].checkout(), "main");
        assert!(list[1].locked && !list[1].prunable);
        assert!(list[2].detached && list[2].prunable);
        assert_eq!(list[2].checkout(), "(detached 3333333)");
    }

    #[test]
    fn parses_bare_and_missing_final_blank_line() {
        let list = parse_porcelain(
            "worktree /r.git\nbare\n\nworktree /r/a\r\nHEAD abc\r\nbranch refs/heads/a",
        );
        assert_eq!(list.len(), 2);
        assert!(list[0].bare);
        assert_eq!(list[0].checkout(), "(bare)");
        assert_eq!(list[1].path, PathBuf::from("/r/a"));
        assert_eq!(list[1].checkout(), "a");
        assert!(parse_porcelain("").is_empty());
    }

    #[test]
    fn containing_prefers_the_deepest_worktree() {
        let roots = [
            PathBuf::from("/repo"),
            PathBuf::from("/repo/.claude/worktrees/fix"),
            PathBuf::from("/wt/b"),
        ];
        let at = |p: &str| containing(&roots, Path::new(p));
        assert_eq!(at("/repo/src"), Some(0));
        assert_eq!(at("/repo/.claude/worktrees/fix/src"), Some(1));
        assert_eq!(at("/repo/.claude/worktrees/fix"), Some(1));
        assert_eq!(at("/wt/b"), Some(2));
        // Whole components only.
        assert_eq!(at("/repository"), None);
        assert_eq!(at("/elsewhere"), None);
    }

    #[test]
    fn containing_matches_other_spellings_of_a_path() {
        let tmp = tempfile::tempdir().unwrap();
        let real = tmp.path().join("real");
        fs::create_dir(&real).unwrap();
        let roots = [real.clone()];
        #[cfg(unix)]
        {
            let link = tmp.path().join("link");
            std::os::unix::fs::symlink(&real, &link).unwrap();
            assert_eq!(containing(&roots, &link), Some(0));
            assert_eq!(containing(&[link], &real), Some(0));
        }
        assert_eq!(containing(&roots, &real), Some(0));
    }

    #[test]
    fn agents_go_to_their_worktree() {
        let mut list = parse_porcelain(PORCELAIN);
        let agent = |label: &str, cwd: &str| AgentCwd {
            label: label.into(),
            cwd: cwd.into(),
        };
        assign_agents(
            &mut list,
            &[
                agent("claude (w1:p2)", "/repo/src"),
                agent("codex (w2:p1)", "/repo/.claude/worktrees/fix"),
                agent("claude (w3:p1)", "/somewhere/else"),
            ],
        );
        assert_eq!(list[0].agents, ["claude (w1:p2)"]);
        assert_eq!(list[1].agents, ["codex (w2:p1)"]);
        assert!(list[2].agents.is_empty());
    }

    #[test]
    fn hc_agents_skip_shells_and_own_terminal() {
        // Shape of real `hc ls --json` output.
        let ls = json!({"hidden_finished": 8, "terminals": [
            {"address": "t2", "agent": "zsh", "cwd": "/repo", "state": "live", "turn": 30, "where": "repo / 1"},
            {"address": "t15", "agent": "claude-code", "cwd": "/repo", "state": "live", "status": "done", "turn": 20, "where": "repo / 1"},
            {"address": "t2j", "agent": "claude-code", "cwd": "/wt/b", "name": "w-b", "parent": "t1m", "state": "live", "status": "working", "turn": 0, "where": "kura / 1"},
            {"address": "t9", "agent": "claude-code", "cwd": "/repo", "status": "idle"},
            {"address": "t3", "agent": "claude-code", "status": "idle"}
        ]});
        assert_eq!(
            hc_agents(&ls, "t9"),
            [
                AgentCwd {
                    label: "claude-code (t15)".into(),
                    cwd: "/repo".into()
                },
                AgentCwd {
                    label: "claude-code \"w-b\" (t2j)".into(),
                    cwd: "/wt/b".into()
                },
            ]
        );
        assert!(hc_agents(&json!({}), "t9").is_empty());
    }

    #[test]
    fn display_path_shortens_home() {
        let home = Path::new("/home/u");
        assert_eq!(
            display_path(Path::new("/home/u/wt/a"), Some(home)),
            "~/wt/a"
        );
        assert_eq!(display_path(Path::new("/home/u"), Some(home)), "~");
        assert_eq!(display_path(Path::new("/srv/a"), Some(home)), "/srv/a");
        assert_eq!(display_path(Path::new("/home/u2"), Some(home)), "/home/u2");
    }

    fn git(dir: &Path, args: &[&str]) {
        let ok = Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(args)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .unwrap()
            .success();
        assert!(ok, "git {args:?}");
    }

    /// A repository with one commit and a linked worktree `wt-a`; returns
    /// (temp dir, repo, worktree), canonicalized.
    fn repo_with_worktree() -> (tempfile::TempDir, PathBuf, PathBuf) {
        let tmp = tempfile::tempdir().unwrap();
        let base = tmp.path().canonicalize().unwrap();
        let repo = base.join("repo");
        fs::create_dir(&repo).unwrap();
        git(&repo, &["init", "-q", "-b", "main"]);
        git(
            &repo,
            &[
                "-c",
                "user.name=t",
                "-c",
                "user.email=t@t",
                "commit",
                "-q",
                "--allow-empty",
                "-m",
                "init",
            ],
        );
        let wt = base.join("wt-a");
        git(
            &repo,
            &[
                "worktree",
                "add",
                "-q",
                "-b",
                "feat-a",
                wt.to_str().unwrap(),
            ],
        );
        fs::write(wt.join("in-wt.txt"), "x").unwrap();
        (tmp, repo, wt)
    }

    #[test]
    fn w_lists_worktrees_and_enter_opens_one_in_the_active_pane() {
        let (_tmp, repo, wt) = repo_with_worktree();
        let mut app = App::with_dir(Picker::halfblocks(), repo.clone()).unwrap();
        app.active = PaneType::Right;
        let _ = app.apply(Cmd::ToggleWorktrees);
        let Mode::Worktrees { items, selected } = &app.mode else {
            panic!("not listing worktrees");
        };
        let paths: Vec<_> = items.iter().map(|w| w.path.clone()).collect();
        assert_eq!(paths, [repo.clone(), wt.clone()]);
        assert_eq!(items[1].branch.as_deref(), Some("feat-a"));
        // The cursor starts on the worktree the pane is in.
        assert_eq!(*selected, 0);
        let _ = app.apply(Cmd::MoveDown(5));
        assert!(matches!(app.mode, Mode::Worktrees { selected: 1, .. }));
        let _ = app.apply(Cmd::Open);
        assert!(matches!(app.mode, Mode::Filer));
        assert_eq!(app.right.current_dir, wt);
        assert_eq!(app.left.current_dir, repo);
        // From inside the worktree the cursor starts on it.
        let _ = app.apply(Cmd::ToggleWorktrees);
        assert!(matches!(app.mode, Mode::Worktrees { selected: 1, .. }));
        let _ = app.apply(Cmd::MoveUp(1));
        let _ = app.apply(Cmd::Refresh);
        assert!(matches!(app.mode, Mode::Worktrees { selected: 0, .. }));
        // `W` again, Esc and q close the list.
        let _ = app.apply(Cmd::ToggleWorktrees);
        assert!(matches!(app.mode, Mode::Filer));
        let _ = app.apply(Cmd::ToggleWorktrees);
        let _ = app.apply(Cmd::Back);
        assert!(matches!(app.mode, Mode::Filer));
        assert_eq!(app.right.current_dir, wt);
    }

    #[test]
    fn w_outside_a_repository_reports_it() {
        let tmp = tempfile::tempdir().unwrap();
        let mut app = App::with_dir(Picker::halfblocks(), tmp.path().to_path_buf()).unwrap();
        let _ = app.apply(Cmd::ToggleWorktrees);
        assert!(matches!(app.mode, Mode::Filer));
        let msg = app.message.as_ref().unwrap();
        assert_eq!(msg.level, crate::app::MessageLevel::Error);
        assert_eq!(msg.text, "Not in a git repository");
    }

    #[test]
    fn enter_on_a_removed_worktree_keeps_the_list() {
        let (_tmp, repo, wt) = repo_with_worktree();
        let mut app = App::with_dir(Picker::halfblocks(), repo.clone()).unwrap();
        let _ = app.apply(Cmd::ToggleWorktrees);
        fs::remove_dir_all(&wt).unwrap();
        let _ = app.apply(Cmd::MoveDown(1));
        let _ = app.apply(Cmd::Open);
        assert!(matches!(app.mode, Mode::Worktrees { .. }));
        assert_eq!(app.left.current_dir, repo);
        assert_eq!(
            app.message.as_ref().unwrap().level,
            crate::app::MessageLevel::Error
        );
    }

    #[cfg(unix)]
    #[test]
    fn herdr_agents_from_every_workspace_are_matched() {
        use crate::ade::tests::FakeHerdr;
        let (_tmp, repo, wt) = repo_with_worktree();
        let fake = FakeHerdr::with_agents(json!([
            // kura's own pane is never listed.
            {"pane_id": "w1:p1", "workspace_id": "w1", "tab_id": "w1:t1", "agent": "claude", "cwd": repo},
            {"pane_id": "w1:p2", "workspace_id": "w1", "tab_id": "w1:t1", "agent": "claude", "cwd": repo.join("src")},
            {"pane_id": "w2:p1", "workspace_id": "w2", "tab_id": "w2:t1", "agent": "codex", "name": "fixer", "cwd": wt},
        ]));
        let mut app = App::with_dir(Picker::halfblocks(), repo.clone()).unwrap();
        app.ade = Ade::Herdr(fake.env());
        let _ = app.apply(Cmd::ToggleWorktrees);
        let Mode::Worktrees { items, .. } = &app.mode else {
            panic!("not listing worktrees");
        };
        assert_eq!(items[0].agents, ["claude (w1:p2)"]);
        assert_eq!(items[1].agents, ["codex \"fixer\" (w2:p1)"]);
        assert_eq!(fake.requests.lock().unwrap()[0]["method"], "agent.list");
    }

    #[cfg(unix)]
    #[test]
    fn worktrees_are_listed_when_herdr_fails() {
        let (_tmp, repo, _wt) = repo_with_worktree();
        let mut app = App::with_dir(Picker::halfblocks(), repo.clone()).unwrap();
        app.ade = Ade::Herdr(crate::ade::HerdrEnv {
            socket: PathBuf::from("/nonexistent/kura-test.sock"),
            pane_id: None,
            workspace_id: None,
            tab_id: None,
        });
        let _ = app.apply(Cmd::ToggleWorktrees);
        assert!(matches!(&app.mode, Mode::Worktrees { items, .. } if items.len() == 2));
        assert!(app
            .message
            .as_ref()
            .unwrap()
            .text
            .starts_with("Cannot list agents: herdr: "));
    }

    #[cfg(unix)]
    #[test]
    fn run_with_timeout_gives_up_on_a_stuck_command() {
        let start = Instant::now();
        let err = run_with_timeout(
            Command::new("sh").args(["-c", "sleep 5"]),
            Duration::from_millis(200),
        )
        .unwrap_err();
        assert_eq!(err, "sh did not respond");
        assert!(start.elapsed() < Duration::from_secs(2));
        assert_eq!(
            run_with_timeout(Command::new("sh").args(["-c", "echo hi"]), HC_TIMEOUT).unwrap(),
            "hi\n"
        );
        assert!(run_with_timeout(Command::new("sh").args(["-c", "exit 3"]), HC_TIMEOUT).is_err());
    }
}
