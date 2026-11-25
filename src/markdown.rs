//! Markdown → display lines for the text viewer and the pane preview.
//!
//! [`render`] is a pure function: it parses CommonMark (plus tables,
//! strikethrough and task lists) with `pulldown-cmark` and returns one
//! [`Line`] per paragraph, heading, list item, table row, code line, …
//! together with the prefix that wrapped continuation rows start with
//! (list indent, quote bar). Wrapping itself happens at draw time in
//! [`crate::text_view::wrap_hanging`], so the result does not depend on
//! the pane width.
//!
//! Look:
//! - Headings keep their `#` marks (dimmed) and are bold; levels 1–3 are
//!   coloured (magenta + underline, cyan, green).
//! - List items get `•` / `◦` / `▪` by depth or their number, two columns
//!   of indent per level, and `[x]` / `[ ]` for task items.
//! - Quotes get a `│ ` bar per level.
//! - Code blocks keep their dimmed fences and are highlighted by the info
//!   string's language (as in the source view).
//! - Tables are drawn with box lines, columns as wide as their widest cell
//!   (by display width, so Japanese text lines up) and aligned as declared.
//! - Links are underlined blue, followed by the dimmed URL unless the text
//!   is the URL itself; images show `[image: alt]` and the URL.
//! - Inline code is yellow, horizontal rules are a dimmed line.

use pulldown_cmark::{Alignment, CodeBlockKind, Event, HeadingLevel, Options, Parser, Tag, TagEnd};
use ratatui::{
    style::{Color, Modifier, Style},
    text::{Line, Span},
};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use crate::text_view::highlight_code;

/// Width of a horizontal rule (`---`) in columns.
pub const RULE_WIDTH: usize = 40;

/// Rendered Markdown: display lines and, for each, the prefix of its
/// wrapped continuation rows.
#[derive(Default)]
pub struct Rendered {
    pub lines: Vec<Line<'static>>,
    pub hangs: Vec<Line<'static>>,
}

/// Render Markdown `src` to display lines.
pub fn render(src: &str) -> Rendered {
    let options =
        Options::ENABLE_TABLES | Options::ENABLE_STRIKETHROUGH | Options::ENABLE_TASKLISTS;
    let mut r = Renderer::default();
    for event in Parser::new_ext(src, options) {
        r.event(event);
    }
    r.flush_inline();
    r.out
}

fn dim() -> Style {
    Style::default().fg(Color::DarkGray)
}

fn heading_style(level: HeadingLevel) -> Style {
    let bold = Style::default().add_modifier(Modifier::BOLD);
    match level {
        HeadingLevel::H1 => bold.fg(Color::Magenta).add_modifier(Modifier::UNDERLINED),
        HeadingLevel::H2 => bold.fg(Color::Cyan),
        HeadingLevel::H3 => bold.fg(Color::Green),
        _ => bold,
    }
}

fn link_style() -> Style {
    Style::default()
        .fg(Color::Blue)
        .add_modifier(Modifier::UNDERLINED)
}

/// Bullet of an unordered list item at nesting `depth` (1 = top level).
fn bullet(depth: usize) -> &'static str {
    ["•", "◦", "▪"][(depth.max(1) - 1) % 3]
}

/// A block that prefixes the lines inside it: a quote or a list item.
struct Container {
    /// Prefix of the first line (the item marker).
    first: Vec<Span<'static>>,
    /// Prefix of every later line (indent or quote bar).
    rest: Vec<Span<'static>>,
    /// Whether the first line has been written.
    used: bool,
}

/// Table being collected; drawn when it ends.
struct Table {
    aligns: Vec<Alignment>,
    /// Rows of cells; each cell is its inline spans.
    rows: Vec<Vec<Vec<Span<'static>>>>,
    /// Number of header rows at the top.
    head: usize,
}

/// An open list.
struct List {
    /// Number of the next item (`None` for bullets).
    next: Option<u64>,
    /// Items are separated by blank lines (they contain paragraphs).
    loose: bool,
}

/// Link or image being collected.
struct LinkState {
    url: String,
    /// Index in `inline` where the link text starts.
    start: usize,
    image: bool,
}

