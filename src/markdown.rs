//! Parses GitHub-flavored markdown into blocks whose text is Pango markup, ready to be laid
//! out as GTK widgets (see `ui::markdown_view`).

use gtk::glib::markup_escape_text;
use pulldown_cmark::{Alignment, BlockQuoteKind, CodeBlockKind, Event, HeadingLevel, Options, Parser, Tag, TagEnd};

const CODE_SPAN: &str = r##"<span font_family="monospace" background="#808080" bgalpha="20%">"##;
const MUTED_SPAN: &str = r##"<span foreground="#808080">"##;

#[derive(Debug)]
pub enum Block {
    /// Inline markup.
    Paragraph(String),
    /// Level 1–6 and inline markup.
    Heading(u8, String),
    /// Escaped code text; `lang` comes from the fence info string.
    Code { lang: Option<String>, markup: String },
    Quote { kind: Option<&'static str>, blocks: Vec<Block> },
    /// `start` is the first number of an ordered list, `None` for bullets.
    List { start: Option<u64>, items: Vec<ListItem> },
    /// Monospace markup with columns already aligned.
    Table(String),
    Rule,
}

#[derive(Debug, Default)]
pub struct ListItem {
    /// `Some(checked)` for task list items.
    pub task: Option<bool>,
    pub blocks: Vec<Block>,
}

pub fn parse(markdown: &str) -> Vec<Block> {
    let options = Options::ENABLE_TABLES
        | Options::ENABLE_STRIKETHROUGH
        | Options::ENABLE_TASKLISTS
        | Options::ENABLE_GFM;
    let mut builder = Builder {
        stack: vec![Container::Root(Vec::new())],
        ..Builder::default()
    };
    for event in Parser::new_ext(markdown, options) {
        builder.event(event);
    }
    builder.flush_text();
    builder.finish_inline();
    match builder.stack.into_iter().next() {
        Some(Container::Root(blocks)) => blocks,
        _ => Vec::new(),
    }
}

fn escape(text: &str) -> String {
    markup_escape_text(text).to_string()
}

enum Container {
    Root(Vec<Block>),
    Quote { kind: Option<&'static str>, blocks: Vec<Block> },
    List { start: Option<u64>, items: Vec<ListItem> },
    Item(ListItem),
}

enum InlineKind {
    Paragraph,
    Heading(u8),
}

/// A paragraph or heading whose markup is still being collected.
struct Inline {
    kind: InlineKind,
    markup: String,
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

#[derive(Default)]
struct Builder {
    stack: Vec<Container>,
    inline: Option<Inline>,
    link_depth: usize,
    /// Adjacent text events are merged so bare URLs can be linkified in one piece.
    pending_text: String,
    code_block: Option<(Option<String>, String)>,
    image: Option<Image>,
    table: Option<Table>,
}

impl Builder {
    fn event(&mut self, event: Event<'_>) {
        if let Event::Text(text) = &event {
            if let Some((_, code)) = &mut self.code_block {
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
                self.count(&html);
                self.push(&escape(&html));
            }
            Event::FootnoteReference(name) => self.push(&format!("<sup>[{}]</sup>", escape(&name))),
            // Like GitHub comments, a single newline in the source is a line break.
            Event::SoftBreak | Event::HardBreak => self.push("\n"),
            Event::Rule => self.add_block(Block::Rule),
            Event::TaskListMarker(checked) => {
                if let Some(Container::Item(item)) = self.stack.last_mut() {
                    item.task = Some(checked);
                }
            }
            Event::Text(_) => unreachable!(),
        }
    }

