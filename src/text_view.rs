//! Shared text renderer: turns file contents into display lines
//! (ratatui [`Line`] / [`Span`]) and draws them with wrapping and relative
//! line numbers.
//!
//! Used by the full-screen text viewer (`Enter`, `D`) and by the text
//! preview in the opposite pane (`P`). How the lines are produced is chosen
//! by a [`Renderer`]; new renderers (e.g. Markdown) are added there.
//!
//! Language detection ([`detect_syntax`]), syntect → span conversion
//! ([`to_span`]), wrapping ([`wrap_line`]) and row selection
//! ([`visible_rows`]) are pure functions.

use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::time::SystemTime;

use ratatui::{
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Paragraph},
    Frame,
};
use syntect::{
    easy::HighlightLines,
    highlighting::{FontStyle, Theme, ThemeSet},
    parsing::{SyntaxReference, SyntaxSet},
    util::LinesWithEndings,
};
use unicode_width::UnicodeWidthChar;

use crate::diff::{self, LineKind};

/// Only the first this many lines are highlighted; the rest is shown plain
/// so that large files open without a noticeable pause.
pub const HIGHLIGHT_MAX_LINES: usize = 2000;
/// Highlighting stops at the first line longer than this (minified files
/// make the highlighter very slow); that line and the rest are plain.
const HIGHLIGHT_MAX_LINE_BYTES: usize = 4096;
/// Default colour theme. Only its foreground colours are used, so the
/// terminal's own background stays visible.
const THEME: &str = "base16-ocean.dark";

/// How a document's lines were produced.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Renderer {
    /// Unstyled text.
    Plain,
    /// Syntax highlighting with the named syntect syntax.
    Syntax(String),
    /// Unified diff coloured by line kind (`D`).
    Diff,
}

/// Text ready to draw: one [`Line`] per source line.
pub struct TextDoc {
    pub title: String,
    pub renderer: Renderer,
    pub lines: Vec<Line<'static>>,
}

impl TextDoc {
    /// A file's contents, highlighted when its language is known.
    pub fn for_file(title: String, path: &Path, text: &str) -> Self {
        let first_line = text.lines().next().unwrap_or("");
        match detect_syntax(path, first_line) {
            Some(syntax) if !looks_binary(text) => Self {
                title,
                renderer: Renderer::Syntax(syntax.name.clone()),
                lines: highlight(text, syntax),
            },
            _ => Self::plain(title, text),
        }
    }

    pub fn plain(title: String, text: &str) -> Self {
        Self {
            title,
            renderer: Renderer::Plain,
            lines: text.lines().map(|l| Line::raw(l.to_string())).collect(),
        }
    }

    /// `git diff` output coloured by line kind.
    pub fn diff(title: String, text: &str) -> Self {
        let lines = text
            .lines()
            .zip(diff::classify_lines(text))
            .map(|(l, kind)| Line::styled(l.to_string(), diff_style(kind)))
            .collect();
        Self {
            title,
            renderer: Renderer::Diff,
            lines,
        }
    }

    /// Number of source lines.
    pub fn line_count(&self) -> usize {
        self.lines.len()
    }
}

/// Colour of a diff line.
fn diff_style(kind: LineKind) -> Style {
    match kind {
        LineKind::Header => Style::default().add_modifier(Modifier::BOLD),
        LineKind::Hunk => Style::default().fg(Color::Cyan),
        LineKind::Added => Style::default().fg(Color::Green),
        LineKind::Removed => Style::default().fg(Color::Red),
        LineKind::Context => Style::default(),
    }
}

fn syntax_set() -> &'static SyntaxSet {
    static SET: OnceLock<SyntaxSet> = OnceLock::new();
    SET.get_or_init(SyntaxSet::load_defaults_newlines)
}

fn theme() -> &'static Theme {
    static THEME_CELL: OnceLock<Theme> = OnceLock::new();
    THEME_CELL.get_or_init(|| {
        let mut themes = ThemeSet::load_defaults().themes;
        themes.remove(THEME).expect("bundled syntect theme")
    })
}