#[derive(Default)]
struct Renderer {
    out: Rendered,
    containers: Vec<Container>,
    /// Inline content of the block being built.
    inline: Vec<Span<'static>>,
    /// Inline styles in effect (emphasis, strong, link, …), innermost last.
    styles: Vec<Style>,
    /// Open lists, innermost last.
    lists: Vec<List>,
    links: Vec<LinkState>,
    /// Open code block: language and collected text.
    code: Option<(String, String)>,
    table: Option<Table>,
    /// A blank line goes before the next block.
    need_blank: bool,
    /// A soft line break is waiting for the next text.
    soft_break: bool,
}

impl Renderer {
    fn event(&mut self, event: Event) {
        match event {
            Event::Start(tag) => self.start(tag),
            Event::End(tag) => self.end(tag),
            Event::Text(text) => match &mut self.code {
                Some((_, code)) => code.push_str(&text),
                None => self.push_text(&text, self.style()),
            },
            Event::Code(text) => self.push_text(&text, self.style().fg(Color::Yellow)),
            Event::InlineMath(text) | Event::DisplayMath(text) => {
                self.push_text(&text, self.style().fg(Color::Yellow))
            }
            Event::Html(html) => {
                for line in html.lines() {
                    self.inline.push(Span::styled(line.to_string(), dim()));
                    self.flush_inline();
                }
            }
            Event::InlineHtml(html) => self.push_text(&html, dim()),
            Event::FootnoteReference(label) => self.push_text(&format!("[^{label}]"), dim()),
            Event::SoftBreak => self.soft_break = true,
            Event::HardBreak => self.flush_inline(),
            Event::Rule => {
                self.start_block();
                self.inline
                    .push(Span::styled("─".repeat(RULE_WIDTH), dim()));
                self.flush_inline();
                self.need_blank = true;
            }
            Event::TaskListMarker(done) => {
                let (mark, style) = if done {
                    ("[x] ", Style::default().fg(Color::Green))
                } else {
                    ("[ ] ", dim())
                };
                self.inline.push(Span::styled(mark, style));
            }
        }
    }

    fn start(&mut self, tag: Tag) {
        match tag {
            Tag::Paragraph => {
                // Only loose list items wrap their text in paragraphs.
                if let Some(list) = self.lists.last_mut() {
                    list.loose = true;
                }
                self.start_block();
            }
            Tag::HtmlBlock | Tag::MetadataBlock(_) => self.start_block(),
            Tag::Heading { level, .. } => {
                self.start_block();
                self.styles.push(heading_style(level));
            }
            Tag::BlockQuote(_) => {
                self.start_block();
                let bar = vec![Span::styled("│ ", dim())];
                self.containers.push(Container {
                    first: bar.clone(),
                    rest: bar,
                    used: false,
                });
            }
            Tag::CodeBlock(kind) => {
                self.start_block();
                let lang = match kind {
                    CodeBlockKind::Fenced(info) => {
                        info.split_whitespace().next().unwrap_or("").to_string()
                    }
                    CodeBlockKind::Indented => String::new(),
                };
                self.code = Some((lang, String::new()));
            }
            Tag::List(first) => {
                self.start_block();
                self.lists.push(List {
                    next: first,
                    loose: false,
                });
            }
            Tag::Item => {
                self.flush_inline();
                let depth = self.lists.len();
                let marker = match self.lists.last_mut().and_then(|l| l.next.as_mut()) {
                    Some(n) => {
                        let m = format!("{n}. ");
                        *n += 1;
                        m
                    }
                    _ => format!("{} ", bullet(depth)),
                };
                let indent = " ".repeat(marker.width());
                self.containers.push(Container {
                    first: vec![Span::styled(marker, Style::default().fg(Color::Cyan))],
                    rest: vec![Span::raw(indent)],
                    used: false,
                });
            }
            Tag::Table(aligns) => {
                self.start_block();
                self.table = Some(Table {
                    aligns,
                    rows: Vec::new(),
                    head: 0,
                });
            }
            Tag::TableHead | Tag::TableRow => {
                if let Some(table) = &mut self.table {
                    table.rows.push(Vec::new());
                }
            }
            Tag::TableCell => self.inline.clear(),
            Tag::Emphasis => self.push_style(Style::default().add_modifier(Modifier::ITALIC)),
            Tag::Strong => self.push_style(Style::default().add_modifier(Modifier::BOLD)),
            Tag::Strikethrough => {
                self.push_style(Style::default().add_modifier(Modifier::CROSSED_OUT))
            }
            Tag::Link { dest_url, .. } => {
                self.links.push(LinkState {
                    url: dest_url.to_string(),
                    start: self.inline.len(),
                    image: false,
                });
                self.push_style(link_style());
            }
            Tag::Image { dest_url, .. } => {
                self.push_text("[image: ", Style::default().fg(Color::Magenta));
                self.links.push(LinkState {
                    url: dest_url.to_string(),
                    start: self.inline.len(),
                    image: true,
                });
                self.push_style(Style::default().fg(Color::Magenta));
            }
            Tag::FootnoteDefinition(label) => {
                self.start_block();
                self.push_text(&format!("[^{label}]: "), dim());
            }
            _ => {}
        }
    }

