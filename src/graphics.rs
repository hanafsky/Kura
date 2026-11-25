//! Terminal graphics protocol detection.
//!
//! The terminal is asked which image protocol it supports (Kitty graphics
//! query, DA1 for Sixel, `CSI 16 t` for the cell size, DSR as a terminator)
//! and its replies are read from stdin. Keys the user types meanwhile arrive
//! on the same stdin, so the reader keeps every byte that is not a reply and
//! hands it to the event loop as type-ahead (see [`split_replies`]).
//!
//! We do this ourselves instead of using `ratatui-image`'s
//! `Picker::from_query_stdio` because that one
//! - drops every byte that is not a reply (keys typed during startup),
//! - stops at the DSR reply, missing a Kitty reply that some multiplexers
//!   (e.g. herdr) forward a bit later, and
//! - reads stdin from a thread that stays blocked after its timeout when the
//!   terminal never answers, swallowing the next key press.
//!
//! An explicit override is possible via `KURA_IMAGE_PROTOCOL`.

use crossterm::event::KeyCode;
use ratatui_image::picker::{Picker, ProtocolType};
#[cfg(unix)]
use ratatui_image::FontSize;

/// Environment variable to force a protocol: kitty | sixel | iterm2 | halfblocks.
pub const PROTOCOL_ENV: &str = "KURA_IMAGE_PROTOCOL";

/// Result of [`detect`].
pub struct Detected {
    pub picker: Picker,
    /// Short description for the UI.
    pub info: String,
    /// Keys typed while the terminal was being queried, oldest first.
    pub typeahead: Vec<KeyCode>,
}

/// Detect the graphics protocol. Must be called after raw mode is enabled and
/// before the event loop starts reading input. Takes at most about a second
/// even if the terminal never answers.
pub fn detect() -> Detected {
    let (mut picker, mut note, typeahead) = query_picker();

    if let Some(forced) = std::env::var(PROTOCOL_ENV)
        .ok()
        .and_then(|v| parse_protocol(&v))
    {
        picker.set_protocol_type(forced);
        note = Some(format!("forced by {PROTOCOL_ENV}"));
    }

    let fs = picker.font_size();
    let mut info = format!("{:?} {}x{}px", picker.protocol_type(), fs.width, fs.height);
    if let Some(n) = note {
        info.push_str(&format!(" ({n})"));
    }
    Detected {
        picker,
        info,
        typeahead,
    }
}

#[cfg(unix)]
fn query_picker() -> (Picker, Option<String>, Vec<KeyCode>) {
    use ratatui_image::picker::cap_parser::{Parser, QueryStdioOptions};
    use std::io::Write;

    // Only for its side effects: tmux detection and enabling tmux
    // passthrough, which must happen before the query is sent.
    #[allow(deprecated)]
    let is_tmux = Picker::from_fontsize(FontSize::new(10, 20)).tmux_detected();

    // Like ratatui-image: WezTerm and Konsole get iTerm2 (from the env)
    // because their Kitty / Sixel support is incomplete.
    let env_set = |k: &str| std::env::var(k).is_ok_and(|v| !v.is_empty());
    let pixel_protocols_ok = !env_set("WEZTERM_EXECUTABLE") && !env_set("KONSOLE_VERSION");
    let mut options = QueryStdioOptions::default();
    if !pixel_protocols_ok {
        options.blacklist_protocols = vec![ProtocolType::Kitty, ProtocolType::Sixel];
    }
    let query = Parser::query(is_tmux, options);

    let mut out = std::io::stdout();
    let replies = if out
        .write_all(query.as_bytes())
        .and_then(|_| out.flush())
        .is_ok()
    {
        read_replies(pixel_protocols_ok)
    } else {
        Scan::default()
    };

    let typeahead = crate::input::typeahead_keys(&replies.input);
    let note = (!replies.replies.status).then(|| "no reply".to_string());
    let Some((w, h)) = replies.replies.cell_size.or_else(cell_size_from_winsize) else {
        // Without the cell size images would be scaled with a made-up font
        // size, so stay with halfblocks.
        return (Picker::halfblocks(), note, typeahead);
    };
    #[allow(deprecated)]
    let mut picker = Picker::from_fontsize(FontSize::new(w, h));
    if let Some(p) = choose_protocol(&replies.replies) {
        picker.set_protocol_type(p);
    }
    (picker, note, typeahead)
}