/// Language of a file from its name / extension, or else from its first
/// line (shebang, `<?xml`, modelines). `None` for plain text.
pub fn detect_syntax(path: &Path, first_line: &str) -> Option<&'static SyntaxReference> {
    let ss = syntax_set();
    let by_name = path
        .file_name()
        .and_then(|n| n.to_str())
        .and_then(|n| ss.find_syntax_by_extension(n));
    let by_ext = || {
        path.extension()
            .and_then(|e| e.to_str())
            .and_then(|e| ss.find_syntax_by_extension(e))
    };
    by_name
        .or_else(by_ext)
        .or_else(|| ss.find_syntax_by_first_line(first_line))
        .filter(|s| s.name != "Plain Text")
}

/// Text with NUL bytes is not highlighted (UTF-8 that is really binary).
fn looks_binary(text: &str) -> bool {
    text.contains('\0')
}

/// Highlight `text` with `syntax`: the first [`HIGHLIGHT_MAX_LINES`] lines
/// get colours, the rest is plain.
fn highlight(text: &str, syntax: &SyntaxReference) -> Vec<Line<'static>> {
    let theme = theme();
    let default_fg = theme.settings.foreground;
    let mut h = HighlightLines::new(syntax, theme);
    let mut lines = Vec::new();
    let mut highlighting = true;
    for (i, line) in LinesWithEndings::from(text).enumerate() {
        if i >= HIGHLIGHT_MAX_LINES || line.len() > HIGHLIGHT_MAX_LINE_BYTES {
            highlighting = false;
        }
        let ranges = if highlighting {
            h.highlight_line(line, syntax_set()).ok()
        } else {
            None
        };
        lines.push(match ranges {
            Some(ranges) => Line::from(
                ranges
                    .into_iter()
                    .map(|(style, s)| to_span(style, s, default_fg))
                    .filter(|s| !s.content.is_empty())
                    .collect::<Vec<_>>(),
            ),
            None => Line::raw(strip_newline(line).to_string()),
        });
    }
    lines
}

fn strip_newline(s: &str) -> &str {
    s.strip_suffix('\n')
        .map(|s| s.strip_suffix('\r').unwrap_or(s))
        .unwrap_or(s)
}

/// Convert one highlighted piece to a span. Only the foreground is kept
/// (the terminal background shows through); the theme's default text
/// colour becomes the terminal's own, and the line ending is dropped.
pub fn to_span(
    style: syntect::highlighting::Style,
    text: &str,
    default_fg: Option<syntect::highlighting::Color>,
) -> Span<'static> {
    let mut out = Style::default();
    let fg = style.foreground;
    if fg.a > 0 && Some(fg) != default_fg {
        out = out.fg(Color::Rgb(fg.r, fg.g, fg.b));
    }
    if style.font_style.contains(FontStyle::BOLD) {
        out = out.add_modifier(Modifier::BOLD);
    }
    if style.font_style.contains(FontStyle::ITALIC) {
        out = out.add_modifier(Modifier::ITALIC);
    }
    if style.font_style.contains(FontStyle::UNDERLINE) {
        out = out.add_modifier(Modifier::UNDERLINED);
    }
    Span::styled(strip_newline(text).to_string(), out)
}

/// Split a line into display rows of at most `width` columns, keeping each
/// span's style. A character wider than `width` gets a row of its own.
pub fn wrap_line(line: &Line, width: usize) -> Vec<Line<'static>> {
    let mut rows = Vec::new();
    let mut row: Vec<Span<'static>> = Vec::new();
    let mut row_width = 0;
    for span in &line.spans {
        let style = line.style.patch(span.style);
        let mut piece = String::new();
        for ch in span.content.chars() {
            let cw = ch.width().unwrap_or(0);
            if row_width + cw > width && row_width > 0 {
                if !piece.is_empty() {
                    row.push(Span::styled(std::mem::take(&mut piece), style));
                }
                rows.push(Line::from(std::mem::take(&mut row)));
                row_width = 0;
            }
            piece.push(ch);
            row_width += cw;
        }
        if !piece.is_empty() {
            row.push(Span::styled(piece, style));
        }
    }
    if !row.is_empty() || rows.is_empty() {
        rows.push(Line::from(row));
    }
    rows
}