    fn end(&mut self, tag: TagEnd) {
        match tag {
            TagEnd::Paragraph | TagEnd::HtmlBlock | TagEnd::MetadataBlock(_) => self.end_block(),
            TagEnd::FootnoteDefinition => self.end_block(),
            TagEnd::Heading(level) => {
                self.styles.pop();
                let marks = format!("{} ", "#".repeat(level as usize));
                self.inline.insert(0, Span::styled(marks, dim()));
                self.end_block();
            }
            TagEnd::BlockQuote(_) => {
                self.flush_inline();
                self.containers.pop();
                self.need_blank = true;
            }
            TagEnd::CodeBlock => {
                if let Some((lang, code)) = self.code.take() {
                    self.code_block(&lang, &code);
                }
                self.need_blank = true;
            }
            TagEnd::List(_) => {
                self.flush_inline();
                self.lists.pop();
                // A nested list continues its item: a gap follows only in a
                // loose list (or after a top-level list).
                if self.lists.last().is_none_or(|l| l.loose) {
                    self.need_blank = true;
                }
            }
            TagEnd::Item => {
                self.flush_inline();
                if self.containers.last().is_some_and(|c| !c.used) {
                    // Empty item: still show its marker.
                    self.emit(Vec::new());
                }
                self.containers.pop();
            }
            TagEnd::Table => {
                if let Some(table) = self.table.take() {
                    self.table_lines(table);
                }
                self.need_blank = true;
            }
            TagEnd::TableHead => {
                if let Some(table) = &mut self.table {
                    table.head = table.rows.len();
                }
            }
            TagEnd::TableCell => {
                let cell = std::mem::take(&mut self.inline);
                self.soft_break = false;
                if let Some(row) = self.table.as_mut().and_then(|t| t.rows.last_mut()) {
                    row.push(cell);
                }
            }
            TagEnd::Emphasis | TagEnd::Strong | TagEnd::Strikethrough => {
                self.styles.pop();
            }
            TagEnd::Link | TagEnd::Image => {
                self.styles.pop();
                if let Some(link) = self.links.pop() {
                    self.end_link(link);
                }
            }
            _ => {}
        }
    }

    /// Style of inline text: all open styles combined.
    fn style(&self) -> Style {
        self.styles
            .iter()
            .fold(Style::default(), |acc, s| acc.patch(*s))
    }

    fn push_style(&mut self, style: Style) {
        let combined = self.style().patch(style);
        self.styles.push(combined);
    }

    /// Append inline text. A pending soft line break becomes a space,
    /// except between two wide (CJK) characters, where Japanese text has
    /// no space.
    fn push_text(&mut self, text: &str, style: Style) {
        if text.is_empty() {
            return;
        }
        if std::mem::take(&mut self.soft_break) {
            let before = self.inline.last().and_then(|s| s.content.chars().last());
            let after = text.chars().next();
            let wide = |c: Option<char>| c.and_then(|c| c.width()).is_some_and(|w| w > 1);
            if !(wide(before) && wide(after)) && before.is_some() {
                self.inline.push(Span::styled(" ", self.style()));
            }
        }
        self.inline.push(Span::styled(text.to_string(), style));
    }

