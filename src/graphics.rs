//! Terminal graphics protocol detection.
//!
//! `ratatui-image`'s `Picker::from_query_stdio` stops reading replies as soon
//! as the Device Status Report (`ESC[0n`) arrives. Some multiplexers (e.g.
//! herdr) answer that immediately but forward the Kitty graphics reply a bit
//! later, so Kitty support is missed and the leftover reply bytes end up being
//! read as key presses. We re-probe for Kitty in that case, and also allow an
//! explicit override via `KURA_IMAGE_PROTOCOL`.

use ratatui_image::picker::{Capability, Picker, ProtocolType};

/// Environment variable to force a protocol: kitty | sixel | iterm2 | halfblocks.
pub const PROTOCOL_ENV: &str = "KURA_IMAGE_PROTOCOL";

/// Detect the graphics protocol. Must be called after raw mode is enabled and
/// before the event loop starts reading input.
/// Returns the picker and a short description for the UI.
pub fn detect() -> (Picker, String) {
    let (mut picker, mut note) = match Picker::from_query_stdio() {
        Ok(p) => (p, None),
        Err(e) => (Picker::halfblocks(), Some(format!("query failed: {e:?}"))),
    };

    if let Some(forced) = std::env::var(PROTOCOL_ENV).ok().and_then(|v| parse_protocol(&v)) {
        picker.set_protocol_type(forced);
        note = Some(format!("forced by {PROTOCOL_ENV}"));
    } else if picker.protocol_type() == ProtocolType::Halfblocks && has_cell_size(&picker) {
        // Also drains any late replies left over from the first query.
        if probe_kitty_again() {
            picker.set_protocol_type(ProtocolType::Kitty);
            note = Some("re-probed".to_string());
        }
    }

    let fs = picker.font_size();
    let mut info = format!("{:?} {}x{}px", picker.protocol_type(), fs.width, fs.height);
    if let Some(n) = note {
        info.push_str(&format!(" ({n})"));
    }
    (picker, info)
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

/// Only switch to a pixel protocol if the terminal reported its cell size;
/// otherwise images would be scaled with a made-up font size.
fn has_cell_size(picker: &Picker) -> bool {
    picker
        .capabilities()
        .iter()
        .any(|c| matches!(c, Capability::CellSize(Some(_))))
}

#[cfg(unix)]
fn probe_kitty_again() -> bool {
    use std::io::Write;
    use std::time::{Duration, Instant};

    // Kitty graphics query followed by DA1, which every terminal answers.
    let query = b"\x1b_Gi=31,s=1,v=1,a=q,t=d,f=24;AAAA\x1b\\\x1b[c";
    let mut out = std::io::stdout();
    if out.write_all(query).and_then(|_| out.flush()).is_err() {
        return false;
    }

    let deadline = Instant::now() + Duration::from_millis(1000);
    let idle = Duration::from_millis(150);
    let mut buf = Vec::new();
    let mut seen_da1_at: Option<Instant> = None;
    loop {
        let now = Instant::now();
        if now >= deadline {
            break;
        }
        // After a DA1 reply, keep reading briefly to collect any stragglers.
        if let Some(t) = seen_da1_at {
            if now.duration_since(t) >= idle {
                break;
            }
        }
        let wait = deadline.saturating_duration_since(now).min(idle);
        let mut pfd = libc::pollfd {
            fd: libc::STDIN_FILENO,
            events: libc::POLLIN,
            revents: 0,
        };
        let ready = unsafe { libc::poll(&mut pfd, 1, wait.as_millis() as libc::c_int) };
        if ready < 0 {
            break;
        }
        if ready == 0 || pfd.revents & libc::POLLIN == 0 {
            continue;
        }
        let mut chunk = [0u8; 256];
        let n = unsafe { libc::read(libc::STDIN_FILENO, chunk.as_mut_ptr().cast(), chunk.len()) };
        if n <= 0 {
            break;
        }
        buf.extend_from_slice(&chunk[..n as usize]);
        if contains_da1(&buf) {
            seen_da1_at.get_or_insert_with(Instant::now);
        }
    }
    contains(&buf, b"\x1b_Gi=31;OK")
}

#[cfg(not(unix))]
fn probe_kitty_again() -> bool {
    false
}

fn contains(hay: &[u8], needle: &[u8]) -> bool {
    hay.windows(needle.len()).any(|w| w == needle)
}

/// DA1 reply looks like `ESC [ ? <digits;...> c`.
fn contains_da1(buf: &[u8]) -> bool {
    buf.windows(3).enumerate().any(|(i, w)| {
        w == b"\x1b[?"
            && buf[i + 3..]
                .iter()
                .position(|&b| !(b.is_ascii_digit() || b == b';'))
                .is_some_and(|p| buf[i + 3 + p] == b'c')
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn da1_detection() {
        assert!(contains_da1(b"\x1b_Gi=31;OK\x1b\\\x1b[?62;22c"));
        assert!(!contains_da1(b"\x1b[6;35;16t\x1b[0n"));
        assert!(!contains_da1(b"\x1b[?62;22")); // incomplete
    }

    #[test]
    fn protocol_parsing() {
        assert_eq!(parse_protocol("Kitty"), Some(ProtocolType::Kitty));
        assert_eq!(parse_protocol(" sixel "), Some(ProtocolType::Sixel));
        assert_eq!(parse_protocol("bogus"), None);
    }
}
