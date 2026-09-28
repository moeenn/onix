//! Lays out parsed markdown as GTK widgets for the ticket preview.

use gtk::prelude::*;

use crate::markdown::{self, Block, ListItem};

/// Vertical space (px) above a block, by what precedes it.
const GAP_AFTER_HEADING: i32 = 4;
const GAP_BEFORE_HEADING: i32 = 18;
const GAP_BETWEEN_BLOCKS: i32 = 10;
const GAP_BETWEEN_ITEMS: i32 = 4;
/// Space between a list marker and the item text.
const GUTTER_SPACING: i32 = 8;

pub fn build(markdown: &str) -> gtk::Box {
    let root = vbox();
    append_blocks(&root, &markdown::parse(markdown));
    root
}

fn vbox() -> gtk::Box {
    gtk::Box::new(gtk::Orientation::Vertical, 0)
}

fn append_blocks(container: &gtk::Box, blocks: &[Block]) {
    let mut previous: Option<&Block> = None;
    for block in blocks {
        let widget = block_widget(block);
        widget.set_margin_top(match (previous, block) {
            (None, _) => 0,
            (Some(Block::Heading(..)), _) => GAP_AFTER_HEADING,
            (Some(_), Block::Heading(..)) => GAP_BEFORE_HEADING,
            _ => GAP_BETWEEN_BLOCKS,
        });
        container.append(&widget);
        previous = Some(block);
    }
}

fn text_label(markup: &str) -> gtk::Label {
    let label = gtk::Label::builder()
        .wrap(true)
        .wrap_mode(gtk::pango::WrapMode::WordChar)
        .xalign(0.0)
        .selectable(true)
        .build();
    label.set_markup(markup);
    label
}

fn block_widget(block: &Block) -> gtk::Widget {
    match block {
        Block::Paragraph(markup) => text_label(markup).upcast(),
        Block::Heading(level, markup) => {
            let label = text_label(markup);
            label.add_css_class(match level {
                1 => "title-2",
                2 => "title-3",
                3 => "title-4",
                _ => "heading",
            });
            label.upcast()
        }
        Block::Code { lang, markup } => {
            let code = vbox();
            code.add_css_class("md-code");
            if let Some(lang) = lang {
                let lang = gtk::Label::builder()
                    .label(lang)
                    .xalign(0.0)
                    .css_classes(["dim-label", "caption"])
                    .margin_bottom(4)
                    .build();
                code.append(&lang);
            }
            let text = text_label(markup);
            text.add_css_class("monospace");
            code.append(&text);
            code.upcast()
        }
        Block::Quote { kind, blocks } => {
            let quote = vbox();
            quote.add_css_class("md-quote");
            if let Some(kind) = kind {
                let title = gtk::Label::builder()
                    .label(*kind)
                    .xalign(0.0)
                    .css_classes(["heading"])
                    .margin_bottom(4)
                    .build();
                quote.append(&title);
            }
            append_blocks(&quote, blocks);
            quote.upcast()
        }
        Block::List { start, items } => list(*start, items).upcast(),
        Block::Table(markup) => {
            let table = text_label(markup);
            table.set_wrap(false);
            gtk::ScrolledWindow::builder()
                .hscrollbar_policy(gtk::PolicyType::Automatic)
                .vscrollbar_policy(gtk::PolicyType::Never)
                .propagate_natural_height(true)
                .child(&table)
                .build()
                .upcast()
        }
        Block::Rule => gtk::Separator::new(gtk::Orientation::Horizontal).upcast(),
    }
}

/// Each item is a fixed-width marker column next to the item's blocks, so wrapped and
/// continuation lines align with the first line of text.
fn list(start: Option<u64>, items: &[ListItem]) -> gtk::Box {
    let list = vbox();
    // Wide enough for the longest number plus its dot, right-aligned so the dots line up.
    let gutter_chars = match start {
        Some(first) => (first + items.len().saturating_sub(1) as u64).to_string().len() as i32 + 1,
        None => 1,
    }
    .max(2);

    for (index, item) in items.iter().enumerate() {
        let marker = match (item.task, start) {
            (Some(true), _) => "☑".to_owned(),
            (Some(false), _) => "☐".to_owned(),
            (None, Some(first)) => format!("{}.", first + index as u64),
            (None, None) => "•".to_owned(),
        };
        let gutter = gtk::Label::builder()
            .label(marker)
            .width_chars(gutter_chars)
            .xalign(1.0)
            .valign(gtk::Align::Start)
            .css_classes(["numeric", "md-list-marker"])
            .build();

        let content = vbox();
        content.set_hexpand(true);
        append_blocks(&content, &item.blocks);

        let row = gtk::Box::new(gtk::Orientation::Horizontal, GUTTER_SPACING);
        row.append(&gutter);
        row.append(&content);
        if index > 0 {
            row.set_margin_top(GAP_BETWEEN_ITEMS);
        }
        list.append(&row);
    }
    list
}
