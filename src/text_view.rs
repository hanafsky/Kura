//! Shared text renderer: turns file contents into display lines
//! (ratatui [`Line`] / [`Span`]) and draws them with wrapping and relative
//! line numbers.
//!
//! Used by the full-screen text viewer (`Enter`, `D`) and by the text
//! preview in the opposite pane (`P`). How the lines are produced is chosen
//! by a [`Renderer`]; Markdown is rendered by [`crate::markdown`].
//!
//! Language detection ([`detect_syntax`]), syntect → span conversion
//! ([`to_span`]), wrapping ([`wrap_line`], [`wrap_hanging`]) and row
//! selection ([`visible_rows`]) are pure functions.

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
    /// Rendered Markdown (headings, lists, tables, …).
    Markdown,
}

/// Text ready to draw: one [`Line`] per source line (per paragraph, list
/// item, table row, … for rendered Markdown).
pub struct TextDoc {
    pub title: String,
    pub renderer: Renderer,
    pub lines: Vec<Line<'static>>,
    /// Prefix of the wrapped continuation rows of each line (Markdown list
    /// indents and quote bars), indexed like `lines`. Empty when no line
    /// has one; rows then wrap character by character as in source code.
    pub hangs: Vec<Line<'static>>,
    /// Markdown source, kept to switch between the rendered and the source
    /// view (`m`). `None` for other files.
    markdown_source: Option<String>,
}

impl TextDoc {
    /// A file's contents: rendered Markdown for `.md` / `.markdown`,
    /// otherwise highlighted when its language is known.
    pub fn for_file(title: String, path: &Path, text: &str) -> Self {
        if is_markdown(path) && !looks_binary(text) {
            return Self::markdown(title, text);
        }
        let first_line = text.lines().next().unwrap_or("");
        match detect_syntax(path, first_line) {
            Some(syntax) if !looks_binary(text) => Self::new(
                title,
                Renderer::Syntax(syntax.name.clone()),
                highlight(text, syntax),
            ),
            _ => Self::plain(title, text),
        }
    }

    fn new(title: String, renderer: Renderer, lines: Vec<Line<'static>>) -> Self {
        Self {
            title,
            renderer,
            lines,
            hangs: Vec::new(),
            markdown_source: None,
        }
    }

    pub fn plain(title: String, text: &str) -> Self {
        let lines = text.lines().map(|l| Line::raw(l.to_string())).collect();
        Self::new(title, Renderer::Plain, lines)
    }

    /// Markdown shown rendered; [`TextDoc::toggle_markdown`] switches to
    /// the source.
    pub fn markdown(title: String, text: &str) -> Self {
        let rendered = crate::markdown::render(text);
        let mut doc = Self::new(title, Renderer::Markdown, rendered.lines);
        doc.hangs = rendered.hangs;
        doc.markdown_source = Some(text.to_string());
        doc
    }

    /// Whether this is a Markdown file (rendered or shown as source).
    pub fn is_markdown(&self) -> bool {
        self.markdown_source.is_some()
    }

    /// Whether a Markdown file is shown as its (highlighted) source.
    pub fn shows_markdown_source(&self) -> bool {
        self.is_markdown() && self.renderer != Renderer::Markdown
    }

    /// Switch a Markdown file between the rendered and the source view.
    /// Returns false (and does nothing) for other documents.
    pub fn toggle_markdown(&mut self) -> bool {
        if !self.is_markdown() {
            return false;
        }
        self.show_markdown_source(!self.shows_markdown_source());
        true
    }

    /// Show a Markdown file rendered (`false`) or as source (`true`).
    /// Does nothing for other documents.
    pub fn show_markdown_source(&mut self, source: bool) {
        if source == self.shows_markdown_source() {
            return;
        }
        let Some(text) = self.markdown_source.take() else {
            return;
        };
        if source {
            match syntax_set().find_syntax_by_extension("md") {
                Some(syntax) => {
                    self.renderer = Renderer::Syntax(syntax.name.clone());
                    self.lines = highlight(&text, syntax);
                }
                None => {
                    self.renderer = Renderer::Plain;
                    self.lines = text.lines().map(|l| Line::raw(l.to_string())).collect();
                }
            }
            self.hangs = Vec::new();
        } else {
            let rendered = crate::markdown::render(&text);
            self.renderer = Renderer::Markdown;
            self.lines = rendered.lines;
            self.hangs = rendered.hangs;
        }
        self.markdown_source = Some(text);
    }

