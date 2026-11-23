//! Image / video preview support: decoding, ffmpeg/ffprobe helpers, and
//! opening files in the OS default application.

use std::{
    fs, io,
    path::Path,
    process::{Command, Stdio},
};

use image::{DynamicImage, ImageReader};
use ratatui_image::{picker::Picker, protocol::StatefulProtocol};

/// Longest edge (in pixels) kept in memory for previews.
const MAX_PREVIEW_EDGE: u32 = 2048;

/// A preview shown in the pane opposite to the active one.
pub struct Preview {
    pub title: String,
    /// Encoded image ready for rendering (None if no image could be produced).
    pub image: Option<StatefulProtocol>,
    /// Text lines shown below the image (metadata or error messages).
    pub info: Vec<String>,
}

fn ext_lower(path: &Path) -> Option<String> {
    path.extension()
        .and_then(|e| e.to_str())
        .map(str::to_lowercase)
}

pub fn is_image(path: &Path) -> bool {
    matches!(
        ext_lower(path).as_deref(),
        Some(
            "png"
                | "jpg"
                | "jpeg"
                | "gif"
                | "bmp"
                | "tiff"
                | "tif"
                | "webp"
                | "heic"
                | "heif"
                | "avif"
        )
    )
}

pub fn is_video(path: &Path) -> bool {
    matches!(
        ext_lower(path).as_deref(),
        Some(
            "mp4"
                | "m4v"
                | "mov"
                | "mkv"
                | "webm"
                | "avi"
                | "wmv"
                | "flv"
                | "mts"
                | "m2ts"
                | "3gp"
                | "mpg"
                | "mpeg"
        )
    )
}

fn file_name(path: &Path) -> String {
    path.file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default()
}

fn human_size(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut v = bytes as f64;
    let mut i = 0;
    while v >= 1024.0 && i < UNITS.len() - 1 {
        v /= 1024.0;
        i += 1;
    }
    if i == 0 {
        format!("{bytes} B")
    } else {
        format!("{v:.1} {}", UNITS[i])
    }
}

fn shrink(img: DynamicImage) -> DynamicImage {
    if img.width() > MAX_PREVIEW_EDGE || img.height() > MAX_PREVIEW_EDGE {
        img.thumbnail(MAX_PREVIEW_EDGE, MAX_PREVIEW_EDGE)
    } else {
        img
    }
}

/// Extract a single frame with ffmpeg (works for videos and formats the
/// `image` crate can't decode, e.g. HEIC).
fn ffmpeg_frame(path: &Path, seek_secs: Option<f64>) -> io::Result<DynamicImage> {
    let mut cmd = Command::new("ffmpeg");
    cmd.args(["-v", "error", "-nostdin"]);
    if let Some(t) = seek_secs {
        cmd.args(["-ss", &format!("{t:.3}")]);
    }
    let out = cmd
        .arg("-i")
        .arg(path)
        .args([
            "-frames:v",
            "1",
            "-vf",
            &format!("scale='min({MAX_PREVIEW_EDGE},iw)':-2"),
            "-f",
            "image2pipe",
            "-c:v",
            "png",
            "-",
        ])
        .stdin(Stdio::null())
        .stderr(Stdio::piped())
        .output()?;
    if !out.status.success() || out.stdout.is_empty() {
        let msg = String::from_utf8_lossy(&out.stderr);
        let first = msg.lines().next().unwrap_or("ffmpeg failed").to_string();
        return Err(io::Error::other(first));
    }
    image::load_from_memory(&out.stdout).map_err(io::Error::other)
}

fn decode_image(path: &Path) -> io::Result<DynamicImage> {
    let native = ImageReader::open(path)
        .and_then(|r| r.with_guessed_format())
        .map_err(io::Error::other)
        .and_then(|r| r.decode().map_err(io::Error::other));
    match native {
        Ok(img) => Ok(img),
        // Fall back to ffmpeg for formats like HEIC/AVIF.
        Err(e) => ffmpeg_frame(path, None).map_err(|_| e),
    }
}

pub fn load_image_preview(picker: &Picker, path: &Path) -> Preview {
    let size = fs::metadata(path).map(|m| m.len()).unwrap_or(0);
    let format = ext_lower(path).unwrap_or_default().to_uppercase();
    match decode_image(path) {
        Ok(img) => {
            let info = vec![format!(
                "{}x{}  {}  {}",
                img.width(),
                img.height(),
                format,
                human_size(size)
            )];
            Preview {
                title: file_name(path),
                image: Some(picker.new_resize_protocol(shrink(img))),
                info,
            }
        }
        Err(e) => Preview {
            title: file_name(path),
            image: None,
            info: vec![format!("Cannot decode image: {e}")],
        },
    }
}