    fn end_link(&mut self, link: LinkState) {
        let text: String = self.inline[link.start.min(self.inline.len())..]
            .iter()
            .map(|s| s.content.as_ref())
            .collect();
        if link.image {
            self.inline
                .push(Span::styled("]", Style::default().fg(Color::Magenta)));
        }
        let url = link.url;
        let same = url == text || url.strip_prefix("mailto:") == Some(text.as_str());
        if !url.is_empty() && (link.image || !same) {
            self.inline.push(Span::styled(format!(" ({url})"), dim()));
        }
    }

    /// Before a block: write pending inline text, then a blank line if the
    /// previous block asked for one.
    fn start_block(&mut self) {
        self.flush_inline();
        if std::mem::take(&mut self.need_blank) && !self.out.lines.is_empty() {
            let prefix = self.rest_prefix();
            let bar: Vec<Span<'static>> = trim_end(prefix);
            self.out.lines.push(Line::from(bar));
            self.out.hangs.push(Line::default());
        }
    }

    fn end_block(&mut self) {
        self.flush_inline();
        self.need_blank = true;
    }

    /// Write the inline content collected so far as one line.
    fn flush_inline(&mut self) {
        self.soft_break = false;
        if self.inline.is_empty() {
            return;
        }
        let content = std::mem::take(&mut self.inline);
        self.emit(content);
    }

    /// Write one line: container prefixes, then `content`.
    fn emit(&mut self, content: Vec<Span<'static>>) {
        let mut spans = Vec::new();
        for c in &mut self.containers {
            spans.extend(if c.used {
                c.rest.clone()
            } else {
                c.first.clone()
            });
            c.used = true;
        }
        spans.extend(content);
        let hang = self.rest_prefix();
        self.out.lines.push(Line::from(spans));
        self.out.hangs.push(Line::from(hang));
    }

    fn rest_prefix(&self) -> Vec<Span<'static>> {
        self.containers
            .iter()
            .flat_map(|c| c.rest.iter().cloned())
            .collect()
    }

    fn code_block(&mut self, lang: &str, code: &str) {
        self.emit(vec![Span::styled(format!("```{lang}"), dim())]);
        for line in highlight_code(code, lang) {
            self.emit(line.spans);
        }
        self.emit(vec![Span::styled("```", dim())]);
    }

    fn table_lines(&mut self, table: Table) {
        let columns = table
            .rows
            .iter()
            .map(Vec::len)
            .chain([table.aligns.len()])
            .max()
            .unwrap_or(0);
        if columns == 0 {
            return;
        }
        let mut widths = vec![1; columns];
        for row in &table.rows {
            for (i, cell) in row.iter().enumerate() {
                widths[i] = widths[i].max(spans_width(cell));
            }
        }
        let border = |left: &str, mid: &str, right: &str| {
            let parts: Vec<String> = widths.iter().map(|w| "─".repeat(w + 2)).collect();
            vec![Span::styled(
                format!("{left}{}{right}", parts.join(mid)),
                dim(),
            )]
        };
        self.emit(border("┌", "┬", "┐"));
        for (r, row) in table.rows.iter().enumerate() {
            if r == table.head && r > 0 {
                self.emit(border("├", "┼", "┤"));
            }
            let mut spans = vec![Span::styled("│", dim())];
            for (i, width) in widths.iter().enumerate() {
                let cell = row.get(i).cloned().unwrap_or_default();
                let pad = width - spans_width(&cell);
                let (left, right) = match table.aligns.get(i) {
                    Some(Alignment::Right) => (pad, 0),
                    Some(Alignment::Center) => (pad / 2, pad - pad / 2),
                    _ => (0, pad),
                };
                spans.push(Span::raw(" ".repeat(left + 1)));
                let bold = r < table.head;
                spans.extend(cell.into_iter().map(|s| {
                    if bold {
                        let style = s.style.add_modifier(Modifier::BOLD);
                        s.style(style)
                    } else {
                        s
                    }
                }));
                spans.push(Span::raw(" ".repeat(right + 1)));
                spans.push(Span::styled("│", dim()));
            }
            self.emit(spans);
        }
        self.emit(border("└", "┴", "┘"));
    }
}