    /// `git diff` output coloured by line kind.
    pub fn diff(title: String, text: &str) -> Self {
        let lines = text
            .lines()
            .zip(diff::classify_lines(text))
            .map(|(l, kind)| Line::styled(l.to_string(), diff_style(kind)))
            .collect();
        Self::new(title, Renderer::Diff, lines)
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

/// Whether `path` is shown as rendered Markdown (`.md`, `.markdown`).
pub fn is_markdown(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| e.eq_ignore_ascii_case("md") || e.eq_ignore_ascii_case("markdown"))
}

/// Highlight a Markdown code block whose info string names `lang`
/// (`rust`, `py`, `sh`, …); unknown or missing languages stay plain.
pub fn highlight_code(code: &str, lang: &str) -> Vec<Line<'static>> {
    let ss = syntax_set();
    match ss
        .find_syntax_by_token(lang)
        .filter(|s| !lang.is_empty() && s.name != "Plain Text")
    {
        Some(syntax) => highlight(code, syntax),
        None => code.lines().map(|l| Line::raw(l.to_string())).collect(),
    }
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
#[cfg(test)]
pub fn wrap_line(line: &Line, width: usize) -> Vec<Line<'static>> {
    wrap_hanging(line, width, None)
}

/// [`wrap_line`] for rendered Markdown: rows after the first start with
/// `hang` (list indent, quote bar) and break between words where possible
/// (after a space, or before / after a wide CJK character), dropping the
/// spaces at the break. Without `hang` it is [`wrap_line`].
pub fn wrap_hanging(line: &Line, width: usize, hang: Option<&Line>) -> Vec<Line<'static>> {
    let cells: Vec<(char, Style)> = line
        .spans
        .iter()
        .flat_map(|span| {
            let style = line.style.patch(span.style);
            span.content.chars().map(move |ch| (ch, style))
        })
        .collect();
    let widths: Vec<usize> = cells.iter().map(|(ch, _)| char_width(*ch)).collect();
    let hang_width = hang.map_or(0, |h| h.width());
    let word = hang.is_some();
    let rest_width = if hang_width < width {
        width - hang_width
    } else {
        width
    };
    let rows = split_rows(&cells, &widths, width, rest_width, word);
    rows.into_iter()
        .enumerate()
        .map(|(i, range)| {
            let mut spans: Vec<Span<'static>> = Vec::new();
            if i > 0 && hang_width < width {
                if let Some(hang) = hang {
                    spans.extend(
                        hang.spans.iter().map(|s| {
                            Span::styled(s.content.to_string(), hang.style.patch(s.style))
                        }),
                    );
                }
            }
            let mut piece = String::new();
            let mut piece_style = Style::default();
            for &(ch, style) in &cells[range] {
                if style != piece_style && !piece.is_empty() {
                    spans.push(Span::styled(std::mem::take(&mut piece), piece_style));
                }
                piece_style = style;
                piece.push(ch);
            }
            if !piece.is_empty() {
                spans.push(Span::styled(piece, piece_style));
            }
            Line::from(spans)
        })
        .collect()
}

/// Move a row break at `end` back so that Japanese closing punctuation
/// (`、` `。` `）` `」` …) does not start the next row and an opening
/// bracket (`（` `「` …) does not end this one. Gives up after a few
/// characters (a run of punctuation), keeping at least one per row.
fn kinsoku(cells: &[(char, Style)], start: usize, end: usize) -> usize {
    const NO_START: &str =
        "、。，．,.）)］]｝}」』】〕〉》！？!?：；ー～…・ゃゅょっぁぃぅぇぉャュョッァィゥェォ";
    const NO_END: &str = "（(［[｛{「『【〔〈《";
    let bad = |e: usize| NO_START.contains(cells[e].0) || NO_END.contains(cells[e - 1].0);
    let mut e = end;
    for _ in 0..3 {
        if e <= start + 1 || e >= cells.len() {
            return end;
        }
        if !bad(e) {
            return e;
        }
        e -= 1;
    }
    end
}

fn char_width(ch: char) -> usize {
    ch.width().unwrap_or(0)
}