#[cfg(not(unix))]
fn query_picker() -> (Picker, Option<String>, Vec<KeyCode>) {
    match Picker::from_query_stdio() {
        Ok(p) => (p, None, Vec::new()),
        Err(e) => (
            Picker::halfblocks(),
            Some(format!("query failed: {e:?}")),
            Vec::new(),
        ),
    }
}

/// Give up on the terminal after this long, answered or not.
#[cfg(unix)]
const QUERY_DEADLINE: std::time::Duration = std::time::Duration::from_millis(1000);
/// After the DSR reply, how long to wait for a Kitty reply that some
/// multiplexers forward late.
#[cfg(unix)]
const KITTY_GRACE: std::time::Duration = std::time::Duration::from_millis(150);

/// Read stdin until the replies are complete (see [`query_done`]) or the
/// deadline passes. Reads the file descriptor directly, so no byte is left
/// behind in a buffer the event loop cannot see.
#[cfg(unix)]
fn read_replies(kitty_queried: bool) -> Scan {
    use std::time::Instant;

    let start = Instant::now();
    let deadline = start + QUERY_DEADLINE;
    let mut buf = Vec::new();
    let mut status_at: Option<Instant> = None;
    loop {
        let scan = split_replies(&buf);
        let now = Instant::now();
        if scan.replies.status {
            status_at.get_or_insert(now);
        }
        let since_status = status_at.map(|t| now.duration_since(t));
        if now >= deadline || query_done(&scan, kitty_queried, since_status) {
            return scan.finish();
        }
        let mut wait = deadline - now;
        if let Some(s) = since_status {
            wait = wait.min(KITTY_GRACE.saturating_sub(s));
        }
        let mut pfd = libc::pollfd {
            fd: libc::STDIN_FILENO,
            events: libc::POLLIN,
            revents: 0,
        };
        let ms = wait.as_millis().max(1) as libc::c_int;
        let ready = unsafe { libc::poll(&mut pfd, 1, ms) };
        if ready < 0 {
            if std::io::Error::last_os_error().kind() == std::io::ErrorKind::Interrupted {
                continue;
            }
            return scan.finish();
        }
        if ready == 0 || pfd.revents & libc::POLLIN == 0 {
            continue;
        }
        let mut chunk = [0u8; 256];
        let n = unsafe { libc::read(libc::STDIN_FILENO, chunk.as_mut_ptr().cast(), chunk.len()) };
        if n <= 0 {
            return scan.finish();
        }
        buf.extend_from_slice(&chunk[..n as usize]);
    }
}

/// Whether to stop reading: the DSR reply (sent last) has arrived, no reply
/// is half-read, and either Kitty has answered, was not asked, or had
/// [`KITTY_GRACE`] to do so.
#[cfg(unix)]
fn query_done(scan: &Scan, kitty_queried: bool, since_status: Option<std::time::Duration>) -> bool {
    scan.replies.status
        && (!scan.partial || scan.lone_esc)
        && (scan.replies.kitty_answered
            || !kitty_queried
            || since_status.is_some_and(|s| s >= KITTY_GRACE))
}

/// Cell size from the window size in pixels, for terminals that do not
/// answer `CSI 16 t`.
#[cfg(unix)]
fn cell_size_from_winsize() -> Option<(u16, u16)> {
    let mut ws: libc::winsize = unsafe { std::mem::zeroed() };
    if unsafe { libc::ioctl(libc::STDOUT_FILENO, libc::TIOCGWINSZ, &mut ws) } != 0 {
        return None;
    }
    if ws.ws_xpixel == 0 || ws.ws_ypixel == 0 || ws.ws_col == 0 || ws.ws_row == 0 {
        return None;
    }
    Some((ws.ws_xpixel / ws.ws_col, ws.ws_ypixel / ws.ws_row))
}