/// Rows to show for `doc` in an area of `width` × `height` cells scrolled by
/// `offset` display rows: wrapped content with a relative line number
/// (the count to type before `j`) in front of each row.
pub fn visible_rows(doc: &TextDoc, width: u16, height: u16, offset: u16) -> Vec<Line<'static>> {
    let height = height as usize;
    let number_width = height.to_string().len().max(1);
    let text_width = (width as usize).saturating_sub(number_width + 1);
    let rows: Vec<Line> = doc
        .lines
        .iter()
        .flat_map(|l| wrap_line(l, text_width))
        .collect();
    let max_off = rows.len().saturating_sub(height);
    let start = (offset as usize).min(max_off);
    rows.into_iter()
        .skip(start)
        .take(height)
        .enumerate()
        .map(|(i, row)| {
            let mut spans = vec![Span::styled(
                format!("{i:>number_width$} "),
                Style::default().fg(Color::DarkGray),
            )];
            spans.extend(row.spans);
            Line::from(spans)
        })
        .collect()
}

/// Draw `doc` inside `block`, scrolled by `offset` display rows. The
/// highlighted language is named at the bottom left of the border.
pub fn draw(f: &mut Frame, area: Rect, block: Block, doc: &TextDoc, offset: u16) {
    let block = match &doc.renderer {
        Renderer::Syntax(name) => {
            block.title_bottom(Line::from(format!(" {name} ")).left_aligned())
        }
        _ => block,
    };
    let inner = block.inner(area);
    let rows = visible_rows(doc, inner.width, inner.height, offset);
    f.render_widget(Paragraph::new(rows).block(block), area);
}

/// Bytes read for the pane preview (`P`); longer files are cut here.
pub const PREVIEW_MAX_BYTES: usize = 256 * 1024;

/// What the pane preview shows for the entry under the cursor.
pub enum PreviewContent {
    Text(TextDoc),
    /// Not previewable as text; the reason is shown instead.
    Message(String),
}

/// Text preview in the pane opposite to the active one (`P`). It follows
/// the cursor; the last loaded file is cached until the cursor moves to
/// another entry or the file changes on disk.
pub struct PanePreview {
    /// Entry the content was loaded for, with its modification time.
    source: Option<(PathBuf, Option<SystemTime>)>,
    pub content: PreviewContent,
}

impl PanePreview {
    pub fn new() -> Self {
        Self {
            source: None,
            content: PreviewContent::Message(String::new()),
        }
    }

    /// Show `path` (`None`: empty pane), reloading only when it changed.
    /// `load` reads the entry; it is a parameter so tests can count calls.
    pub fn sync(&mut self, path: Option<&Path>, load: impl FnOnce(&Path) -> PreviewContent) {
        let Some(path) = path else {
            self.source = None;
            self.content = PreviewContent::Message(String::new());
            return;
        };
        let mtime = std::fs::metadata(path).and_then(|m| m.modified()).ok();
        let key = (path.to_path_buf(), mtime);
        if self.source.as_ref() != Some(&key) {
            self.content = load(path);
            self.source = Some(key);
        }
    }
}

/// Decode the start of a file read for the preview. A multi-byte character
/// cut at the end is dropped; any other invalid UTF-8 or a NUL byte means
/// the file is binary (`None`).
pub fn decode_prefix(bytes: &[u8]) -> Option<&str> {
    let text = match std::str::from_utf8(bytes) {
        Ok(s) => s,
        Err(e) if e.error_len().is_none() => {
            std::str::from_utf8(&bytes[..e.valid_up_to()]).expect("valid prefix")
        }
        Err(_) => return None,
    };
    (!looks_binary(text)).then_some(text)
}