/// Index ranges of `cells` that make up each display row: the first row
/// holds at most `first` columns, the others `rest`. With `word`, rows
/// break at the last space or wide character that fits and continuation
/// rows skip leading spaces; otherwise they break at any character.
fn split_rows(
    cells: &[(char, Style)],
    widths: &[usize],
    first: usize,
    rest: usize,
    word: bool,
) -> Vec<std::ops::Range<usize>> {
    let is_space = |i: usize| cells[i].0 == ' ';
    let mut rows = Vec::new();
    let mut start = 0;
    let mut row_width = 0;
    // Where the current row could end at a word boundary.
    let mut boundary: Option<usize> = None;
    let mut i = 0;
    while i < cells.len() {
        if word && i == start && !rows.is_empty() && is_space(i) {
            start += 1;
            i += 1;
            continue;
        }
        let limit = if rows.is_empty() { first } else { rest };
        let cw = widths[i];
        if row_width + cw > limit && row_width > 0 {
            // Width of the word that would move to the next row.
            let word_width = |b: usize| {
                (b..cells.len())
                    .take_while(|&j| !is_space(j) && widths[j] <= 1)
                    .take(rest + 1)
                    .map(|j| widths[j])
                    .sum::<usize>()
            };
            let end = match boundary {
                // A space or a wide character can itself start the next row;
                // a word too long for any row is cut where it is.
                Some(b)
                    if word && b > start && !is_space(i) && cw <= 1 && word_width(b) <= rest =>
                {
                    b
                }
                _ => i,
            };
            let end = if word {
                kinsoku(cells, start, end)
            } else {
                end
            };
            let mut trimmed = end;
            while word && trimmed > start + 1 && is_space(trimmed - 1) {
                trimmed -= 1;
            }
            rows.push(start..trimmed);
            start = end;
            i = end;
            row_width = 0;
            boundary = None;
            continue;
        }
        row_width += cw;
        if is_space(i) || cw > 1 {
            boundary = Some(i + 1);
        } else if i > start && widths[i - 1] > 1 {
            // Break allowed between a wide character and narrow text.
            boundary = Some(i);
        }
        i += 1;
    }
    if start < cells.len() || rows.is_empty() {
        rows.push(start..cells.len());
    }
    rows
}

/// Rows to show for `doc` in an area of `width` × `height` cells scrolled by
/// `offset` display rows: wrapped content with a relative line number
/// (the count to type before `j`) in front of each row.
pub fn visible_rows(doc: &TextDoc, width: u16, height: u16, offset: usize) -> Vec<Line<'static>> {
    let height = height as usize;
    let number_width = height.to_string().len().max(1);
    let text_width = (width as usize).saturating_sub(number_width + 1);
    let wrap = |i: usize| wrap_hanging(&doc.lines[i], text_width, doc.hangs.get(i));
    // Only the lines on screen are wrapped, so scrolling stays fast in
    // large files: skip whole lines by counting their rows first.
    let mut skip = offset;
    let mut first = doc.lines.len();
    for (i, line) in doc.lines.iter().enumerate() {
        let n = row_count_hanging(line, text_width, doc.hangs.get(i));
        if skip < n {
            first = i;
            break;
        }
        skip -= n;
    }
    let mut rows = Vec::new();
    for i in first..doc.lines.len() {
        rows.extend(wrap(i));
        if rows.len() >= skip + height {
            break;
        }
    }
    let mut rows: Vec<Line> = rows.into_iter().skip(skip).take(height).collect();
    if rows.len() < height {
        // Scrolled past the end: show the last full page.
        let mut tail: Vec<Line> = Vec::new();
        for i in (0..doc.lines.len()).rev() {
            let mut wrapped = wrap(i);
            wrapped.append(&mut tail);
            tail = wrapped;
            if tail.len() >= height {
                break;
            }
        }
        rows = tail.split_off(tail.len().saturating_sub(height));
    }
    rows.into_iter()
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

/// Number of rows [`wrap_line`] makes of `line`, without building them.
#[cfg(test)]
pub fn row_count(line: &Line, width: usize) -> usize {
    row_count_hanging(line, width, None)
}

/// Number of rows [`wrap_hanging`] makes of `line`.
pub fn row_count_hanging(line: &Line, width: usize, hang: Option<&Line>) -> usize {
    // A character is never wider than its UTF-8 length, so a line with no
    // more bytes than columns fits in one row (the common case).
    let bytes: usize = line.spans.iter().map(|s| s.content.len()).sum();
    if bytes <= width {
        return 1;
    }
    if hang.is_some() {
        return wrap_hanging(line, width, hang).len();
    }
    let mut rows = 1;
    let mut row_width = 0;
    for ch in line.spans.iter().flat_map(|s| s.content.chars()) {
        let cw = char_width(ch);
        if row_width + cw > width && row_width > 0 {
            rows += 1;
            row_width = 0;
        }
        row_width += cw;
    }
    rows
}