/// The protocol the replies point to, best first; `None` leaves the guess
/// from the environment (e.g. iTerm2) in place.
fn choose_protocol(r: &Replies) -> Option<ProtocolType> {
    if r.kitty {
        Some(ProtocolType::Kitty)
    } else if r.sixel {
        Some(ProtocolType::Sixel)
    } else {
        None
    }
}

fn parse_protocol(s: &str) -> Option<ProtocolType> {
    match s.trim().to_lowercase().as_str() {
        "kitty" => Some(ProtocolType::Kitty),
        "sixel" => Some(ProtocolType::Sixel),
        "iterm2" | "iterm" => Some(ProtocolType::Iterm2),
        "halfblocks" | "halfblock" | "none" => Some(ProtocolType::Halfblocks),
        _ => None,
    }
}

/// What the terminal told us.
#[derive(Debug, Default, PartialEq)]
pub struct Replies {
    /// The Kitty graphics query was answered (OK or an error).
    pub kitty_answered: bool,
    /// The Kitty graphics query was answered with OK.
    pub kitty: bool,
    /// DA1 lists Sixel (attribute 4).
    pub sixel: bool,
    /// Cell size in pixels (width, height) from `CSI 6 ; h ; w t`.
    pub cell_size: Option<(u16, u16)>,
    /// The DSR reply (`CSI 0 n`) arrived.
    pub status: bool,
}

/// Bytes read from stdin while querying, split into replies and user input.
#[derive(Debug, Default, PartialEq)]
pub struct Scan {
    pub replies: Replies,
    /// Everything that is not a reply (keys typed meanwhile), in order.
    pub input: Vec<u8>,
    /// The bytes end in the middle of an escape sequence. It is left out of
    /// `input`; once more bytes arrive the whole buffer is split again.
    pub partial: bool,
    /// The held-back tail is a single Esc: once the replies are complete it
    /// can only be the Esc key.
    pub lone_esc: bool,
}

impl Scan {
    /// Final result when reading stops: a trailing lone Esc is the Esc key;
    /// any other unfinished sequence is dropped (read as keys, its `?`,
    /// digits, … would trigger commands).
    pub fn finish(mut self) -> Scan {
        if self.lone_esc {
            self.input.push(ESC);
        }
        self.partial = false;
        self.lone_esc = false;
        self
    }
}

const ESC: u8 = 0x1b;

/// Split the bytes read so far into the terminal's replies and the user's
/// key presses. Recognised as replies: APC (Kitty `ESC _ G … ESC \`), OSC and
/// DCS strings, DA1 (`CSI ? … c`), cell size (`CSI 6 ; h ; w t`) and DSR
/// (`CSI 0 n` / `CSI 3 n`). Everything else, including CSI sequences typed by
/// the user such as arrow keys, is kept as input.
pub fn split_replies(buf: &[u8]) -> Scan {
    let mut scan = Scan::default();
    let mut i = 0;
    while i < buf.len() {
        if buf[i] != ESC {
            scan.input.push(buf[i]);
            i += 1;
            continue;
        }
        let Some(&kind) = buf.get(i + 1) else {
            scan.partial = true;
            scan.lone_esc = true;
            break;
        };
        match kind {
            // String sequences, terminated by ST (`ESC \`) or, for OSC, BEL.
            b'_' | b']' | b'P' => {
                let body_start = i + 2;
                let Some((body_end, next)) = find_string_end(buf, body_start, kind == b']') else {
                    scan.partial = true;
                    break;
                };
                let body = &buf[body_start..body_end];
                if kind == b'_' && body.first() == Some(&b'G') && contains(body, b"i=31") {
                    scan.replies.kitty_answered = true;
                    if contains(body, b";OK") {
                        scan.replies.kitty = true;
                    }
                }
                i = next;
            }
            b'[' => {
                // Parameter / intermediate bytes, then a final byte.
                let Some(fin) = (i + 2..buf.len()).find(|&j| !(0x20..=0x3f).contains(&buf[j]))
                else {
                    scan.partial = true;
                    break;
                };
                let params = &buf[i + 2..fin];
                if classify_csi(params, buf[fin], &mut scan.replies) {
                    i = fin + 1;
                } else {
                    scan.input.extend_from_slice(&buf[i..=fin]);
                    i = fin + 1;
                }
            }
            _ => {
                // Esc pressed on its own, or Alt+key: user input.
                scan.input.push(ESC);
                i += 1;
            }
        }
    }
    scan
}

