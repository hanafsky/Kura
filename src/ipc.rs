//! Control a running kura from outside: `kura msg reveal <path>`,
//! `kura msg cd <dir>`.
//!
//! Each kura listens on a Unix domain socket of its own,
//! `<dir>/kura-<pid>.sock`, where `<dir>` is `$XDG_RUNTIME_DIR/kura` or
//! `<temp dir>/kura-<uid>` (created private, mode 0700). Its path is put in
//! `KURA_SOCKET`, so programs kura starts (the editor, a shell in it)
//! reach their own kura. Otherwise `kura msg` sends to the most recently
//! started kura that still answers.
//!
//! The protocol is one JSON object per line, answered by one line:
//! `{"cmd":"reveal","path":"/abs/file"}` → `{"ok":true}` or
//! `{"ok":false,"error":"..."}`. The answer only says that the request
//! was understood; the result (e.g. a directory that cannot be read)
//! shows in kura's status line. Requests are read on a background thread
//! and handed to the event loop through a channel, like file changes.

use std::{
    ffi::OsStr,
    fs,
    io::{self, BufRead, BufReader, Read, Write},
    os::unix::{
        fs::{DirBuilderExt, FileTypeExt, MetadataExt, PermissionsExt},
        net::{UnixListener, UnixStream},
    },
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicUsize, Ordering},
        mpsc::Sender,
        Arc,
    },
    time::{Duration, SystemTime},
};

use serde::{Deserialize, Serialize};

use crate::command::Command;

/// Environment variable with the socket of the kura to send to. kura sets
/// it for the programs it starts.
pub const SOCKET_ENV: &str = "KURA_SOCKET";

/// Longest request line accepted; longer ones are refused.
const MAX_LINE: usize = 64 * 1024;

/// How long either side waits for the other to send or receive.
const IO_TIMEOUT: Duration = Duration::from_secs(2);

/// Connections served at the same time.
const MAX_CONNECTIONS: usize = 16;

/// Pause after a failed `accept`, so a persistent error does not spin.
const ACCEPT_RETRY: Duration = Duration::from_millis(100);

/// A request sent to a running kura.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "cmd", rename_all = "kebab-case", deny_unknown_fields)]
pub enum Request {
    /// Show `path` in the active pane with the cursor on it.
    Reveal { path: PathBuf },
    /// Move the active pane to the directory `path`.
    Cd { path: PathBuf },
}

impl Request {
    /// The command that carries out this request.
    pub fn into_command(self) -> Command {
        match self {
            Request::Reveal { path } => Command::Reveal(path),
            Request::Cd { path } => Command::ChangeDir(path),
        }
    }

    fn path(&self) -> &Path {
        match self {
            Request::Reveal { path } | Request::Cd { path } => path,
        }
    }
}

/// The answer to one request line.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Reply {
    pub ok: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

impl Reply {
    fn from_result(result: &Result<Request, String>) -> Self {
        match result {
            Ok(_) => Reply {
                ok: true,
                error: None,
            },
            Err(e) => Reply {
                ok: false,
                error: Some(e.clone()),
            },
        }
    }
}

/// Parse one request line. Paths must be absolute (the sender resolves
/// relative ones against its own working directory).
pub fn parse_request(line: &str) -> Result<Request, String> {
    let request: Request =
        serde_json::from_str(line.trim()).map_err(|e| format!("invalid request: {e}"))?;
    if !request.path().is_absolute() {
        return Err(format!(
            "path must be absolute: {}",
            request.path().display()
        ));
    }
    Ok(request)
}

/// One line of JSON (with the newline) for `value`.
fn json_line<T: Serialize>(value: &T) -> String {
    // Serializing these plain structs cannot fail.
    let mut line = serde_json::to_string(value).unwrap_or_default();
    line.push('\n');
    line
}

/// Read the answer line sent back by kura.
pub fn parse_reply(line: &str) -> Result<(), String> {
    let reply: Reply =
        serde_json::from_str(line.trim()).map_err(|e| format!("invalid reply from kura: {e}"))?;
    if reply.ok {
        Ok(())
    } else {
        Err(reply.error.unwrap_or_else(|| "request refused".to_string()))
    }
}

/// Directory that holds the sockets: `$XDG_RUNTIME_DIR/kura` when it is set
/// to an absolute path, else `<tmp>/kura-<uid>`.
pub fn socket_dir(xdg_runtime_dir: Option<&OsStr>, tmp: &Path, uid: u32) -> PathBuf {
    match xdg_runtime_dir.map(Path::new) {
        Some(dir) if dir.is_absolute() => dir.join("kura"),
        _ => tmp.join(format!("kura-{uid}")),
    }
}