/// Draw `doc` inside `block`, scrolled by `offset` display rows. The
/// highlighted language is named at the bottom left of the border.
pub fn draw(f: &mut Frame, area: Rect, block: Block, doc: &TextDoc, offset: usize) {
    let block = match bottom_label(doc) {
        Some(label) => block.title_bottom(Line::from(label).left_aligned()),
        None => block,
    };
    let inner = block.inner(area);
    let rows = visible_rows(doc, inner.width, inner.height, offset);
    f.render_widget(Paragraph::new(rows).block(block), area);
}

/// Label at the bottom left of the border: the highlighted language, or
/// for Markdown which view is shown.
fn bottom_label(doc: &TextDoc) -> Option<String> {
    match &doc.renderer {
        Renderer::Markdown => Some(" Markdown ".to_string()),
        Renderer::Syntax(_) | Renderer::Plain if doc.is_markdown() => {
            Some(" Markdown source ".to_string())
        }
        Renderer::Syntax(name) => Some(format!(" {name} ")),
        _ => None,
    }
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
    /// Markdown files are shown as source instead of rendered (`m`); kept
    /// while the cursor moves to other files.
    markdown_source: bool,
}

impl PanePreview {
    pub fn new() -> Self {
        Self {
            source: None,
            content: PreviewContent::Message(String::new()),
            markdown_source: false,
        }
    }

