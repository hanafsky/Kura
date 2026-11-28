//! Tests for the herdr plugin launcher (`herdr/open.sh`).
//!
//! The launcher is run with a stub `herdr` that records its arguments and a
//! stub `kura` on PATH, so no herdr server is needed.
#![cfg(unix)]

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

const LAUNCHER: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/herdr/open.sh");

struct Env {
    dir: tempfile::TempDir,
}

impl Env {
    /// A stub `herdr` that writes its arguments (one per line) to `args.txt`.
    /// With `with_kura`, a stub `kura` is also put on PATH.
    fn new(with_kura: bool) -> Env {
        let dir = tempfile::tempdir().unwrap();
        let bin = dir.path().join("bin");
        fs::create_dir(&bin).unwrap();
        let args = dir.path().join("args.txt");
        write_script(
            &bin.join("herdr"),
            &format!(
                "#!/bin/sh\nfor a in \"$@\"; do printf '%s\\n' \"$a\"; done >> '{}'\n",
                args.display()
            ),
        );
        if with_kura {
            write_script(&bin.join("kura"), "#!/bin/sh\n");
        }
        Env { dir }
    }

    fn path(&self) -> &Path {
        self.dir.path()
    }

    fn run(&self, entry: &str, context_json: &str) -> Output {
        self.run_with(entry, context_json, &[])
    }

    /// Like [`Env::run`], with extra environment variables.
    fn run_with(&self, entry: &str, context_json: &str, vars: &[(&str, &Path)]) -> Output {
        let bin = self.path().join("bin");
        let mut cmd = Command::new("sh");
        cmd.arg(LAUNCHER)
            .arg(entry)
            // Keep the system tools (sed) but not a real kura or herdr.
            .env("PATH", format!("{}:/usr/bin:/bin", bin.display()))
            .env("HERDR_BIN_PATH", bin.join("herdr"))
            .env("HERDR_PLUGIN_CONTEXT_JSON", context_json)
            .env("HOME", self.path())
            .env_remove("HERDR_PANE_ID")
            .env_remove("HERDR_PLUGIN_CONFIG_DIR");
        for (k, v) in vars {
            cmd.env(k, v);
        }
        cmd.output().unwrap()
    }

    /// The arguments of every stub `herdr` call, in order.
    fn herdr_args(&self) -> Vec<String> {
        fs::read_to_string(self.path().join("args.txt"))
            .unwrap_or_default()
            .lines()
            .map(String::from)
            .collect()
    }
}

fn write_script(path: &Path, body: &str) {
    fs::write(path, body).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
}

/// `s` as a JSON string literal (only `"` and `\` need escaping here).
fn json_str(s: &str) -> String {
    format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\""))
}

fn context(focused: &Path, workspace: &Path) -> String {
    format!(
        "{{\"workspace_id\":\"w1\",\"workspace_cwd\":{},\"tab_id\":\"w1:t1\",\
         \"focused_pane_id\":\"w1:p1\",\"focused_pane_cwd\":{},\"invocation_source\":\"keybinding\"}}",
        json_str(&workspace.display().to_string()),
        json_str(&focused.display().to_string()),
    )
}

fn pane_open(entry: &str, cwd: &Path, extra: &[&str]) -> Vec<String> {
    let mut args: Vec<String> = [
        "plugin",
        "pane",
        "open",
        "--plugin",
        "kura",
        "--entrypoint",
        entry,
        "--cwd",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect();
    args.push(cwd.display().to_string());
    args.extend(extra.iter().map(|s| s.to_string()));
    args
}

fn mkdir(env: &Env, name: &str) -> PathBuf {
    let dir = env.path().join(name);
    fs::create_dir_all(&dir).unwrap();
    dir
}

#[test]
fn split_opens_in_the_focused_pane_directory() {
    let env = Env::new(true);
    let focused = mkdir(&env, "repo/sub dir");
    let workspace = mkdir(&env, "repo");
    let out = env.run("split", &context(&focused, &workspace));
    assert!(out.status.success(), "{out:?}");
    assert_eq!(
        env.herdr_args(),
        pane_open("split", &focused, &["--direction", "right", "--focus"])
    );
}

#[test]
fn popup_opens_the_popup_entrypoint() {
    let env = Env::new(true);
    let focused = mkdir(&env, "repo");
    let out = env.run("popup", &context(&focused, &focused));
    assert!(out.status.success(), "{out:?}");
    assert_eq!(env.herdr_args(), pane_open("popup", &focused, &[]));
}

#[test]
fn escaped_characters_in_the_cwd_are_restored() {
    let env = Env::new(true);
    let focused = mkdir(&env, "a \"quoted\" back\\slash");
    let out = env.run("popup", &context(&focused, env.path()));
    assert!(out.status.success(), "{out:?}");
    assert_eq!(env.herdr_args(), pane_open("popup", &focused, &[]));
}

#[test]
fn missing_focused_directory_falls_back_to_the_workspace() {
    let env = Env::new(true);
    let workspace = mkdir(&env, "repo");
    let out = env.run("popup", &context(&env.path().join("gone"), &workspace));
    assert!(out.status.success(), "{out:?}");
    assert_eq!(env.herdr_args(), pane_open("popup", &workspace, &[]));
}

#[test]
fn no_context_falls_back_to_home() {
    let env = Env::new(true);
    let out = env.run("popup", "");
    assert!(out.status.success(), "{out:?}");
    assert_eq!(env.herdr_args(), pane_open("popup", env.path(), &[]));
}

#[test]
fn missing_kura_binary_is_reported_without_opening_a_pane() {
    let env = Env::new(false);
    let out = env.run("split", &context(env.path(), env.path()));
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("kura is not on"));
    let args = env.herdr_args();
    assert_eq!(&args[..2], ["notification", "show"], "{args:?}");
    assert!(!args.contains(&"pane".to_string()), "{args:?}");
}

