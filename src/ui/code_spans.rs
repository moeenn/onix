//! Wraps a markdown label and draws rounded backgrounds behind its inline code spans.
//!
//! Pango can only paint square, unpadded text backgrounds, and `GtkLabel` can't be
//! subclassed, so this parent widget paints the backgrounds before drawing the label.
//! Code spans are found through the `insert_hyphens="false"` attribute that
//! `markdown::CODE_SPAN` puts on them.

use gtk::glib;
use gtk::prelude::*;
use gtk::subclass::prelude::*;
use gtk::{graphene, gsk, pango};

/// Horizontal padding (px) around the code text; the markup leaves room for it.
const PAD_X: f32 = 3.0;
const PAD_Y: f32 = 1.0;
const RADIUS: f32 = 4.0;
const BACKGROUND_ALPHA: f32 = 0.12;

mod imp {
    use std::cell::OnceCell;

    use super::*;

    #[derive(Default)]
    pub struct CodeSpans {
        pub label: OnceCell<gtk::Label>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for CodeSpans {
        const NAME: &'static str = "OrgxCodeSpans";
        type Type = super::CodeSpans;
        type ParentType = gtk::Widget;

        fn class_init(klass: &mut Self::Class) {
            klass.set_layout_manager_type::<gtk::BinLayout>();
        }
    }

    impl ObjectImpl for CodeSpans {
        fn dispose(&self) {
            if let Some(label) = self.label.get() {
                label.unparent();
            }
        }
    }

    impl WidgetImpl for CodeSpans {
        fn snapshot(&self, snapshot: &gtk::Snapshot) {
            let Some(label) = self.label.get() else {
                return;
            };
            let widget = self.obj();
            draw_backgrounds(widget.upcast_ref(), label, snapshot);
            widget.snapshot_child(label, snapshot);
        }
    }
}

glib::wrapper! {
    pub struct CodeSpans(ObjectSubclass<imp::CodeSpans>)
        @extends gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

impl CodeSpans {
    pub fn new(label: &gtk::Label) -> Self {
        let this: Self = glib::Object::new();
        label.set_parent(&this);
        this.imp()
            .label
            .set(label.clone())
            .expect("label is set once");
        this
    }
}

fn draw_backgrounds(widget: &gtk::Widget, label: &gtk::Label, snapshot: &gtk::Snapshot) {
    let layout = label.layout();
    let Some(attributes) = layout.attributes() else {
        return;
    };
    let spans: Vec<(i32, i32)> = attributes
        .attributes()
        .iter()
        .filter(|a| a.type_() == pango::AttrType::InsertHyphens)
        .map(|a| (a.start_index() as i32, a.end_index() as i32))
        .collect();
    if spans.is_empty() {
        return;
    }

    let Some(origin) = label.compute_point(widget, &graphene::Point::zero()) else {
        return;
    };
    let (offset_x, offset_y) = label.layout_offsets();
    let origin_x = origin.x() + offset_x as f32;
    let origin_y = origin.y() + offset_y as f32;
    let metrics = label.pango_context().metrics(None, None);
    let ascent = metrics.ascent() as f32 / pango::SCALE as f32;
    let height = ascent + metrics.descent() as f32 / pango::SCALE as f32;
    let mut color = label.color();
    color.set_alpha(BACKGROUND_ALPHA);

    // A span that wraps gets one background per line it covers.
    let mut lines = layout.iter();
    loop {
        if let Some(line) = lines.line_readonly() {
            let line_start = line.start_index();
            let line_end = line_start + line.length();
            let baseline = lines.baseline() as f32 / pango::SCALE as f32;
            for &(start, end) in &spans {
                if end <= line_start || start >= line_end {
                    continue;
                }
                let ranges = line.x_ranges(start.max(line_start), end.min(line_end));
                for &[range_start, range_end] in ranges.as_chunks::<2>().0 {
                    let x = range_start as f32 / pango::SCALE as f32;
                    let width = (range_end - range_start) as f32 / pango::SCALE as f32;
                    let rect = graphene::Rect::new(
                        origin_x + x - PAD_X,
                        origin_y + baseline - ascent - PAD_Y,
                        width + 2.0 * PAD_X,
                        height + 2.0 * PAD_Y,
                    );
                    snapshot.push_rounded_clip(&gsk::RoundedRect::from_rect(rect, RADIUS));
                    snapshot.append_color(&color, &rect);
                    snapshot.pop();
                }
            }
        }
        if !lines.next_line() {
            break;
        }
    }
}
