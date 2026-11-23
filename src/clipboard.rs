//! Copy `@path` references (for Claude Code and other agents) to the system
//! clipboard.
//!
//! Paths are made relative to the directory kura was started in (assumed to
//! be the project root the agent runs in); paths outside it stay absolute.
//! A path containing whitespace is written as `@"path"`, the quoted form the
//! Claude Code CLI accepts. Several paths are separated by a single space.
//!
//! The text is sent with OSC 52, so it also reaches the outer terminal
//! through tmux / herdr / ssh. Whether the terminal accepted OSC 52 cannot be
//! detected, so outside ssh the native tools (pbcopy, wl-copy, xclip,
//! clip.exe) are written as well. Environment variables:
//!
//! - `KURA_CLIPBOARD`: `auto` (default: OSC 52 and native; OSC 52 only over
//!   ssh), `osc52`, `native`
//! - `KURA_AT_PATH`: `relative` (default) or `absolute`

use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use crate::actions::target_paths;
use crate::app::App;

pub const CLIPBOARD_ENV: &str = "KURA_CLIPBOARD";
pub const PATH_STYLE_ENV: &str = "KURA_AT_PATH";

/// Whether `@path` is relative to the launch directory or absolute.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PathStyle {
    Relative,
    Absolute,
}

impl PathStyle {
    /// Parse `KURA_AT_PATH`. Anything other than `absolute` means relative.
    fn from_env_value(value: Option<&str>) -> Self {
        match value.map(|v| v.trim().to_ascii_lowercase()).as_deref() {
            Some("absolute" | "abs") => PathStyle::Absolute,
            _ => PathStyle::Relative,
        }
    }
}

/// A way to put text on the clipboard.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Method {
    Osc52,
    Pbcopy,
    WlCopy,
    Xclip,
    ClipExe,
}

impl Method {
    fn name(self) -> &'static str {
        match self {
            Method::Osc52 => "OSC 52",
            Method::Pbcopy => "pbcopy",
            Method::WlCopy => "wl-copy",
            Method::Xclip => "xclip",
            Method::ClipExe => "clip.exe",
        }
    }
}

/// The parts of the environment that decide the clipboard method.
#[derive(Debug, Default, Clone)]
pub struct ClipEnv {
    /// Value of `KURA_CLIPBOARD`.
    pub mode: Option<String>,
    pub macos: bool,
    pub wsl: bool,
    pub wayland: bool,
    pub x11: bool,
    /// Running over ssh (see [`is_ssh`]).
    pub ssh: bool,
}

/// Over ssh when any of `SSH_CONNECTION` / `SSH_TTY` / `SSH_CLIENT` is set.
pub fn is_ssh(connection: Option<&str>, tty: Option<&str>, client: Option<&str>) -> bool {
    [connection, tty, client].iter().any(Option::is_some)
}

impl ClipEnv {
    fn current() -> Self {
        let distro = std::env::var("WSL_DISTRO_NAME").ok();
        let osrelease = fs::read_to_string("/proc/sys/kernel/osrelease").ok();
        let non_empty = |k: &str| std::env::var(k).is_ok_and(|v| !v.is_empty());
        ClipEnv {
            mode: std::env::var(CLIPBOARD_ENV).ok(),
            macos: cfg!(target_os = "macos"),
            wsl: crate::media::is_wsl(distro.as_deref(), osrelease.as_deref()),
            wayland: non_empty("WAYLAND_DISPLAY"),
            x11: non_empty("DISPLAY"),
            ssh: is_ssh(
                std::env::var("SSH_CONNECTION").ok().as_deref(),
                std::env::var("SSH_TTY").ok().as_deref(),
                std::env::var("SSH_CLIENT").ok().as_deref(),
            ),
        }
    }
}

/// Native clipboard tools for this environment, in the order to try them.
fn native_methods(env: &ClipEnv) -> Vec<Method> {
    if env.macos {
        return vec![Method::Pbcopy];
    }
    if env.wsl {
        return vec![Method::ClipExe];
    }
    let mut methods = Vec::new();
    if env.wayland {
        methods.push(Method::WlCopy);
    }
    if env.x11 {
        methods.push(Method::Xclip);
    }
    methods
}

