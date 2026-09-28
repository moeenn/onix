//! Renders GitHub-flavored markdown as Pango markup for display in a `GtkLabel`.

use gtk::glib::markup_escape_text;
use pulldown_cmark::{Alignment, BlockQuoteKind, CodeBlockKind, Event, HeadingLevel, Options, Parser, Tag, TagEnd};

const CODE_SPAN: &str = r##"<span font_family="monospace" background="#808080" bgalpha="20%">"##;
const MUTED_SPAN: &str = r##"<span foreground="#808080">"##;
const INDENT: &str = "    ";

pub fn to_pango(markdown: &str) -> String {
    let options = Options::ENABLE_TABLES
        | Options::ENABLE_STRIKETHROUGH
        | Options::ENABLE_TASKLISTS
        | Options::ENABLE_GFM;
    let mut renderer = Renderer::default();
    for event in Parser::new_ext(markdown, options) {
        renderer.event(event);
    }
    renderer.flush_text();
    renderer.out
}

fn escape(text: &str) -> String {
    markup_escape_text(text).to_string()
}

#[derive(Default)]
struct Renderer {
    out: String,
    /// One entry per open list: the next number for ordered lists, `None` for bullets.
    lists: Vec<Option<u64>>,
    quote_depth: usize,
    /// A bullet was just written, so the item's first block continues on the same line.
    item_fresh: bool,
    bullet_at: Option<usize>,
    link_depth: usize,
    /// Adjacent text events are merged so bare URLs can be linkified in one piece.
    pending_text: String,
    code_block: Option<String>,
    image: Option<Image>,
    table: Option<Table>,
}

struct Image {
    url: String,
    alt: String,
}

#[derive(Default)]
struct Table {
    alignments: Vec<Alignment>,
    rows: Vec<Vec<Cell>>,
    header_rows: usize,
    cell: Option<Cell>,
}

#[derive(Default)]
struct Cell {
    markup: String,
    /// Visible width in characters, used to align columns.
    width: usize,
}

impl Renderer {
    fn event(&mut self, event: Event<'_>) {
        if let Event::Text(text) = &event {
            if let Some(code) = &mut self.code_block {
                code.push_str(text);
            } else if let Some(image) = &mut self.image {
                image.alt.push_str(text);
            } else {
                self.pending_text.push_str(text);
            }
            return;
        }
        self.flush_text();

        match event {
            Event::Start(tag) => self.start(tag),
            Event::End(tag) => self.end(tag),
            Event::Code(code) | Event::InlineMath(code) | Event::DisplayMath(code) => {
                self.count(&code);
                self.push(&format!("{CODE_SPAN}{}</span>", escape(&code)));
            }
            Event::Html(html) | Event::InlineHtml(html) => {
                self.count(html.trim_end());
                self.push(&escape(html.trim_end()));
            }
            Event::FootnoteReference(name) => self.push(&format!("<sup>[{}]</sup>", escape(&name))),
            Event::SoftBreak => self.push(" "),
            Event::HardBreak => {
                let prefix = self.prefix(true);
                self.push(&format!("\n{prefix}"));
            }
            Event::Rule => {
                self.start_block();
                self.push(&format!("{MUTED_SPAN}{}</span>", "─".repeat(32)));
            }
            Event::TaskListMarker(checked) => {
                // Checkboxes replace the bullet of unordered items.
                if let (Some(at), Some(None)) = (self.bullet_at.take(), self.lists.last()) {
                    self.out.truncate(at);
                }
                self.push(if checked { "☑ " } else { "☐ " });
            }
            Event::Text(_) => unreachable!(),
        }
    }