fn spans_width(spans: &[Span]) -> usize {
    spans.iter().map(|s| s.content.width()).sum()
}

/// Drop trailing spaces from a prefix (the blank line inside a quote shows
/// just the bar).
fn trim_end(mut spans: Vec<Span<'static>>) -> Vec<Span<'static>> {
    while let Some(last) = spans.pop() {
        let trimmed = last.content.trim_end().to_string();
        if !trimmed.is_empty() {
            spans.push(Span::styled(trimmed, last.style));
            break;
        }
    }
    spans
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::text_view::wrap_hanging;

    fn text_of(line: &Line) -> String {
        line.spans.iter().map(|s| s.content.as_ref()).collect()
    }

    fn texts(src: &str) -> Vec<String> {
        render(src).lines.iter().map(text_of).collect()
    }

    /// Rows of the rendered document wrapped at `width`.
    fn wrapped(src: &str, width: usize) -> Vec<String> {
        let r = render(src);
        r.lines
            .iter()
            .zip(&r.hangs)
            .flat_map(|(l, h)| wrap_hanging(l, width, Some(h)))
            .map(|l| text_of(&l))
            .collect()
    }

    fn span_with<'a>(line: &'a Line, text: &str) -> &'a Span<'a> {
        line.spans
            .iter()
            .find(|s| s.content == text)
            .unwrap_or_else(|| panic!("no span {text:?} in {line:?}"))
    }

    #[test]
    fn headings_keep_marks_and_are_styled_by_level() {
        let r = render("# Title\n\n## Sub\n\n#### Deep\n");
        let t: Vec<String> = r.lines.iter().map(text_of).collect();
        assert_eq!(t, ["# Title", "", "## Sub", "", "#### Deep"]);
        let h1 = span_with(&r.lines[0], "Title").style;
        assert_eq!(h1.fg, Some(Color::Magenta));
        assert!(h1
            .add_modifier
            .contains(Modifier::BOLD | Modifier::UNDERLINED));
        assert_eq!(span_with(&r.lines[2], "Sub").style.fg, Some(Color::Cyan));
        let h4 = span_with(&r.lines[4], "Deep").style;
        assert_eq!(h4.fg, None);
        assert!(h4.add_modifier.contains(Modifier::BOLD));
        assert_eq!(span_with(&r.lines[0], "# ").style, dim());
    }

    #[test]
    fn inline_styles_nest() {
        let r = render("a *it **both*** ~~gone~~ `code`\n");
        let line = &r.lines[0];
        assert_eq!(text_of(line), "a it both gone code");
        assert!(span_with(line, "it ")
            .style
            .add_modifier
            .contains(Modifier::ITALIC));
        let both = span_with(line, "both").style.add_modifier;
        assert!(both.contains(Modifier::ITALIC | Modifier::BOLD));
        assert!(span_with(line, "gone")
            .style
            .add_modifier
            .contains(Modifier::CROSSED_OUT));
        assert_eq!(span_with(line, "code").style.fg, Some(Color::Yellow));
    }

    #[test]
    fn soft_breaks_join_lines_without_spaces_between_japanese() {
        assert_eq!(texts("one\ntwo\n"), ["one two"]);
        assert_eq!(texts("日本語の\n文章です\n"), ["日本語の文章です"]);
        assert_eq!(texts("日本語\nand English\n"), ["日本語 and English"]);
        // A hard break (two trailing spaces) starts a new line.
        assert_eq!(texts("one  \ntwo\n"), ["one", "two"]);
    }

    #[test]
    fn lists_get_markers_and_nested_indent() {
        let src = "- a\n- b\n  - c\n    - d\n- e\n\n1. one\n2. two\n\n- [x] done\n- [ ] todo\n";
        assert_eq!(
            texts(src),
            [
                "• a",
                "• b",
                "  ◦ c",
                "    ▪ d",
                "• e",
                "",
                "1. one",
                "2. two",
                "",
                "• [x] done",
                "• [ ] todo",
            ]
        );
        // Ordered lists keep their start number.
        assert_eq!(texts("3. x\n4. y\n"), ["3. x", "4. y"]);
        // Loose lists keep a blank line between items, also around a
        // nested list.
        assert_eq!(texts("- a\n\n- b\n"), ["• a", "", "• b"]);
        assert_eq!(
            texts("- a\n\n  - n\n\n- b\n"),
            ["• a", "", "  ◦ n", "", "• b"]
        );
    }

    #[test]
    fn wrapped_list_items_and_quotes_keep_their_indent() {
        let rows = wrapped("- 日本語の長い項目です\n", 10);
        assert_eq!(rows, ["• 日本語の", "  長い項目", "  です"]);
        let rows = wrapped("> quoted words here\n", 10);
        assert_eq!(rows, ["│ quoted", "│ words", "│ here"]);
        let rows = wrapped("1. alpha beta gamma\n", 11);
        assert_eq!(rows, ["1. alpha", "   beta", "   gamma"]);
    }

    #[test]
    fn quotes_nest_and_keep_the_bar_on_blank_lines() {
        assert_eq!(
            texts("> a\n>\n> b\n>\n> > c\n\nafter\n"),
            ["│ a", "│", "│ b", "│", "│ │ c", "", "after"]
        );
    }

    #[test]
    fn code_blocks_are_fenced_and_highlighted() {
        let r = render("text\n\n```rust\nfn main() {}\n```\n\n    plain\n");
        let t: Vec<String> = r.lines.iter().map(text_of).collect();
        assert_eq!(
            t,
            [
                "text",
                "",
                "```rust",
                "fn main() {}",
                "```",
                "",
                "```",
                "plain",
                "```"
            ]
        );
        assert!(r.lines[3].spans.iter().any(|s| s.style.fg.is_some()));
        assert!(r.lines[7].spans.iter().all(|s| s.style.fg.is_none()));
        // Code in a list item is indented with it.
        assert_eq!(
            texts("- item\n\n  ```\n  x\n  ```\n"),
            ["• item", "", "  ```", "  x", "  ```"]
        );
    }

    #[test]
    fn tables_align_columns_by_display_width() {
        let src = "| 名前 | n |\n|:---|---:|\n| a | 10 |\n| 日本 | 2 |\n";
        assert_eq!(
            texts(src),
            [
                "┌──────┬────┐",
                "│ 名前 │  n │",
                "├──────┼────┤",
                "│ a    │ 10 │",
                "│ 日本 │  2 │",
                "└──────┴────┘",
            ]
        );
        let r = render(src);
        assert!(span_with(&r.lines[1], "名前")
            .style
            .add_modifier
            .contains(Modifier::BOLD));
        // Centered columns and missing cells.
        assert_eq!(
            texts("| x | y |\n|:-:|---|\n| abc |\n"),
            [
                "┌─────┬───┐",
                "│  x  │ y │",
                "├─────┼───┤",
                "│ abc │   │",
                "└─────┴───┘",
            ]
        );
    }

    #[test]
    fn links_show_their_url_unless_it_is_the_text() {
        let r = render("see [docs](https://e.x/d) or <https://e.x>\n");
        let line = &r.lines[0];
        assert_eq!(text_of(line), "see docs (https://e.x/d) or https://e.x");
        assert_eq!(span_with(line, "docs").style, link_style());
        assert_eq!(span_with(line, " (https://e.x/d)").style, dim());
        assert_eq!(texts("![logo](img/a.png)\n"), ["[image: logo] (img/a.png)"]);
    }

    #[test]
    fn rules_html_and_blank_line_handling() {
        assert_eq!(
            texts("a\n\n---\n\nb\n"),
            ["a", "", &"─".repeat(RULE_WIDTH), "", "b"]
        );
        assert_eq!(texts("<div>\nhi\n</div>\n"), ["<div>", "hi", "</div>"]);
        assert!(texts("").is_empty());
        let r = render("x\n");
        assert_eq!(r.lines.len(), r.hangs.len());
    }
}