    fn start(&mut self, tag: Tag<'_>) {
        match tag {
            Tag::Paragraph | Tag::HtmlBlock => self.open_inline(InlineKind::Paragraph),
            Tag::Heading { level, .. } => {
                let level = match level {
                    HeadingLevel::H1 => 1,
                    HeadingLevel::H2 => 2,
                    HeadingLevel::H3 => 3,
                    HeadingLevel::H4 => 4,
                    HeadingLevel::H5 => 5,
                    HeadingLevel::H6 => 6,
                };
                self.open_inline(InlineKind::Heading(level));
            }
            Tag::BlockQuote(kind) => {
                self.finish_inline();
                let kind = kind.map(|kind| match kind {
                    BlockQuoteKind::Note => "Note",
                    BlockQuoteKind::Tip => "Tip",
                    BlockQuoteKind::Important => "Important",
                    BlockQuoteKind::Warning => "Warning",
                    BlockQuoteKind::Caution => "Caution",
                });
                self.stack.push(Container::Quote { kind, blocks: Vec::new() });
            }
            Tag::CodeBlock(kind) => {
                self.finish_inline();
                let lang = match kind {
                    CodeBlockKind::Fenced(lang) if !lang.is_empty() => Some(lang.into_string()),
                    _ => None,
                };
                self.code_block = Some((lang, String::new()));
            }
            Tag::List(start) => {
                self.finish_inline();
                self.stack.push(Container::List { start, items: Vec::new() });
            }
            Tag::Item => {
                self.finish_inline();
                self.stack.push(Container::Item(ListItem::default()));
            }
            Tag::Table(alignments) => {
                self.finish_inline();
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
            TagEnd::Paragraph | TagEnd::HtmlBlock | TagEnd::Heading(_) => self.finish_inline(),
            TagEnd::BlockQuote(_) => {
                self.finish_inline();
                if let Some(Container::Quote { kind, blocks }) = self.stack.pop() {
                    self.add_block(Block::Quote { kind, blocks });
                }
            }
            TagEnd::CodeBlock => {
                if let Some((lang, code)) = self.code_block.take() {
                    let markup = escape(code.trim_end_matches('\n'));
                    self.add_block(Block::Code { lang, markup });
                }
            }
            TagEnd::List(_) => {
                self.finish_inline();
                if let Some(Container::List { start, items }) = self.stack.pop() {
                    self.add_block(Block::List { start, items });
                }
            }
            TagEnd::Item => {
                self.finish_inline();
                if let Some(Container::Item(item)) = self.stack.pop()
                    && let Some(Container::List { items, .. }) = self.stack.last_mut()
                {
                    items.push(item);
                }
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
                    self.add_block(Block::Table(render_table(&table)));
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

    fn open_inline(&mut self, kind: InlineKind) {
        self.finish_inline();
        self.inline = Some(Inline { kind, markup: String::new() });
    }

    /// Turns the paragraph or heading being collected into a block.
    fn finish_inline(&mut self) {
        let Some(Inline { kind, markup }) = self.inline.take() else {
            return;
        };
        let markup = markup.trim_end_matches('\n').to_owned();
        if markup.trim().is_empty() {
            return;
        }
        self.add_block(match kind {
            InlineKind::Paragraph => Block::Paragraph(markup),
            InlineKind::Heading(level) => Block::Heading(level, markup),
        });
    }

    fn add_block(&mut self, block: Block) {
        self.finish_inline();
        match self.stack.last_mut() {
            Some(Container::Root(blocks) | Container::Quote { blocks, .. }) => blocks.push(block),
            Some(Container::Item(item)) => item.blocks.push(block),
            // Lists only ever contain items.
            Some(Container::List { .. }) | None => {}
        }
    }

    /// Appends inline markup to the table cell or paragraph being built. Tight list items
    /// have text without a paragraph around it, so one is opened on demand.
    fn push(&mut self, markup: &str) {
        if let Some(cell) = self.table.as_mut().and_then(|t| t.cell.as_mut()) {
            cell.markup.push_str(markup);
            return;
        }
        self.inline
            .get_or_insert_with(|| Inline { kind: InlineKind::Paragraph, markup: String::new() })
            .markup
            .push_str(markup);
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
}

fn render_table(table: &Table) -> String {
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
    format!(r#"<span font_family="monospace">{}</span>"#, lines.join("\n"))
}

#[cfg(test)]
mod tests {
    use super::{Block, parse};

    /// `<a href>` is a GtkLabel extension, so links are stripped before asking Pango.
    fn assert_valid_markup(markup: &str) {
        let mut plain = String::new();
        let mut rest = markup;
        while let Some(start) = rest.find("<a href=") {
            plain.push_str(&rest[..start]);
            rest = &rest[start + rest[start..].find('>').unwrap() + 1..];
        }
        plain.push_str(rest);
        let plain = plain.replace("</a>", "");
        if let Err(e) = gtk::pango::parse_markup(&plain, '\0') {
            panic!("invalid markup ({e}): {markup}");
        }
    }

    fn assert_all_valid(blocks: &[Block]) {
        for block in blocks {
            match block {
                Block::Paragraph(m) | Block::Heading(_, m) | Block::Table(m) => assert_valid_markup(m),
                Block::Code { markup, .. } => assert_valid_markup(markup),
                Block::Quote { blocks, .. } => assert_all_valid(blocks),
                Block::List { items, .. } => items.iter().for_each(|i| assert_all_valid(&i.blocks)),
                Block::Rule => {}
            }
        }
    }

    #[test]
    fn renders_inline_styles() {
        let blocks = parse("Some **bold**, *italic*, ~~gone~~ and `code <x>`.");
        let [Block::Paragraph(p)] = blocks.as_slice() else { panic!("{blocks:?}") };
        assert!(p.contains("<b>bold</b>") && p.contains("<i>italic</i>") && p.contains("<s>gone</s>"));
        assert!(p.contains("code &lt;x&gt;"));
        assert_all_valid(&blocks);
    }

    #[test]
    fn single_newlines_are_line_breaks() {
        let blocks = parse("first line\nsecond line\n\nnext paragraph");
        let [Block::Paragraph(a), Block::Paragraph(b)] = blocks.as_slice() else { panic!("{blocks:?}") };
        assert_eq!(a, "first line\nsecond line");
        assert_eq!(b, "next paragraph");
    }

    #[test]
    fn builds_nested_and_task_lists() {
        let blocks = parse("- [x] done\n- [ ] todo\n  1. nested\n  2. list\n\n3. three\n4. four");
        let [Block::List { start: None, items }, Block::List { start: Some(3), items: numbered }] = blocks.as_slice()
        else {
            panic!("{blocks:?}")
        };
        assert_eq!(items[0].task, Some(true));
        assert_eq!(items[1].task, Some(false));
        let [Block::Paragraph(todo), Block::List { start: Some(1), items: nested }] = items[1].blocks.as_slice() else {
            panic!("{:?}", items[1].blocks)
        };
        assert_eq!(todo, "todo");
        assert_eq!(nested.len(), 2);
        assert_eq!(numbered.len(), 2);
    }

    #[test]
    fn renders_gfm_blocks_as_valid_markup() {
        let md = "# Title\n\nIntro with https://example.com/a_b?x=1&y=2.\n\n\
                  > quoted <tag>\n\n```rust\nfn main() {}\n```\n\n\
                  | a | b |\n|:-|-:|\n| 1 | **22** |\n\n---\n\n![alt](img.png) [link](https://x.y)";
        let blocks = parse(md);
        assert!(matches!(blocks[0], Block::Heading(1, _)));
        let Block::Paragraph(intro) = &blocks[1] else { panic!("{blocks:?}") };
        assert!(intro.contains(r#"<a href="https://example.com/a_b?x=1&amp;y=2">"#), "{intro}");
        assert!(matches!(&blocks[3], Block::Code { lang: Some(l), .. } if l == "rust"));
        assert!(matches!(blocks[5], Block::Rule));
        assert_all_valid(&blocks);
    }
}