#[test]
fn unknown_entry_is_rejected() {
    let env = Env::new(true);
    let out = env.run("tab", &context(env.path(), env.path()));
    assert!(!out.status.success());
    assert!(!env.herdr_args().contains(&"pane".to_string()));
}

/// The plugin config directory, with the opt-in file when `opted_in`.
fn config_dir(env: &Env, opted_in: bool) -> PathBuf {
    let dir = mkdir(env, "config");
    if opted_in {
        fs::write(dir.join("open-on-worktree-created"), "").unwrap();
    }
    dir
}

#[test]
fn worktree_hook_does_nothing_without_the_opt_in_file() {
    // Not even a "kura is not on PATH" notification.
    let env = Env::new(false);
    let wt = mkdir(&env, "wt");
    let config = config_dir(&env, false);
    let out = env.run_with(
        "worktree",
        &context(&wt, &wt),
        &[("HERDR_PLUGIN_CONFIG_DIR", &config)],
    );
    assert!(out.status.success(), "{out:?}");
    assert!(out.stderr.is_empty(), "{out:?}");
    assert!(env.herdr_args().is_empty());
    // Without a config directory either.
    let out = env.run("worktree", &context(&wt, &wt));
    assert!(out.status.success(), "{out:?}");
    assert!(env.herdr_args().is_empty());
}

#[test]
fn worktree_hook_splits_beside_the_event_pane_without_focus() {
    let env = Env::new(true);
    let wt = mkdir(&env, "wt");
    let config = config_dir(&env, true);
    let out = env.run_with(
        "worktree",
        &context(&wt, &wt),
        &[
            ("HERDR_PLUGIN_CONFIG_DIR", &config),
            ("HERDR_PANE_ID", Path::new("w2:p1")),
        ],
    );
    assert!(out.status.success(), "{out:?}");
    assert_eq!(
        env.herdr_args(),
        pane_open(
            "split",
            &wt,
            &[
                "--target-pane",
                "w2:p1",
                "--direction",
                "right",
                "--no-focus"
            ]
        )
    );
}

#[test]
fn worktree_hook_falls_back_to_the_focused_pane_id() {
    let env = Env::new(true);
    let wt = mkdir(&env, "wt");
    let config = config_dir(&env, true);
    let out = env.run_with(
        "worktree",
        &context(&wt, &wt),
        &[("HERDR_PLUGIN_CONFIG_DIR", &config)],
    );
    assert!(out.status.success(), "{out:?}");
    assert_eq!(
        env.herdr_args(),
        pane_open(
            "split",
            &wt,
            &[
                "--target-pane",
                "w1:p1",
                "--direction",
                "right",
                "--no-focus"
            ]
        )
    );
}

#[test]
fn manifest_declares_the_launcher_actions_and_panes() {
    let manifest =
        fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/herdr-plugin.toml")).unwrap();
    for line in [
        "id = \"kura\"",
        "command = [\"sh\", \"herdr/open.sh\", \"split\"]",
        "command = [\"sh\", \"herdr/open.sh\", \"popup\"]",
        "on = \"worktree.created\"",
        "command = [\"sh\", \"herdr/open.sh\", \"worktree\"]",
        "placement = \"split\"",
        "placement = \"popup\"",
    ] {
        assert!(manifest.contains(line), "missing: {line}");
    }
    // The plugin version follows the crate version.
    assert!(manifest.contains(&format!("version = \"{}\"", env!("CARGO_PKG_VERSION"))));
}
