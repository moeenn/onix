//! Lays out parsed markdown as GTK widgets for the ticket preview.

use std::ops::Range;
use std::rc::Rc;

use gtk::prelude::*;

use super::code_spans::CodeSpans;
use crate::markdown::{self, Block, ListItem};

/// Called with a task's marker range and new state when its checkbox is clicked.
pub type OnTaskToggled = Rc<dyn Fn(&Range<usize>, bool)>;

/// Vertical space (px) above a block, by what precedes it.
const GAP_AFTER_HEADING: i32 = 4;
const GAP_BEFORE_HEADING: i32 = 18;
const GAP_BETWEEN_BLOCKS: i32 = 10;
const GAP_BETWEEN_ITEMS: i32 = 4;
/// Space between a list marker and the item text.
const GUTTER_SPACING: i32 = 8;

pub fn build(markdown: &str, on_task_toggled: OnTaskToggled) -> gtk::Box {
    let root = vbox();
    append_blocks(&root, &markdown::parse(markdown), &on_task_toggled);
    root
}

fn vbox() -> gtk::Box {
    gtk::Box::new(gtk::Orientation::Vertical, 0)
}

fn append_blocks(container: &gtk::Box, blocks: &[Block], on_task_toggled: &OnTaskToggled) {
    let mut previous: Option<&Block> = None;
    for block in blocks {
        let widget = block_widget(block, on_task_toggled);
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
        // Text starts at the label's top edge, which `center_on_first_line` relies on.
        .yalign(0.0)
        .selectable(true)
        .build();
    label.set_markup(markup);
    label
}

fn block_widget(block: &Block, on_task_toggled: &OnTaskToggled) -> gtk::Widget {
    match block {
        Block::Paragraph(markup) => CodeSpans::new(&text_label(markup)).upcast(),
        Block::Heading(level, markup) => {
            let label = text_label(markup);
            label.add_css_class(match level {
                1 => "title-2",
                2 => "title-3",
                3 => "title-4",
                _ => "heading",
            });
            CodeSpans::new(&label).upcast()
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
            append_blocks(&quote, blocks, on_task_toggled);
            quote.upcast()
        }
        Block::List { start, items } => list(*start, items, on_task_toggled).upcast(),
        Block::Table(markup) => {
            let table = text_label(markup);
            table.set_wrap(false);
            gtk::ScrolledWindow::builder()
                .hscrollbar_policy(gtk::PolicyType::Automatic)
                .vscrollbar_policy(gtk::PolicyType::Never)
                .propagate_natural_height(true)
                .child(&CodeSpans::new(&table))
                .build()
                .upcast()
        }
        Block::Rule => gtk::Separator::new(gtk::Orientation::Horizontal).upcast(),
    }
}

/// Each item is a fixed-width marker column next to the item's blocks, so wrapped and
/// continuation lines align with the first line of text.
fn list(start: Option<u64>, items: &[ListItem], on_task_toggled: &OnTaskToggled) -> gtk::Box {
    let list = vbox();
    // Checkboxes and text markers can share a list, so size the gutters as one column.
    let gutters = gtk::SizeGroup::new(gtk::SizeGroupMode::Horizontal);
    // Wide enough for the longest number plus its dot, right-aligned so the dots line up.
    let gutter_chars = match start {
        Some(first) => {
            (first + items.len().saturating_sub(1) as u64)
                .to_string()
                .len() as i32
                + 1
        }
        None => 1,
    }
    .max(2);

    for (index, item) in items.iter().enumerate() {
        let gutter: gtk::Widget = match (&item.task, start) {
            (Some(task), _) => {
                let check = gtk::CheckButton::builder()
                    .active(task.checked)
                    .halign(gtk::Align::End)
                    .valign(gtk::Align::Start)
                    .css_classes(["md-task"])
                    .tooltip_text(if task.checked {
                        "Mark as not done"
                    } else {
                        "Mark as done"
                    })
                    .build();
                check.set_cursor_from_name(Some("pointer"));
                let marker = task.marker.clone();
                let on_task_toggled = Rc::clone(on_task_toggled);
                check.connect_toggled(move |check| on_task_toggled(&marker, check.is_active()));
                check.upcast()
            }
            (None, Some(first)) => {
                marker_label(&format!("{}.", first + index as u64), gutter_chars)
            }
            (None, None) => marker_label("•", gutter_chars),
        };
        gutters.add_widget(&gutter);

        let content = vbox();
        content.set_hexpand(true);
        append_blocks(&content, &item.blocks, on_task_toggled);
        if let (Some(check), Some(text)) = (
            gutter.downcast_ref::<gtk::CheckButton>(),
            first_label(content.upcast_ref()),
        ) {
            center_on_first_line(check, &text);
        }

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

/// The label holding an item's first line, if its first block is text.
fn first_label(widget: &gtk::Widget) -> Option<gtk::Label> {
    let mut current = widget.first_child();
    while let Some(child) = current {
        if let Ok(label) = child.clone().downcast::<gtk::Label>() {
            return Some(label);
        }
        current = child.first_child();
    }
    None
}

/// Moves `check` down so it is centered on the first line of `text`. The line is taller than
/// the checkbox (its font plus line-height spacing), and only known once the label is styled.
/// The preview can be built while hidden behind the editor, so this runs each time the
/// label is shown rather than when it is realized.
fn center_on_first_line(check: &gtk::CheckButton, text: &gtk::Label) {
    let check = check.downgrade();
    text.connect_map(move |text| {
        let Some(check) = check.upgrade() else {
            return;
        };
        let (_, line) = text.layout().iter().line_extents();
        let line_center = (line.y() as f32 + line.height() as f32 / 2.0) / gtk::pango::SCALE as f32;
        let (_, check_height, _, _) = check.measure(gtk::Orientation::Vertical, -1);
        // Not `layout_offsets()`: a label shown for the first time isn't sized yet, and its
        // offset is meaningless until it is.
        let offset = line_center - check_height as f32 / 2.0;
        check.set_margin_top(offset.round().max(0.0) as i32);
    });
}

fn marker_label(marker: &str, width_chars: i32) -> gtk::Widget {
    gtk::Label::builder()
        .label(marker)
        .width_chars(width_chars)
        .xalign(1.0)
        .valign(gtk::Align::Start)
        .css_classes(["numeric", "md-list-marker"])
        .build()
        .upcast()
}