/// End of a string sequence starting at `from`: (end of body, index after
/// the terminator).
fn find_string_end(buf: &[u8], from: usize, bel_ends: bool) -> Option<(usize, usize)> {
    (from..buf.len()).find_map(|j| match buf[j] {
        0x07 if bel_ends => Some((j, j + 1)),
        ESC if buf.get(j + 1) == Some(&b'\\') => Some((j, j + 2)),
        _ => None,
    })
}

/// Record a CSI sequence if it is one of the replies; false if it is not.
fn classify_csi(params: &[u8], fin: u8, r: &mut Replies) -> bool {
    let text = std::str::from_utf8(params).unwrap_or("");
    match fin {
        b'c' if text.starts_with('?') => {
            r.sixel |= text[1..].split(';').any(|p| p == "4");
            true
        }
        b't' => {
            let nums: Vec<&str> = text.split(';').collect();
            match nums[..] {
                ["6", h, w] => {
                    if let (Ok(h), Ok(w)) = (h.parse::<u16>(), w.parse::<u16>()) {
                        if h > 0 && w > 0 {
                            r.cell_size = Some((w, h));
                        }
                    }
                    true
                }
                _ => false,
            }
        }
        b'n' if text == "0" || text == "3" => {
            r.status = true;
            true
        }
        _ => false,
    }
}

fn contains(hay: &[u8], needle: &[u8]) -> bool {
    hay.windows(needle.len()).any(|w| w == needle)
}

#[cfg(test)]
mod tests {
    use super::*;

    const KITTY_OK: &[u8] = b"\x1b_Gi=31;OK\x1b\\";
    const DA1_SIXEL: &[u8] = b"\x1b[?62;4;22c";
    const CELL: &[u8] = b"\x1b[6;20;10t";
    const DSR: &[u8] = b"\x1b[0n";

    fn cat(parts: &[&[u8]]) -> Vec<u8> {
        parts.concat()
    }

    #[test]
    fn full_reply_is_parsed_with_no_input() {
        let s = split_replies(&cat(&[KITTY_OK, DA1_SIXEL, CELL, DSR]));
        assert_eq!(
            s.replies,
            Replies {
                kitty_answered: true,
                kitty: true,
                sixel: true,
                cell_size: Some((10, 20)),
                status: true,
            }
        );
        assert!(s.input.is_empty());
        assert!(!s.partial);
    }

    #[test]
    fn keys_before_between_and_after_replies_are_kept_in_order() {
        let s = split_replies(&cat(&[b"j", KITTY_OK, b"k", DA1_SIXEL, CELL, DSR, b"q"]));
        assert_eq!(s.input, b"jkq");
        assert!(s.replies.kitty && s.replies.status);
    }

    #[test]
    fn key_typed_before_a_kitty_answering_terminal_replies_is_kept() {
        let ghostty = split_replies(&cat(&[b"j", KITTY_OK, DA1_SIXEL, CELL, DSR]));
        // herdr: cell size and DSR first, Kitty and DA1 a little later.
        let herdr = split_replies(&cat(&[b"j", CELL, DSR, KITTY_OK, b"\x1b[?62;22c"]));
        for s in [ghostty, herdr] {
            assert_eq!(s.input, b"j");
            assert!(s.replies.kitty && s.replies.status);
            assert_eq!(crate::input::typeahead_keys(&s.input), [KeyCode::Char('j')]);
        }
    }