/// How to write the clipboard: OSC 52 and/or native tools.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Plan {
    /// Emit the OSC 52 sequence.
    pub osc52: bool,
    /// Native tools to try in order; the first that succeeds is used.
    pub native: Vec<Method>,
}

/// Decide how to write the clipboard. Unknown `KURA_CLIPBOARD` values are
/// treated as `auto`: OSC 52 plus native tools locally, OSC 52 only over ssh
/// (a native tool there would write the remote host's clipboard).
pub fn choose_plan(env: &ClipEnv) -> Plan {
    let mode = env.mode.as_deref().map(|m| m.trim().to_ascii_lowercase());
    match mode.as_deref() {
        Some("osc52") => Plan {
            osc52: true,
            native: Vec::new(),
        },
        Some("native") => Plan {
            osc52: false,
            native: native_methods(env),
        },
        _ if env.ssh => Plan {
            osc52: true,
            native: Vec::new(),
        },
        _ => Plan {
            osc52: true,
            native: native_methods(env),
        },
    }
}

/// Format one path as an `@` reference.
///
/// With [`PathStyle::Relative`], a path under `base` becomes relative to it
/// (`base` itself becomes `.`); a path outside `base` stays absolute.
/// A path containing whitespace is quoted as `@"path"`.
pub fn format_at_path(path: &Path, base: &Path, style: PathStyle) -> String {
    let shown: PathBuf = match style {
        PathStyle::Relative => match path.strip_prefix(base) {
            Ok(rel) if rel.as_os_str().is_empty() => PathBuf::from("."),
            Ok(rel) => rel.to_path_buf(),
            Err(_) => path.to_path_buf(),
        },
        PathStyle::Absolute => path.to_path_buf(),
    };
    let s = shown.to_string_lossy();
    if s.chars().any(char::is_whitespace) {
        format!("@\"{s}\"")
    } else {
        format!("@{s}")
    }
}

/// Format several paths as space-separated `@` references.
pub fn format_at_paths(paths: &[PathBuf], base: &Path, style: PathStyle) -> String {
    paths
        .iter()
        .map(|p| format_at_path(p, base, style))
        .collect::<Vec<_>>()
        .join(" ")
}