/// Metadata gathered from ffprobe.
#[derive(Default)]
struct ProbeInfo {
    duration: Option<f64>,
    lines: Vec<String>,
}

fn kv(line: &str) -> Vec<(&str, &str)> {
    line.split('|').filter_map(|p| p.split_once('=')).collect()
}

fn get<'a>(pairs: &[(&'a str, &'a str)], key: &str) -> Option<&'a str> {
    pairs
        .iter()
        .find(|(k, _)| *k == key)
        .map(|(_, v)| *v)
        .filter(|v| !v.is_empty() && *v != "N/A")
}

fn format_duration(secs: f64) -> String {
    let total = secs.round() as u64;
    format!(
        "{:02}:{:02}:{:02}",
        total / 3600,
        (total / 60) % 60,
        total % 60
    )
}

fn fps(rate: &str) -> Option<f64> {
    let (n, d) = rate.split_once('/')?;
    let (n, d): (f64, f64) = (n.parse().ok()?, d.parse().ok()?);
    (d > 0.0 && n > 0.0).then(|| n / d)
}

fn probe(path: &Path) -> io::Result<ProbeInfo> {
    let out = Command::new("ffprobe")
        .args([
            "-v",
            "error",
            "-show_entries",
            "format=duration,format_long_name:stream=codec_type,codec_name,width,height,r_frame_rate,sample_rate,channels",
            "-of",
            "compact=p=0",
        ])
        .arg(path)
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()?;
    if !out.status.success() {
        return Err(io::Error::other("ffprobe failed"));
    }
    let text = String::from_utf8_lossy(&out.stdout);
    let mut info = ProbeInfo::default();
    let mut streams = Vec::new();
    let mut format_name = None;
    for line in text.lines() {
        let pairs = kv(line);
        match get(&pairs, "codec_type") {
            Some("video") => {
                let mut s = format!("Video: {}", get(&pairs, "codec_name").unwrap_or("?"));
                if let (Some(w), Some(h)) = (get(&pairs, "width"), get(&pairs, "height")) {
                    s.push_str(&format!("  {w}x{h}"));
                }
                if let Some(f) = get(&pairs, "r_frame_rate").and_then(fps) {
                    s.push_str(&format!("  {f:.2}fps"));
                }
                streams.push(s);
            }
            Some("audio") => {
                let mut s = format!("Audio: {}", get(&pairs, "codec_name").unwrap_or("?"));
                if let Some(r) = get(&pairs, "sample_rate") {
                    s.push_str(&format!("  {r}Hz"));
                }
                if let Some(c) = get(&pairs, "channels") {
                    s.push_str(&format!("  {c}ch"));
                }
                streams.push(s);
            }
            Some(_) => {}
            None => {
                info.duration = get(&pairs, "duration").and_then(|d| d.parse().ok());
                format_name = get(&pairs, "format_long_name").map(str::to_string);
            }
        }
    }
    if let Some(d) = info.duration {
        info.lines.push(format!("Duration: {}", format_duration(d)));
    }
    info.lines.extend(streams);
    if let Some(f) = format_name {
        info.lines.push(format!("Format: {f}"));
    }
    Ok(info)
}

pub fn load_video_preview(picker: &Picker, path: &Path) -> Preview {
    let size = fs::metadata(path).map(|m| m.len()).unwrap_or(0);
    let mut info = Vec::new();
    let probe = probe(path);
    let duration = match &probe {
        Ok(p) => {
            info.extend(p.lines.iter().cloned());
            p.duration
        }
        Err(_) => {
            info.push("ffprobe not available (brew install ffmpeg)".to_string());
            None
        }
    };
    info.push(format!("Size: {}", human_size(size)));

    // Grab a frame ~10% in (max 10s) to avoid black intro frames.
    let seek = duration.map(|d| (d * 0.1).min(10.0));
    let image = match ffmpeg_frame(path, seek) {
        Ok(img) => Some(picker.new_resize_protocol(img)),
        Err(e) => {
            info.push(format!("No thumbnail: {e}"));
            None
        }
    };
    Preview {
        title: file_name(path),
        image,
        info,
    }
}

/// Open a file or directory with the OS default application.
pub fn open_external(path: &Path) -> io::Result<()> {
    #[cfg(target_os = "macos")]
    let opener = "open";
    #[cfg(not(target_os = "macos"))]
    let opener = "xdg-open";
    Command::new(opener)
        .arg(path)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map(|mut child| {
            // Reap the opener so it doesn't linger as a zombie.
            std::thread::spawn(move || {
                let _ = child.wait();
            });
        })
}