    #[test]
    fn user_csi_sequences_are_input_not_replies() {
        // Arrow up, Delete, and a DSR-looking-but-different `CSI 6 n`.
        let s = split_replies(&cat(&[b"\x1b[A", b"\x1b[3~", DSR]));
        assert_eq!(s.input, b"\x1b[A\x1b[3~");
        assert!(s.replies.status);
    }

    #[test]
    fn lone_escape_and_alt_key_are_input() {
        let s = split_replies(&cat(&[DSR, b"\x1bj"]));
        assert_eq!(s.input, b"\x1bj");
    }

    #[test]
    fn incomplete_sequence_at_the_end_is_partial_and_held_back() {
        let s = split_replies(b"j\x1b_Gi=31;O");
        assert!(s.partial);
        assert_eq!(s.input, b"j");
        assert!(!s.replies.kitty_answered);

        let s = split_replies(b"j\x1b[?62");
        assert!(s.partial);
        assert_eq!(s.input, b"j");

        // A trailing Esc may be the start of a reply, too.
        assert!(split_replies(b"\x1b").partial);
    }

    #[test]
    fn finish_keeps_a_trailing_esc_key_and_drops_other_fragments() {
        let s = split_replies(&cat(&[DSR, b"\x1b"])).finish();
        assert_eq!(s.input, b"\x1b");
        assert!(!s.partial);
        // No reply at all, only Esc pressed.
        assert_eq!(split_replies(b"\x1b").finish().input, b"\x1b");
        // A reply cut off by the deadline is not turned into keys.
        assert_eq!(split_replies(b"j\x1b[?62").finish().input, b"j");
    }

    #[test]
    fn kitty_error_counts_as_answered_but_not_supported() {
        let s = split_replies(b"\x1b_Gi=31;ENOTSUPPORTED:\x1b\\");
        assert!(s.replies.kitty_answered);
        assert!(!s.replies.kitty);
    }

    #[test]
    fn da1_without_sixel_and_other_strings_are_consumed() {
        let s = split_replies(b"\x1b[?62;22c\x1b]11;rgb:0/0/0\x07\x1bP1$r0m\x1b\\");
        assert!(!s.replies.sixel);
        assert!(s.input.is_empty());
        // Attribute 42 is not 4.
        assert!(!split_replies(b"\x1b[?62;42c").replies.sixel);
    }

    #[test]
    fn protocol_choice_prefers_kitty_then_sixel() {
        let mut r = Replies {
            sixel: true,
            ..Replies::default()
        };
        assert_eq!(choose_protocol(&r), Some(ProtocolType::Sixel));
        r.kitty = true;
        assert_eq!(choose_protocol(&r), Some(ProtocolType::Kitty));
        assert_eq!(choose_protocol(&Replies::default()), None);
    }

    #[cfg(unix)]
    #[test]
    fn query_is_done_after_dsr_and_kitty_or_its_grace_period() {
        use std::time::Duration;
        let dsr_only = split_replies(DSR);
        assert!(!query_done(&dsr_only, true, Some(Duration::ZERO)));
        assert!(query_done(&dsr_only, true, Some(KITTY_GRACE)));
        assert!(query_done(&dsr_only, false, Some(Duration::ZERO)));
        assert!(query_done(
            &split_replies(&cat(&[KITTY_OK, DSR])),
            true,
            Some(Duration::ZERO)
        ));
        // Nothing yet, or a reply still coming in.
        assert!(!query_done(&split_replies(b""), true, None));
        let partial = split_replies(&cat(&[DSR, b"\x1b_Gi=31"]));
        assert!(!query_done(&partial, true, Some(KITTY_GRACE)));
        // A trailing Esc after the DSR reply does not hold things up.
        let esc = split_replies(&cat(&[KITTY_OK, DSR, b"\x1b"]));
        assert!(query_done(&esc, true, Some(Duration::ZERO)));
    }

    #[test]
    fn protocol_parsing() {
        assert_eq!(parse_protocol("Kitty"), Some(ProtocolType::Kitty));
        assert_eq!(parse_protocol(" sixel "), Some(ProtocolType::Sixel));
        assert_eq!(parse_protocol("bogus"), None);
    }
}