/// File name of the socket of the kura with process id `pid`.
pub fn socket_name(pid: u32) -> String {
    format!("kura-{pid}.sock")
}

/// Process id from a socket file name made by [`socket_name`].
pub fn parse_socket_name(name: &str) -> Option<u32> {
    let digits = name.strip_prefix("kura-")?.strip_suffix(".sock")?;
    if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    digits.parse().ok()
}

/// Which socket `kura msg` sends to: `--socket`, else `KURA_SOCKET` (when
/// not empty). `None` means the most recently started kura.
pub fn explicit_socket(flag: Option<&Path>, env: Option<&OsStr>) -> Option<PathBuf> {
    flag.map(Path::to_path_buf)
        .or_else(|| env.filter(|v| !v.is_empty()).map(PathBuf::from))
}

/// The socket directory for this user and environment.
fn default_socket_dir() -> PathBuf {
    let xdg = std::env::var_os("XDG_RUNTIME_DIR");
    // SAFETY: geteuid has no preconditions and cannot fail.
    let uid = unsafe { libc::geteuid() };
    socket_dir(xdg.as_deref(), &std::env::temp_dir(), uid)
}

/// Create the socket directory (mode 0700) if needed and check that it is
/// a real directory that only this user can use.
fn ensure_private_dir(dir: &Path) -> io::Result<()> {
    match fs::DirBuilder::new().mode(0o700).create(dir) {
        Ok(()) => {}
        Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {}
        Err(e) => return Err(e),
    }
    let meta = dir.symlink_metadata()?;
    // SAFETY: geteuid has no preconditions and cannot fail.
    let uid = unsafe { libc::geteuid() };
    if !meta.is_dir() || meta.uid() != uid || meta.permissions().mode() & 0o077 != 0 {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            format!("not a private directory: {}", dir.display()),
        ));
    }
    Ok(())
}

/// The socket a running kura listens on. The listening thread hands each
/// request to the event loop; dropping the server removes the socket file.
pub struct Server {
    path: PathBuf,
}

impl Server {
    /// Listen in the default socket directory (see the module docs) and
    /// put the socket in [`SOCKET_ENV`] for the programs kura starts. On
    /// failure the variable is removed, so that they do not reach the kura
    /// this one was started from. Call before any other thread exists.
    pub fn start<E: Send + 'static>(tx: Sender<E>, wrap: fn(Request) -> E) -> io::Result<Self> {
        let result = Self::bind_default().map(|(server, listener)| {
            std::env::set_var(SOCKET_ENV, &server.path);
            (server, listener)
        });
        let result = result.and_then(|(server, listener)| server.spawn(listener, tx, wrap));
        if result.is_err() {
            std::env::remove_var(SOCKET_ENV);
        }
        result
    }

    fn bind_default() -> io::Result<(Self, UnixListener)> {
        let dir = default_socket_dir();
        ensure_private_dir(&dir)?;
        Self::bind(dir.join(socket_name(std::process::id())))
    }

    /// Listen on `path` (without touching the environment).
    #[cfg(test)]
    pub fn start_at<E: Send + 'static>(
        path: PathBuf,
        tx: Sender<E>,
        wrap: fn(Request) -> E,
    ) -> io::Result<Self> {
        let (server, listener) = Self::bind(path)?;
        server.spawn(listener, tx, wrap)
    }

    /// Bind `path`, replacing a socket left there by an earlier process
    /// with the same id.
    fn bind(path: PathBuf) -> io::Result<(Self, UnixListener)> {
        if path
            .symlink_metadata()
            .is_ok_and(|m| m.file_type().is_socket())
        {
            fs::remove_file(&path)?;
        }
        let listener = UnixListener::bind(&path)?;
        Ok((Self { path }, listener))
    }

    /// Answer requests on a background thread. (On failure `self` is
    /// dropped, which removes the socket.)
    fn spawn<E: Send + 'static>(
        self,
        listener: UnixListener,
        tx: Sender<E>,
        wrap: fn(Request) -> E,
    ) -> io::Result<Self> {
        std::thread::Builder::new()
            .name("kura-ipc".into())
            .spawn(move || serve(listener, tx, wrap))?;
        Ok(self)
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

/// Accept connections, each answered on a thread of its own so that a
/// client that stays silent does not hold up the others. At most
/// [`MAX_CONNECTIONS`] are served at once; more are closed right away.
fn serve<E: Send + 'static>(listener: UnixListener, tx: Sender<E>, wrap: fn(Request) -> E) {
    let open = Arc::new(AtomicUsize::new(0));
    for stream in listener.incoming() {
        let Ok(stream) = stream else {
            std::thread::sleep(ACCEPT_RETRY);
            continue;
        };
        if open.fetch_add(1, Ordering::SeqCst) >= MAX_CONNECTIONS {
            open.fetch_sub(1, Ordering::SeqCst);
            continue;
        }
        let (tx, done) = (tx.clone(), Arc::clone(&open));
        let spawned = std::thread::Builder::new()
            .name("kura-ipc-conn".into())
            .spawn(move || {
                handle_connection(stream, &tx, wrap);
                done.fetch_sub(1, Ordering::SeqCst);
            });
        if spawned.is_err() {
            open.fetch_sub(1, Ordering::SeqCst);
        }
    }
}