/// Standard base64 (RFC 4648) with `=` padding.
pub fn base64_encode(data: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for chunk in data.chunks(3) {
        let b = [
            chunk[0],
            chunk.get(1).copied().unwrap_or(0),
            chunk.get(2).copied().unwrap_or(0),
        ];
        let n = (u32::from(b[0]) << 16) | (u32::from(b[1]) << 8) | u32::from(b[2]);
        for i in 0..4 {
            if i <= chunk.len() {
                out.push(TABLE[((n >> (18 - 6 * i)) & 0x3f) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

/// OSC 52 sequence that sets the system clipboard (`c`) to `text`.
/// Terminated with ST (`ESC \`).
pub fn osc52_sequence(text: &str) -> String {
    format!("\x1b]52;c;{}\x1b\\", base64_encode(text.as_bytes()))
}

/// Program and arguments for a native clipboard tool.
fn native_command(method: Method) -> Option<(&'static str, &'static [&'static str])> {
    match method {
        Method::Osc52 => None,
        Method::Pbcopy => Some(("pbcopy", &[])),
        Method::WlCopy => Some(("wl-copy", &[])),
        Method::Xclip => Some(("xclip", &["-selection", "clipboard"])),
        Method::ClipExe => Some(("clip.exe", &[])),
    }
}

/// Bytes to feed to a native tool on stdin. clip.exe reads UTF-8 as the
/// ANSI code page, so it gets UTF-16LE with a BOM instead.
fn native_payload(method: Method, text: &str) -> Vec<u8> {
    match method {
        Method::ClipExe => {
            let mut bytes = vec![0xFF, 0xFE];
            bytes.extend(text.encode_utf16().flat_map(u16::to_le_bytes));
            bytes
        }
        _ => text.as_bytes().to_vec(),
    }
}

#[cfg_attr(test, allow(dead_code))]
fn write_osc52(text: &str) -> io::Result<()> {
    let mut out = io::stdout();
    out.write_all(osc52_sequence(text).as_bytes())?;
    out.flush()
}

#[cfg_attr(test, allow(dead_code))]
fn run_native(method: Method, text: &str) -> io::Result<()> {
    let Some((program, args)) = native_command(method) else {
        return Err(io::Error::other("not a native method"));
    };
    // stdout/stderr go to null: wl-copy and xclip fork a process that keeps
    // serving the clipboard, and inherited pipes would make wait() hang.
    let mut child = Command::new(program)
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()?;
    if let Some(mut stdin) = child.stdin.take() {
        stdin.write_all(&native_payload(method, text))?;
    }
    let status = child.wait()?;
    if status.success() {
        Ok(())
    } else {
        Err(io::Error::other(format!("{program} failed ({status})")))
    }
}

/// Write `text` as the plan says. Succeeds if any write succeeded.
#[cfg_attr(test, allow(dead_code))]
fn copy_text(text: &str, plan: &Plan) -> Result<(), String> {
    let mut ok = false;
    let mut last_err = String::from("no clipboard method available");
    if plan.osc52 {
        match write_osc52(text) {
            Ok(()) => ok = true,
            Err(e) => last_err = format!("{}: {e}", Method::Osc52.name()),
        }
    }
    for &method in &plan.native {
        match run_native(method, text) {
            Ok(()) => return Ok(()),
            Err(e) => last_err = format!("{}: {e}", method.name()),
        }
    }
    if ok {
        Ok(())
    } else {
        Err(last_err)
    }
}

/// Copy the marked entries (or the cursor entry) as `@path` references.
pub fn copy_at_paths(app: &mut App) {
    let paths = target_paths(app);
    if paths.is_empty() {
        return;
    }
    let style = PathStyle::from_env_value(std::env::var(PATH_STYLE_ENV).ok().as_deref());
    let text = format_at_paths(&paths, &app.launch_dir, style);
    match deliver(&text, &choose_plan(&ClipEnv::current())) {
        Ok(_) => {
            app.current_pane_mut().marked.clear();
            app.notify_info(format!("Copied {} path(s)", paths.len()));
        }
        Err(e) => app.notify_error(format!("Copy failed: {e}")),
    }
}

#[cfg(not(test))]
fn deliver(text: &str, plan: &Plan) -> Result<(), String> {
    copy_text(text, plan)
}

#[cfg(test)]
thread_local! {
    /// Text the last `copy_at_paths` would have put on the clipboard.
    pub static LAST_COPY: std::cell::RefCell<Option<String>> =
        const { std::cell::RefCell::new(None) };
}

/// Tests never touch the real terminal or clipboard.
#[cfg(test)]
fn deliver(text: &str, _plan: &Plan) -> Result<(), String> {
    LAST_COPY.with(|c| *c.borrow_mut() = Some(text.to_string()));
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rel(p: &str) -> String {
        format_at_path(Path::new(p), Path::new("/proj"), PathStyle::Relative)
    }

    #[test]
    fn path_under_base_is_relative() {
        assert_eq!(rel("/proj/src/main.rs"), "@src/main.rs");
    }

    #[test]
    fn base_itself_is_dot() {
        assert_eq!(rel("/proj"), "@.");
    }

    #[test]
    fn path_outside_base_is_absolute() {
        assert_eq!(rel("/etc/hosts"), "@/etc/hosts");
        // A sibling sharing the prefix string is still outside.
        assert_eq!(rel("/project/a"), "@/project/a");
    }

    #[test]
    fn absolute_style_keeps_full_path() {
        let s = format_at_path(
            Path::new("/proj/src/main.rs"),
            Path::new("/proj"),
            PathStyle::Absolute,
        );
        assert_eq!(s, "@/proj/src/main.rs");
    }

    #[test]
    fn whitespace_is_quoted() {
        assert_eq!(rel("/proj/my file.txt"), "@\"my file.txt\"");
        assert_eq!(rel("/proj/a\tb"), "@\"a\tb\"");
        assert_eq!(rel("/proj/日本語.md"), "@日本語.md");
    }

    #[test]
    fn multiple_paths_are_space_separated() {
        let paths = vec![
            PathBuf::from("/proj/a.rs"),
            PathBuf::from("/proj/b c.rs"),
            PathBuf::from("/tmp/x"),
        ];
        assert_eq!(
            format_at_paths(&paths, Path::new("/proj"), PathStyle::Relative),
            "@a.rs @\"b c.rs\" @/tmp/x"
        );
    }

    #[test]
    fn path_style_from_env() {
        assert_eq!(PathStyle::from_env_value(None), PathStyle::Relative);
        assert_eq!(
            PathStyle::from_env_value(Some("Absolute")),
            PathStyle::Absolute
        );
        assert_eq!(
            PathStyle::from_env_value(Some("bogus")),
            PathStyle::Relative
        );
    }

    #[test]
    fn base64_rfc4648_vectors() {
        let cases = [
            ("", ""),
            ("f", "Zg=="),
            ("fo", "Zm8="),
            ("foo", "Zm9v"),
            ("foob", "Zm9vYg=="),
            ("fooba", "Zm9vYmE="),
            ("foobar", "Zm9vYmFy"),
        ];
        for (input, want) in cases {
            assert_eq!(base64_encode(input.as_bytes()), want, "input {input:?}");
        }
        assert_eq!(base64_encode(&[0xff, 0xfe, 0xfd]), "//79");
    }

    #[test]
    fn osc52_sequence_format() {
        assert_eq!(osc52_sequence("@a.rs"), "\x1b]52;c;QGEucnM=\x1b\\");
    }

    fn env(mode: Option<&str>) -> ClipEnv {
        ClipEnv {
            mode: mode.map(str::to_string),
            ..ClipEnv::default()
        }
    }

    fn plan(osc52: bool, native: &[Method]) -> Plan {
        Plan {
            osc52,
            native: native.to_vec(),
        }
    }

    #[test]
    fn ssh_detection() {
        assert!(!is_ssh(None, None, None));
        assert!(is_ssh(Some("1.2.3.4 5 6.7.8.9 22"), None, None));
        assert!(is_ssh(None, Some("/dev/pts/0"), None));
        assert!(is_ssh(None, None, Some("1.2.3.4 5 22")));
    }

    #[test]
    fn auto_local_writes_osc52_and_native() {
        let mac = ClipEnv {
            macos: true,
            ..env(None)
        };
        assert_eq!(choose_plan(&mac), plan(true, &[Method::Pbcopy]));
        let wsl = ClipEnv {
            wsl: true,
            x11: true,
            ..env(Some("auto"))
        };
        assert_eq!(choose_plan(&wsl), plan(true, &[Method::ClipExe]));
        let linux = ClipEnv {
            wayland: true,
            x11: true,
            ..env(None)
        };
        assert_eq!(
            choose_plan(&linux),
            plan(true, &[Method::WlCopy, Method::Xclip])
        );
        // No display server: nothing native to try.
        assert_eq!(choose_plan(&env(None)), plan(true, &[]));
    }

    #[test]
    fn auto_over_ssh_is_osc52_only() {
        let remote = ClipEnv {
            ssh: true,
            x11: true,
            macos: true,
            ..env(None)
        };
        assert_eq!(choose_plan(&remote), plan(true, &[]));
    }

    #[test]
    fn mode_override() {
        let mac = |m| ClipEnv {
            macos: true,
            ..env(Some(m))
        };
        assert_eq!(choose_plan(&mac("osc52")), plan(true, &[]));
        assert_eq!(choose_plan(&mac("NATIVE")), plan(false, &[Method::Pbcopy]));
        assert_eq!(choose_plan(&mac("bogus")), plan(true, &[Method::Pbcopy]));
        // Explicit modes ignore ssh.
        let ssh_native = ClipEnv {
            ssh: true,
            x11: true,
            ..env(Some("native"))
        };
        assert_eq!(choose_plan(&ssh_native), plan(false, &[Method::Xclip]));
    }

    #[test]
    fn native_commands() {
        assert_eq!(
            native_command(Method::Xclip),
            Some(("xclip", &["-selection", "clipboard"][..]))
        );
        assert_eq!(native_command(Method::Pbcopy), Some(("pbcopy", &[][..])));
        assert_eq!(native_command(Method::Osc52), None);
    }

    #[test]
    fn clip_exe_gets_utf16le_with_bom() {
        assert_eq!(
            native_payload(Method::ClipExe, "@a"),
            vec![0xFF, 0xFE, b'@', 0, b'a', 0]
        );
        assert_eq!(native_payload(Method::Pbcopy, "@a"), b"@a".to_vec());
    }
}