    /// `m`: switch a Markdown preview between rendered and source, without
    /// reading the file again. Returns whether the source is now shown, or
    /// `None` when the preview is not showing Markdown.
    pub fn toggle_markdown(&mut self) -> Option<bool> {
        let PreviewContent::Text(doc) = &mut self.content else {
            return None;
        };
        if !doc.toggle_markdown() {
            return None;
        }
        self.markdown_source = doc.shows_markdown_source();
        Some(self.markdown_source)
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
            if let PreviewContent::Text(doc) = &mut self.content {
                doc.show_markdown_source(self.markdown_source);
            }
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

/// Load the start of `path` for the pane preview. A read error is returned
/// as `Err` with the status-line message.
pub fn load_preview(path: &Path) -> Result<PreviewContent, String> {
    use std::io::Read;
    let title = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    if path.is_dir() {
        return Ok(PreviewContent::Message("Directory".to_string()));
    }
    let mut bytes = Vec::new();
    let read = std::fs::File::open(path).and_then(|f| {
        f.take(PREVIEW_MAX_BYTES as u64)
            .read_to_end(&mut bytes)
            .map(|_| ())
    });
    if let Err(e) = read {
        return Err(crate::app::fs_error_message(&e, path));
    }
    Ok(match decode_prefix(&bytes) {
        Some(text) => PreviewContent::Text(TextDoc::for_file(title, path, text)),
        None => PreviewContent::Message("Binary file".to_string()),
    })
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
    fn row_count_matches_wrap_line() {
        let lines = [
            Line::raw(""),
            Line::raw("abc"),
            Line::raw("abcdefghij"),
            Line::raw("日本語のテキスト"),
            Line::from(vec![Span::raw("ab"), Span::raw("日本"), Span::raw("cdefg")]),
        ];
        for line in &lines {
            for width in 0..12 {
                assert_eq!(
                    row_count(line, width),
                    wrap_line(line, width).len(),
                    "{line} at {width}"
                );
            }
        }
    }

    #[test]
    fn wrap_hanging_breaks_between_words_and_indents_continuations() {
        let hang = Line::raw("  ");
        let wrap = |text: &str, width| {
            wrap_hanging(&Line::raw(text.to_string()), width, Some(&hang))
                .iter()
                .map(text_of)
                .collect::<Vec<_>>()
        };
        assert_eq!(
            wrap("- alpha beta gamma", 10),
            ["- alpha", "  beta", "  gamma"]
        );
        // A word longer than the row is cut.
        assert_eq!(wrap("- abcdefghijk", 8), ["- abcdef", "  ghijk"]);
        // Japanese breaks between any two characters, and before / after
        // wide text inside English.
        assert_eq!(wrap("- 日本語の文章", 8), ["- 日本語", "  の文章"]);
        assert_eq!(wrap("- ab日本語", 7), ["- ab日", "  本語"]);
        // Styles survive the break.
        let red = Style::default().fg(Color::Red);
        let line = Line::from(vec![Span::raw("- "), Span::styled("red words", red)]);
        let rows = wrap_hanging(&line, 7, Some(&hang));
        assert_eq!(rows[1].spans[1].style, red);
        assert_eq!(rows[1].spans[0].content, "  ");
        // Closing punctuation does not start a row, opening brackets do
        // not end one.
        assert_eq!(wrap("- あいう。えお", 8), ["- あい", "  う。え", "  お"]);
        assert_eq!(wrap("- あい（う）", 8), ["- あい", "  （う）"]);
        // A hang wider than the area is dropped.
        assert_eq!(wrap("abcdef", 2), ["ab", "cd", "ef"]);
    }

    #[test]
    fn row_count_hanging_matches_wrap_hanging() {
        let hang = Line::raw("│ ");
        let lines = [
            Line::raw("│ quoted words that wrap"),
            Line::raw("│ 日本語の引用文がここに続きます"),
            Line::raw("│ mixed 日本語 and English words"),
            Line::raw("    leading spaces stay"),
        ];
        for line in &lines {
            for width in 0..20 {
                assert_eq!(
                    row_count_hanging(line, width, Some(&hang)),
                    wrap_hanging(line, width, Some(&hang)).len(),
                    "{line} at {width}"
                );
            }
        }
    }

    #[test]
    fn markdown_files_render_and_switch_to_source() {
        let mut doc = TextDoc::for_file("r.md".into(), Path::new("R.MD"), "# T\n\n- a\n");
        assert_eq!(doc.renderer, Renderer::Markdown);
        assert!(doc.is_markdown() && !doc.shows_markdown_source());
        assert_eq!(doc.lines.len(), doc.hangs.len());
        assert_eq!(text_of(&doc.lines[2]), "• a");
        assert_eq!(bottom_label(&doc).unwrap(), " Markdown ");
        assert!(doc.toggle_markdown());
        assert_eq!(doc.renderer, Renderer::Syntax("Markdown".into()));
        assert!(doc.hangs.is_empty());
        assert_eq!(text_of(&doc.lines[2]), "- a");
        assert_eq!(bottom_label(&doc).unwrap(), " Markdown source ");
        let mut plain = TextDoc::plain("t".into(), "x\n");
        assert!(!plain.toggle_markdown());
        assert!(is_markdown(Path::new("a.markdown")));
        assert!(!is_markdown(Path::new("a.mdx")));
    }

    #[test]
    fn visible_rows_wraps_markdown_with_hanging_indent() {
        let doc = TextDoc::markdown("m".into(), "- one two three\n");
        let rows = visible_rows(&doc, 10, 3, 0);
        assert_eq!(
            rows.iter().map(text_of).collect::<Vec<_>>(),
            ["0 • one", "1   two", "2   three"]
        );
        let rows = visible_rows(&doc, 10, 2, 1);
        assert_eq!(
            rows.iter().map(text_of).collect::<Vec<_>>(),
            ["0   two", "1   three"]
        );
    }

    #[test]
    fn visible_rows_scrolls_by_wrapped_rows() {
        // Width 6 leaves 4 columns for text after "0 " / "1 " numbers.
        let doc = TextDoc::plain("t".into(), "abcdefgh\nij\nkl\n");
        let texts = |off| {
            visible_rows(&doc, 6, 2, off)
                .iter()
                .map(text_of)
                .collect::<Vec<_>>()
        };
        assert_eq!(texts(0), ["0 abcd", "1 efgh"]);
        assert_eq!(texts(1), ["0 efgh", "1 ij"]);
        assert_eq!(texts(2), ["0 ij", "1 kl"]);
        assert_eq!(texts(50), ["0 ij", "1 kl"]);
        let short = TextDoc::plain("t".into(), "x\n");
        let rows = visible_rows(&short, 6, 3, 5);
        assert_eq!(rows.iter().map(text_of).collect::<Vec<_>>(), ["0 x"]);
        // Offsets beyond u16 work (files with more than 65,535 rows).
        let big = TextDoc::plain("t".into(), &"y\n".repeat(70_000));
        let rows = visible_rows(&big, 10, 1, 69_999);
        assert_eq!(rows.iter().map(text_of).collect::<Vec<_>>(), ["0 y"]);
        let rows = visible_rows(&short, 6, 3, 5);
        assert_eq!(rows.iter().map(text_of).collect::<Vec<_>>(), ["0 x"]);
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
                load_preview(path).unwrap()
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
        assert!(
            matches!(load_preview(dir.path()).unwrap(), PreviewContent::Message(m) if m == "Directory")
        );
        let bin = dir.path().join("x.bin");
        std::fs::write(&bin, [0u8, 159, 146, 150]).unwrap();
        assert!(
            matches!(load_preview(&bin).unwrap(), PreviewContent::Message(m) if m == "Binary file")
        );
        let py = dir.path().join("tool");
        std::fs::write(&py, "#!/usr/bin/env python3\nprint(1)\n").unwrap();
        assert!(load_preview(&dir.path().join("missing")).is_err());
        assert!(matches!(
            load_preview(&py).unwrap(),
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