/// Answer every request line of one connection.
fn handle_connection<E>(stream: UnixStream, tx: &Sender<E>, wrap: fn(Request) -> E) {
    let _ = stream.set_read_timeout(Some(IO_TIMEOUT));
    let _ = stream.set_write_timeout(Some(IO_TIMEOUT));
    let Ok(read_half) = stream.try_clone() else {
        return;
    };
    let mut reader = BufReader::new(read_half);
    let mut writer = stream;
    loop {
        let line = match read_line(&mut reader) {
            Ok(Some(line)) => line,
            // End of input, a timeout or a broken connection.
            Ok(None) | Err(_) => return,
        };
        let line = match line {
            Line::Text(text) if text.trim().is_empty() => continue,
            Line::Text(text) => text,
            Line::TooLong => {
                let reply = Reply::from_result(&Err("request too long".into()));
                let _ = writer.write_all(json_line(&reply).as_bytes());
                return;
            }
        };
        let request = parse_request(&line);
        let reply = Reply::from_result(&request);
        if let Ok(request) = request {
            if tx.send(wrap(request)).is_err() {
                // The event loop has ended; kura is exiting.
                return;
            }
        }
        if writer.write_all(json_line(&reply).as_bytes()).is_err() {
            return;
        }
    }
}

/// A line read by [`read_line`].
#[derive(Debug, PartialEq, Eq)]
enum Line {
    Text(String),
    /// Longer than [`MAX_LINE`] (the rest of the input is not read).
    TooLong,
}

/// Read one line of at most [`MAX_LINE`] bytes; `None` at the end of the
/// input. Invalid UTF-8 is replaced (and then fails to parse as JSON).
fn read_line(reader: &mut impl BufRead) -> io::Result<Option<Line>> {
    let mut buf = Vec::new();
    let n = reader
        .by_ref()
        .take(MAX_LINE as u64 + 1)
        .read_until(b'\n', &mut buf)?;
    if n == 0 {
        return Ok(None);
    }
    if buf.last() == Some(&b'\n') {
        buf.pop();
    } else if buf.len() > MAX_LINE {
        return Ok(Some(Line::TooLong));
    }
    Ok(Some(Line::Text(String::from_utf8_lossy(&buf).into_owned())))
}

/// Resolve a path given to `kura msg` against the sender's working
/// directory. Only the parent is resolved, so a symbolic link is revealed
/// as itself rather than as its target. The path must exist.
pub fn resolve_path(arg: &Path, cwd: &Path) -> Result<PathBuf, String> {
    let joined = cwd.join(arg);
    let describe = |e: io::Error| format!("{}: {e}", arg.display());
    let resolved = match (joined.parent(), joined.file_name()) {
        (Some(parent), Some(name)) if !matches!(name.to_str(), Some("." | "..")) => {
            let parent = parent.canonicalize().map_err(describe)?;
            parent.join(name)
        }
        // `/`, `..`, `dir/..`: resolve the whole path.
        _ => joined.canonicalize().map_err(describe)?,
    };
    resolved.symlink_metadata().map_err(describe)?;
    if resolved.to_str().is_none() {
        return Err(format!("{}: path is not valid UTF-8", arg.display()));
    }
    Ok(resolved)
}