    fn start(&mut self, tag: Tag<'_>) {
        match tag {
            Tag::Paragraph | Tag::HtmlBlock => self.start_block(),
            Tag::Heading { level, .. } => {
                self.start_block();
                let size = match level {
                    HeadingLevel::H1 => "xx-large",
                    HeadingLevel::H2 => "x-large",
                    HeadingLevel::H3 => "large",
                    _ => "medium",
                };
                self.push(&format!(r#"<span weight="bold" size="{size}">"#));
            }
            Tag::BlockQuote(kind) => {
                self.quote_depth += 1;
                if let Some(kind) = kind {
                    let label = match kind {
                        BlockQuoteKind::Note => "Note",
                        BlockQuoteKind::Tip => "Tip",
                        BlockQuoteKind::Important => "Important",
                        BlockQuoteKind::Warning => "Warning",
                        BlockQuoteKind::Caution => "Caution",
                    };
                    self.start_block();
                    self.push(&format!("<b>{label}</b>"));
                }
            }
            Tag::CodeBlock(kind) => {
                self.start_block();
                if let CodeBlockKind::Fenced(lang) = kind
                    && !lang.is_empty()
                {
                    self.push(&format!("{MUTED_SPAN}<small>{}</small></span>\n{}", escape(&lang), self.prefix(true)));
                }
                self.code_block = Some(String::new());
            }
            Tag::List(start) => {
                if self.lists.is_empty() && !self.out.is_empty() {
                    self.out.push('\n');
                }
                self.lists.push(start);
            }
            Tag::Item => {
                if !self.out.is_empty() {
                    self.out.push('\n');
                }
                let mut line = self.prefix(false);
                line.push_str(&INDENT.repeat(self.lists.len().saturating_sub(1)));
                let bullet = match self.lists.last_mut() {
                    Some(Some(n)) => {
                        let bullet = format!("{n}. ");
                        *n += 1;
                        bullet
                    }
                    _ => "• ".to_owned(),
                };
                self.out.push_str(&line);
                self.bullet_at = Some(self.out.len());
                self.out.push_str(&bullet);
                self.item_fresh = true;
            }
            Tag::Table(alignments) => {
                self.start_block();
                self.table = Some(Table { alignments, ..Table::default() });
            }
            Tag::TableHead | Tag::TableRow => {
                if let Some(table) = &mut self.table {
                    table.rows.push(Vec::new());
                }
            }
            Tag::TableCell => {
                if let Some(table) = &mut self.table {
                    table.cell = Some(Cell::default());
                }
            }
            Tag::Emphasis => self.push("<i>"),
            Tag::Strong => self.push("<b>"),
            Tag::Strikethrough => self.push("<s>"),
            Tag::Superscript => self.push("<sup>"),
            Tag::Subscript => self.push("<sub>"),
            Tag::Link { dest_url, .. } => {
                if self.link_depth == 0 {
                    self.push(&format!(r#"<a href="{}">"#, escape(&dest_url)));
                }
                self.link_depth += 1;
            }
            Tag::Image { dest_url, .. } => {
                self.image = Some(Image { url: dest_url.into_string(), alt: String::new() });
            }
            _ => {}
        }
    }

    fn end(&mut self, tag: TagEnd) {
        match tag {
            TagEnd::Heading(_) => self.push("</span>"),
            TagEnd::BlockQuote(_) => self.quote_depth = self.quote_depth.saturating_sub(1),
            TagEnd::CodeBlock => {
                let code = self.code_block.take().unwrap_or_default();
                let separator = format!("\n{}", self.prefix(true));
                let body = escape(code.trim_end_matches('\n')).replace('\n', &separator);
                self.push(&format!("{CODE_SPAN}{body}</span>"));
            }
            TagEnd::List(_) => {
                self.lists.pop();
            }
            TagEnd::Item => {
                self.item_fresh = false;
                self.bullet_at = None;
            }
            TagEnd::TableHead => {
                if let Some(table) = &mut self.table {
                    table.header_rows = table.rows.len();
                }
            }
            TagEnd::TableCell => {
                if let Some(table) = &mut self.table
                    && let (Some(cell), Some(row)) = (table.cell.take(), table.rows.last_mut())
                {
                    row.push(cell);
                }
            }
            TagEnd::Table => {
                if let Some(table) = self.table.take() {
                    self.render_table(table);
                }
            }
            TagEnd::Emphasis => self.push("</i>"),
            TagEnd::Strong => self.push("</b>"),
            TagEnd::Strikethrough => self.push("</s>"),
            TagEnd::Superscript => self.push("</sup>"),
            TagEnd::Subscript => self.push("</sub>"),
            TagEnd::Link => {
                self.link_depth = self.link_depth.saturating_sub(1);
                if self.link_depth == 0 {
                    self.push("</a>");
                }
            }
            TagEnd::Image => {
                if let Some(Image { url, alt }) = self.image.take() {
                    let label = if alt.is_empty() { url.clone() } else { alt };
                    self.count(&label);
                    if self.link_depth == 0 {
                        self.push(&format!(r#"<a href="{}">🖼 {}</a>"#, escape(&url), escape(&label)));
                    } else {
                        self.push(&format!("🖼 {}", escape(&label)));
                    }
                }
            }
            _ => {}
        }
    }

    /// Separates a new block from what came before and writes the line prefix.
    fn start_block(&mut self) {
        if self.item_fresh {
            self.item_fresh = false;
            return;
        }
        if !self.out.is_empty() {
            self.out.push('\n');
            if self.lists.is_empty() {
                self.out.push('\n');
            }
        }
        let prefix = self.prefix(true);
        self.out.push_str(&prefix);
    }

    /// Quote bars, plus list indentation for lines that continue an item.
    fn prefix(&self, continuation: bool) -> String {
        let mut prefix = format!("{MUTED_SPAN}▎</span> ").repeat(self.quote_depth);
        if continuation {
            prefix.push_str(&INDENT.repeat(self.lists.len()));
        }
        prefix
    }

    fn push(&mut self, markup: &str) {
        self.item_fresh = false;
        match self.table.as_mut().and_then(|t| t.cell.as_mut()) {
            Some(cell) => cell.markup.push_str(markup),
            None => self.out.push_str(markup),
        }
    }

    fn count(&mut self, text: &str) {
        if let Some(cell) = self.table.as_mut().and_then(|t| t.cell.as_mut()) {
            cell.width += text.chars().count();
        }
    }

    fn flush_text(&mut self) {
        if self.pending_text.is_empty() {
            return;
        }
        let text = std::mem::take(&mut self.pending_text);
        self.count(&text);
        if self.link_depth > 0 {
            self.push(&escape(&text));
        } else {
            self.push_linkified(&text);
        }
    }

    /// GFM autolinks bare `http(s)://` URLs.
    fn push_linkified(&mut self, text: &str) {
        let mut rest = text;
        while let Some(start) = ["https://", "http://"].iter().filter_map(|scheme| rest.find(scheme)).min() {
            let (before, tail) = rest.split_at(start);
            let end = tail.find(char::is_whitespace).unwrap_or(tail.len());
            let url = tail[..end].trim_end_matches(['.', ',', ';', ':', '!', '?', ')', '\'', '"']);
            self.push(&escape(before));
            let url_markup = escape(url);
            self.push(&format!(r#"<a href="{url_markup}">{url_markup}</a>"#));
            rest = &tail[url.len()..];
        }
        self.push(&escape(rest));
    }

    fn render_table(&mut self, table: Table) {
        let column_count = table.rows.iter().map(Vec::len).max().unwrap_or(0);
        let mut widths = vec![0; column_count];
        for row in &table.rows {
            for (i, cell) in row.iter().enumerate() {
                widths[i] = widths[i].max(cell.width);
            }
        }

        let divider = format!("{MUTED_SPAN} │ </span>");
        let mut lines = Vec::new();
        for (row_index, row) in table.rows.iter().enumerate() {
            let is_header = row_index < table.header_rows;
            let mut line = String::new();
            for (i, width) in widths.iter().enumerate() {
                if i > 0 {
                    line.push_str(&divider);
                }
                let (markup, cell_width) = row.get(i).map_or(("", 0), |c| (c.markup.as_str(), c.width));
                let pad = width - cell_width;
                let (left, right) = match table.alignments.get(i) {
                    Some(Alignment::Right) => (pad, 0),
                    Some(Alignment::Center) => (pad / 2, pad - pad / 2),
                    _ => (0, pad),
                };
                line.push_str(&" ".repeat(left));
                if is_header {
                    line.push_str(&format!("<b>{markup}</b>"));
                } else {
                    line.push_str(markup);
                }
                line.push_str(&" ".repeat(right));
            }
            lines.push(line);
            if row_index + 1 == table.header_rows {
                let rule: Vec<String> = widths.iter().map(|w| "─".repeat(*w)).collect();
                lines.push(format!("{MUTED_SPAN}{}</span>", rule.join("─┼─")));
            }
        }

        let separator = format!("\n{}", self.prefix(true));
        self.push(&format!(r#"<span font_family="monospace">{}</span>"#, lines.join(&separator)));
    }
}

#[cfg(test)]
mod tests {
    use super::to_pango;

    /// `<a href>` is a GtkLabel extension, so links are stripped before asking Pango.
    fn is_valid_markup(markup: &str) -> bool {
        let mut plain = String::new();
        let mut rest = markup;
        while let Some(start) = rest.find("<a href=") {
            plain.push_str(&rest[..start]);
            rest = &rest[start + rest[start..].find('>').unwrap() + 1..];
        }
        plain.push_str(rest);
        let plain = plain.replace("</a>", "");
        gtk::pango::parse_markup(&plain, '\0').map_err(|e| eprintln!("{e}")).is_ok()
    }

    #[test]
    fn renders_inline_styles() {
        let out = to_pango("Some **bold**, *italic*, ~~gone~~ and `code <x>`.");
        assert!(out.contains("<b>bold</b>"));
        assert!(out.contains("<i>italic</i>"));
        assert!(out.contains("<s>gone</s>"));
        assert!(out.contains("code &lt;x&gt;"));
        assert!(is_valid_markup(&out), "{out}");
    }

    #[test]
    fn renders_gfm_blocks_as_valid_markup() {
        let md = "# Title\n\nIntro with https://example.com/a_b?x=1&y=2.\n\n\
                  - [x] done\n- [ ] todo\n  1. nested\n  2. list\n\n\
                  > quoted <tag>\n\n```rust\nfn main() {}\n```\n\n\
                  | a | b |\n|:-|-:|\n| 1 | **22** |\n\n---\n\n![alt](img.png) [link](https://x.y)";
        let out = to_pango(md);
        assert!(out.contains("☑ done") && out.contains("☐ todo"));
        assert!(out.contains(r#"<a href="https://example.com/a_b?x=1&amp;y=2">"#), "{out}");
        assert!(out.contains("1. nested"));
        assert!(is_valid_markup(&out), "{out}");
    }
}