/// Load the start of `path` for the pane preview.
pub fn load_preview(path: &Path) -> PreviewContent {
    use std::io::Read;
    let title = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    if path.is_dir() {
        return PreviewContent::Message("Directory".to_string());
    }
    let mut bytes = Vec::new();
    let read = std::fs::File::open(path).and_then(|f| {
        f.take(PREVIEW_MAX_BYTES as u64)
            .read_to_end(&mut bytes)
            .map(|_| ())
    });
    if let Err(e) = read {
        return PreviewContent::Message(crate::app::fs_error_message(&e, path));
    }
    match decode_prefix(&bytes) {
        Some(text) => PreviewContent::Text(TextDoc::for_file(title, path, text)),
        None => PreviewContent::Message("Binary file".to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn name(path: &str, first: &str) -> Option<String> {
        detect_syntax(&PathBuf::from(path), first).map(|s| s.name.clone())
    }

    fn text_of(line: &Line) -> String {
        line.spans.iter().map(|s| s.content.as_ref()).collect()
    }

    #[test]
    fn detects_language_from_extension_name_and_shebang() {
        assert_eq!(name("src/main.rs", "").as_deref(), Some("Rust"));
        assert_eq!(name("a.py", "").as_deref(), Some("Python"));
        assert_eq!(name("Makefile", "").as_deref(), Some("Makefile"));
        assert_eq!(
            name("run", "#!/usr/bin/env python3").as_deref(),
            Some("Python")
        );
        assert_eq!(
            name("run", "#!/bin/bash").as_deref(),
            Some("Bourne Again Shell (bash)")
        );
        assert_eq!(name("notes.txt", "hello"), None);
        assert_eq!(name("README", "hello"), None);
    }

    #[test]
    fn to_span_keeps_foreground_and_font_style_only() {
        use syntect::highlighting::{Color as C, Style as S};
        let red = C {
            r: 200,
            g: 10,
            b: 20,
            a: 255,
        };
        let style = S {
            foreground: red,
            background: C::WHITE,
            font_style: FontStyle::BOLD | FontStyle::ITALIC,
        };
        let span = to_span(style, "fn\n", None);
        assert_eq!(span.content, "fn");
        assert_eq!(span.style.fg, Some(Color::Rgb(200, 10, 20)));
        assert_eq!(span.style.bg, None);
        assert!(span.style.add_modifier.contains(Modifier::BOLD));
        assert!(span.style.add_modifier.contains(Modifier::ITALIC));
        // The theme's default text colour is left to the terminal.
        let plain = to_span(style, "x\r\n", Some(red));
        assert_eq!(plain.content, "x");
        assert_eq!(plain.style.fg, None);
    }

    #[test]
    fn highlighting_colours_code_and_keeps_text() {
        let doc = TextDoc::for_file(
            "a.rs".into(),
            Path::new("a.rs"),
            "fn main() {\n    // hi\n}\n",
        );
        assert_eq!(doc.renderer, Renderer::Syntax("Rust".into()));
        assert_eq!(doc.line_count(), 3);
        assert_eq!(text_of(&doc.lines[0]), "fn main() {");
        assert_eq!(text_of(&doc.lines[1]), "    // hi");
        assert!(doc.lines[0].spans.iter().any(|s| s.style.fg.is_some()));
    }

    #[test]
    fn only_the_first_lines_are_highlighted() {
        let text = "let x = 1;\n".repeat(HIGHLIGHT_MAX_LINES + 5);
        let doc = TextDoc::for_file("a.rs".into(), Path::new("a.rs"), &text);
        assert_eq!(doc.line_count(), HIGHLIGHT_MAX_LINES + 5);
        assert!(doc.lines[0].spans.len() > 1);
        let last = &doc.lines[HIGHLIGHT_MAX_LINES + 1];
        assert_eq!(last.spans.len(), 1);
        assert_eq!(last.spans[0].style, Style::default());
    }

    #[test]
    fn unknown_and_nul_text_stays_plain() {
        let doc = TextDoc::for_file("a.txt".into(), Path::new("a.txt"), "hello\n");
        assert_eq!(doc.renderer, Renderer::Plain);
        let doc = TextDoc::for_file("a.rs".into(), Path::new("a.rs"), "fn\0x\n");
        assert_eq!(doc.renderer, Renderer::Plain);
    }

    #[test]
    fn diff_lines_are_coloured_by_kind() {
        let doc = TextDoc::diff("d".into(), "@@ -1 +1 @@\n-a\n+b\n c\n");
        let fgs: Vec<_> = doc.lines.iter().map(|l| l.style.fg).collect();
        assert_eq!(
            fgs,
            [
                Some(Color::Cyan),
                Some(Color::Red),
                Some(Color::Green),
                None
            ]
        );
    }

    #[test]
    fn wrap_line_splits_spans_by_display_width() {
        let red = Style::default().fg(Color::Red);
        let line = Line::from(vec![Span::styled("abcd", red), Span::raw("日本語")]);
        let rows = wrap_line(&line, 5);
        let texts: Vec<String> = rows.iter().map(text_of).collect();
        // "abcd" + "日" would be 6 columns, so "日" moves to the next row.
        assert_eq!(texts, ["abcd", "日本", "語"]);
        assert_eq!(rows[0].spans[0].style, red);
        assert_eq!(rows[1].spans[0].style, Style::default());

        let rows = wrap_line(&Line::from(vec![Span::styled("abcdef", red)]), 4);
        assert_eq!(rows.iter().map(text_of).collect::<Vec<_>>(), ["abcd", "ef"]);
        assert!(rows.iter().all(|r| r.spans[0].style == red));
    }

    #[test]
    fn wrap_line_edge_cases() {
        // Empty line: one empty row.
        assert_eq!(wrap_line(&Line::raw(""), 10).len(), 1);
        // A wide character in a 1-column area still gets shown.
        let rows = wrap_line(&Line::raw("日本"), 1);
        assert_eq!(rows.iter().map(text_of).collect::<Vec<_>>(), ["日", "本"]);
        // Line-level style (diff colours) is kept on the wrapped rows.
        let rows = wrap_line(&Line::styled("abc", Style::default().fg(Color::Red)), 2);
        assert!(rows.iter().all(|r| r.spans[0].style.fg == Some(Color::Red)));
    }

    #[test]
    fn visible_rows_numbers_relative_rows_and_clamps_offset() {
        let doc = TextDoc::plain("t".into(), "a\nb\nc\nd\n");
        let rows = visible_rows(&doc, 10, 2, 1);
        assert_eq!(rows.iter().map(text_of).collect::<Vec<_>>(), ["0 b", "1 c"]);
        // Scrolling past the end stops at the last full page.
        let rows = visible_rows(&doc, 10, 2, 99);
        assert_eq!(rows.iter().map(text_of).collect::<Vec<_>>(), ["0 c", "1 d"]);
    }

    #[test]
    fn pane_preview_reloads_only_when_the_entry_changes() {
        let dir = tempfile::tempdir().unwrap();
        let a = dir.path().join("a.rs");
        let b = dir.path().join("b.txt");
        std::fs::write(&a, "fn a() {}\n").unwrap();
        std::fs::write(&b, "b\n").unwrap();
        let mut p = PanePreview::new();
        let mut loads = 0;
        for path in [&a, &a, &b, &b, &a] {
            p.sync(Some(path), |path| {
                loads += 1;
                load_preview(path)
            });
        }
        assert_eq!(loads, 3);
        assert!(matches!(&p.content, PreviewContent::Text(d) if d.title == "a.rs"));
        p.sync(None, |_| unreachable!());
        assert!(matches!(&p.content, PreviewContent::Message(m) if m.is_empty()));
    }

    #[test]
    fn load_preview_reports_directories_and_binary_files() {
        let dir = tempfile::tempdir().unwrap();
        assert!(matches!(load_preview(dir.path()), PreviewContent::Message(m) if m == "Directory"));
        let bin = dir.path().join("x.bin");
        std::fs::write(&bin, [0u8, 159, 146, 150]).unwrap();
        assert!(matches!(load_preview(&bin), PreviewContent::Message(m) if m == "Binary file"));
        let py = dir.path().join("tool");
        std::fs::write(&py, "#!/usr/bin/env python3\nprint(1)\n").unwrap();
        assert!(matches!(
            load_preview(&py),
            PreviewContent::Text(d) if d.renderer == Renderer::Syntax("Python".into())
        ));
    }

    #[test]
    fn decode_prefix_handles_cut_characters_and_binary() {
        assert_eq!(decode_prefix(b"abc"), Some("abc"));
        let cut = &"あ".as_bytes()[..2];
        let bytes = [b"ok".as_slice(), cut].concat();
        assert_eq!(decode_prefix(&bytes), Some("ok"));
        assert_eq!(decode_prefix(&[0xff, b'a', b'b']), None);
        assert_eq!(decode_prefix(b"a\0b"), None);
    }
}