/// Send `request` to the kura at `socket` (or the most recently started
/// one) and wait for its answer.
pub fn send(socket: Option<&Path>, request: &Request) -> Result<(), String> {
    let mut stream = match socket {
        Some(path) => UnixStream::connect(path)
            .map_err(|e| format!("cannot connect to {}: {e}", path.display()))?,
        None => connect_latest(&default_socket_dir())?,
    };
    let io_err = |e: io::Error| format!("cannot talk to kura: {e}");
    stream.set_read_timeout(Some(IO_TIMEOUT)).map_err(io_err)?;
    stream.set_write_timeout(Some(IO_TIMEOUT)).map_err(io_err)?;
    stream
        .write_all(json_line(request).as_bytes())
        .map_err(io_err)?;
    let _ = stream.shutdown(std::net::Shutdown::Write);
    let mut reply = String::new();
    BufReader::new(stream.take(MAX_LINE as u64))
        .read_line(&mut reply)
        .map_err(io_err)?;
    if reply.is_empty() {
        return Err("kura closed the connection without answering".into());
    }
    parse_reply(&reply)
}

/// Connect to the most recently started kura in `dir` that still answers.
/// Sockets that refuse connections (left by a kura that was killed) are
/// removed on the way.
fn connect_latest(dir: &Path) -> Result<UnixStream, String> {
    let none = || "no running kura found (set KURA_SOCKET or use --socket)".to_string();
    let entries = fs::read_dir(dir).map_err(|_| none())?;
    let mut sockets: Vec<(SystemTime, PathBuf)> = entries
        .flatten()
        .filter(|e| e.file_name().to_str().and_then(parse_socket_name).is_some())
        .filter_map(|e| {
            let meta = e.metadata().ok()?;
            Some((meta.modified().ok()?, e.path()))
        })
        .collect();
    sockets.sort_by_key(|s| std::cmp::Reverse(s.0));
    for (_, path) in sockets {
        match UnixStream::connect(&path) {
            Ok(stream) => return Ok(stream),
            Err(e) if e.kind() == io::ErrorKind::ConnectionRefused => {
                let _ = fs::remove_file(&path);
            }
            Err(_) => {}
        }
    }
    Err(none())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;
    use std::sync::mpsc;

    #[test]
    fn requests_parse_from_one_json_object_per_line() {
        assert_eq!(
            parse_request(r#"{"cmd":"reveal","path":"/a/b.txt"}"#),
            Ok(Request::Reveal {
                path: PathBuf::from("/a/b.txt")
            })
        );
        assert_eq!(
            parse_request(" {\"cmd\":\"cd\",\"path\":\"/tmp\"}\r\n"),
            Ok(Request::Cd {
                path: PathBuf::from("/tmp")
            })
        );
        // Paths are any JSON string (spaces, newlines, non-ASCII).
        assert_eq!(
            parse_request(r#"{"cmd":"reveal","path":"/a b/\n/日本語"}"#),
            Ok(Request::Reveal {
                path: PathBuf::from("/a b/\n/日本語")
            })
        );
    }

    #[test]
    fn bad_requests_are_errors() {
        for line in [
            "",
            "reveal /a",
            "{}",
            r#"{"cmd":"rm","path":"/a"}"#,
            r#"{"cmd":"reveal"}"#,
            r#"{"cmd":"reveal","path":1}"#,
            r#"{"cmd":"reveal","path":"/a","extra":true}"#,
            r#"{"cmd":"reveal","path":"/a"} trailing"#,
            r#"[{"cmd":"reveal","path":"/a"}]"#,
        ] {
            assert!(parse_request(line).is_err(), "{line:?}");
        }
        assert_eq!(
            parse_request(r#"{"cmd":"cd","path":"rel/dir"}"#),
            Err("path must be absolute: rel/dir".to_string())
        );
    }

    #[test]
    fn requests_round_trip_and_map_to_commands() {
        let reveal = Request::Reveal {
            path: PathBuf::from("/x/y"),
        };
        let line = json_line(&reveal);
        assert_eq!(line, "{\"cmd\":\"reveal\",\"path\":\"/x/y\"}\n");
        assert_eq!(parse_request(&line), Ok(reveal.clone()));
        assert_eq!(
            reveal.into_command(),
            Command::Reveal(PathBuf::from("/x/y"))
        );
        let cd = Request::Cd {
            path: PathBuf::from("/x"),
        };
        assert_eq!(cd.into_command(), Command::ChangeDir(PathBuf::from("/x")));
    }

    #[test]
    fn replies() {
        let ok = Reply::from_result(&Ok(Request::Cd { path: "/".into() }));
        assert_eq!(json_line(&ok), "{\"ok\":true}\n");
        assert_eq!(parse_reply("{\"ok\":true}\n"), Ok(()));
        let err = Reply::from_result(&Err("nope".into()));
        assert_eq!(json_line(&err), "{\"ok\":false,\"error\":\"nope\"}\n");
        assert_eq!(parse_reply(&json_line(&err)), Err("nope".into()));
        assert_eq!(parse_reply("{\"ok\":false}"), Err("request refused".into()));
        assert!(parse_reply("garbage").is_err());
    }

    #[test]
    fn socket_dir_prefers_xdg_runtime_dir() {
        let tmp = Path::new("/tmp");
        assert_eq!(
            socket_dir(Some(OsStr::new("/run/user/501")), tmp, 501),
            PathBuf::from("/run/user/501/kura")
        );
        assert_eq!(socket_dir(None, tmp, 501), PathBuf::from("/tmp/kura-501"));
        // Empty or relative values are ignored, as the XDG spec says.
        assert_eq!(
            socket_dir(Some(OsStr::new("")), tmp, 7),
            PathBuf::from("/tmp/kura-7")
        );
        assert_eq!(
            socket_dir(Some(OsStr::new("run")), tmp, 7),
            PathBuf::from("/tmp/kura-7")
        );
    }

    #[test]
    fn socket_names_carry_the_pid() {
        assert_eq!(socket_name(4242), "kura-4242.sock");
        assert_eq!(parse_socket_name("kura-4242.sock"), Some(4242));
        for name in [
            "kura-.sock",
            "kura-12",
            "kura-+1.sock",
            "kura-1a.sock",
            "other-1.sock",
            "kura-99999999999.sock",
        ] {
            assert_eq!(parse_socket_name(name), None, "{name}");
        }
    }

    #[test]
    fn flag_beats_env_and_empty_env_means_latest() {
        let flag = Path::new("/f.sock");
        let env = OsStr::new("/e.sock");
        assert_eq!(
            explicit_socket(Some(flag), Some(env)),
            Some(flag.to_path_buf())
        );
        assert_eq!(explicit_socket(None, Some(env)), Some(PathBuf::from(env)));
        assert_eq!(explicit_socket(None, Some(OsStr::new(""))), None);
        assert_eq!(explicit_socket(None, None), None);
    }

    #[test]
    fn read_line_limits_the_length() {
        let mut input = Cursor::new(b"one\ntwo".to_vec());
        assert_eq!(
            read_line(&mut input).unwrap(),
            Some(Line::Text("one".into()))
        );
        assert_eq!(
            read_line(&mut input).unwrap(),
            Some(Line::Text("two".into()))
        );
        assert_eq!(read_line(&mut input).unwrap(), None);

        let exact = "x".repeat(MAX_LINE);
        let mut input = Cursor::new(format!("{exact}\n").into_bytes());
        assert_eq!(read_line(&mut input).unwrap(), Some(Line::Text(exact)));

        let mut input = Cursor::new(vec![b'x'; MAX_LINE + 10]);
        assert_eq!(read_line(&mut input).unwrap(), Some(Line::TooLong));

        let mut input = Cursor::new(vec![0xff, b'\n']);
        assert_eq!(
            read_line(&mut input).unwrap(),
            Some(Line::Text("\u{fffd}".into()))
        );
    }

    #[test]
    fn resolve_path_is_absolute_and_keeps_symlinks() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().canonicalize().unwrap();
        fs::create_dir(root.join("sub")).unwrap();
        fs::write(root.join("sub/f.txt"), "x").unwrap();
        std::os::unix::fs::symlink(root.join("sub/f.txt"), root.join("link")).unwrap();
        let sub = root.join("sub");

        assert_eq!(
            resolve_path(Path::new("f.txt"), &sub).unwrap(),
            sub.join("f.txt")
        );
        assert_eq!(
            resolve_path(Path::new("./f.txt"), &sub).unwrap(),
            sub.join("f.txt")
        );
        assert_eq!(
            resolve_path(Path::new("../link"), &sub).unwrap(),
            root.join("link")
        );
        assert_eq!(resolve_path(Path::new(".."), &sub).unwrap(), root);
        assert_eq!(resolve_path(Path::new("."), &sub).unwrap(), sub);
        assert_eq!(
            resolve_path(&sub.join("f.txt"), Path::new("/")).unwrap(),
            sub.join("f.txt")
        );
        assert_eq!(
            resolve_path(Path::new("/"), &sub).unwrap(),
            PathBuf::from("/")
        );
        assert!(resolve_path(Path::new("missing"), &sub).is_err());
        assert!(resolve_path(Path::new("nodir/f"), &sub).is_err());
    }

    /// A short socket path (macOS limits them to 104 bytes, and test temp
    /// dirs can be long).
    fn short_socket(tag: &str) -> PathBuf {
        PathBuf::from(format!("/tmp/kura-t-{}-{tag}.sock", std::process::id()))
    }

    #[test]
    fn server_hands_requests_to_the_loop_and_removes_its_socket() {
        let path = short_socket("srv");
        let (tx, rx) = mpsc::channel();
        let server = Server::start_at(path.clone(), tx, |r| r).unwrap();
        assert!(path.exists());

        let req = Request::Reveal {
            path: PathBuf::from("/a/b"),
        };
        assert_eq!(send(Some(&path), &req), Ok(()));
        assert_eq!(rx.recv_timeout(IO_TIMEOUT), Ok(req));

        // Bad input is answered with an error and does not stop the server.
        let mut raw = UnixStream::connect(&path).unwrap();
        raw.write_all(b"\nnot json\n{\"cmd\":\"cd\",\"path\":\"/\"}\n")
            .unwrap();
        raw.shutdown(std::net::Shutdown::Write).unwrap();
        let mut answers = String::new();
        raw.read_to_string(&mut answers).unwrap();
        let lines: Vec<&str> = answers.lines().collect();
        assert_eq!(lines.len(), 2, "{answers}");
        assert!(parse_reply(lines[0]).is_err());
        assert_eq!(parse_reply(lines[1]), Ok(()));
        assert_eq!(
            rx.recv_timeout(IO_TIMEOUT),
            Ok(Request::Cd { path: "/".into() })
        );

        // A line that never ends is cut off.
        let mut raw = UnixStream::connect(&path).unwrap();
        raw.write_all(&vec![b'x'; MAX_LINE + 1]).unwrap();
        let mut answer = String::new();
        BufReader::new(&raw).read_line(&mut answer).unwrap();
        assert_eq!(parse_reply(&answer), Err("request too long".into()));

        // A client that connects and says nothing does not hold up others.
        let _idle = UnixStream::connect(&path).unwrap();
        let req = Request::Cd { path: "/x".into() };
        assert_eq!(send(Some(&path), &req), Ok(()));
        assert_eq!(rx.recv_timeout(IO_TIMEOUT * 2), Ok(req));

        assert!(rx.try_recv().is_err());
        drop(server);
        assert!(!path.exists());
    }

    #[test]
    fn sending_to_a_missing_socket_fails() {
        let path = short_socket("none");
        let req = Request::Cd { path: "/".into() };
        assert!(send(Some(&path), &req).is_err());
    }

    #[test]
    fn latest_skips_and_removes_dead_sockets() {
        let dir = PathBuf::from(format!("/tmp/kura-t-{}-dir", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        ensure_private_dir(&dir).unwrap();
        assert!(connect_latest(&dir).is_err());

        let (tx, rx) = mpsc::channel();
        let live = Server::start_at(dir.join(socket_name(1)), tx, |r| r).unwrap();
        // A socket whose listener is gone (as after `kill -9`), newer.
        std::thread::sleep(Duration::from_millis(20));
        let dead_path = dir.join(socket_name(2));
        drop(UnixListener::bind(&dead_path).unwrap());
        // Files with other names are left alone.
        fs::write(dir.join("notes.txt"), "").unwrap();

        let mut stream = connect_latest(&dir).unwrap();
        stream
            .write_all(b"{\"cmd\":\"cd\",\"path\":\"/\"}\n")
            .unwrap();
        assert_eq!(
            rx.recv_timeout(IO_TIMEOUT),
            Ok(Request::Cd { path: "/".into() })
        );
        assert!(!dead_path.exists());
        assert!(dir.join("notes.txt").exists());

        drop(live);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn socket_dir_must_be_private() {
        let dir = PathBuf::from(format!("/tmp/kura-t-{}-perm", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        ensure_private_dir(&dir).unwrap();
        assert_eq!(dir.metadata().unwrap().permissions().mode() & 0o777, 0o700);
        fs::set_permissions(&dir, fs::Permissions::from_mode(0o755)).unwrap();
        assert!(ensure_private_dir(&dir).is_err());
        let _ = fs::remove_dir_all(&dir);
    }
}
