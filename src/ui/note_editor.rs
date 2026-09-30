//! Page for writing a new note or editing one. Unsaved changes are saved every few seconds,
//! and when leaving the page.

use std::cell::{Cell, RefCell};
use std::rc::{Rc, Weak};

use adw::prelude::*;
use gtk::glib::{self, clone};

use super::dialog::{
    TAB_WIDTH_CHARS, indent_with_spaces, set_tab_width, trim_pasted_text_in_entry,
    trim_pasted_text_in_view,
};
use super::notes::NotesPage;
use crate::model::Note;

/// How often unsaved changes are saved.
const AUTOSAVE_SECONDS: u32 = 10;

pub struct NoteEditor {
    notes: Weak<NotesPage>,
    page: adw::NavigationPage,
    window_title: adw::WindowTitle,
    back_button: gtk::Button,
    delete_button: gtk::Button,
    save_button: gtk::Button,
    title_row: adw::EntryRow,
    detail_view: gtk::TextView,
    detail_buffer: gtk::TextBuffer,
    /// `None` until a new note has been saved.
    note: RefCell<Option<Note>>,
    /// Set once the note is deleted, so leaving the page doesn't save it again.
    deleted: Cell<bool>,
}

impl NoteEditor {
    pub fn new(notes: &Rc<NotesPage>, note: Option<Note>) -> Rc<Self> {
        let delete_button = gtk::Button::builder()
            .label("Delete")
            .tooltip_text("Delete note")
            .css_classes(["destructive-action"])
            .build();
        let save_button = gtk::Button::builder()
            .label("Save")
            .tooltip_text("Save (Ctrl+S)")
            .css_classes(["suggested-action"])
            .build();
        // The page can't be popped by gestures, so libadwaita shows no back button of its own.
        let back_button = gtk::Button::builder()
            .icon_name("go-previous-symbolic")
            .tooltip_text("Back (Escape)")
            .build();
        for button in [&back_button, &delete_button, &save_button] {
            button.set_cursor_from_name(Some("pointer"));
        }
        let window_title = adw::WindowTitle::new("", "");
        let header = adw::HeaderBar::builder()
            .title_widget(&window_title)
            .show_back_button(false)
            .build();
        header.pack_start(&back_button);
        header.pack_end(&save_button);
        header.pack_end(&delete_button);

        let title_row = adw::EntryRow::builder().title("Title (optional)").build();
        let title_list = gtk::ListBox::builder()
            .selection_mode(gtk::SelectionMode::None)
            .css_classes(["boxed-list"])
            .margin_bottom(6)
            .build();
        title_list.append(&title_row);
        if let Some(text) = title_row
            .delegate()
            .and_then(|d| d.downcast::<gtk::Text>().ok())
        {
            trim_pasted_text_in_entry(&text);
        }
        let detail_heading = gtk::Label::builder()
            .label("Details (Markdown)")
            .xalign(0.0)
            .css_classes(["heading"])
            .build();
        let detail_view = gtk::TextView::builder()
            .monospace(true)
            .wrap_mode(gtk::WrapMode::WordChar)
            .top_margin(12)
            .bottom_margin(12)
            .left_margin(12)
            .right_margin(12)
            // Extra space between lines, and between wrapped rows of one line.
            .pixels_below_lines(4)
            .pixels_inside_wrap(2)
            .accepts_tab(true)
            .css_classes(["ticket-editor"])
            .build();
        let detail_buffer = detail_view.buffer();
        trim_pasted_text_in_view(&detail_view);
        indent_with_spaces(&detail_view);
        // Tab stops need the editor's font, which is only known once it's realized.
        detail_view.connect_realize(|view| set_tab_width(view, TAB_WIDTH_CHARS));
        let detail_scroller = gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Never)
            .vexpand(true)
            .child(&detail_view)
            .build();
        // Styled like the boxed title row above it.
        detail_scroller.add_css_class("card");
        detail_scroller.add_css_class("details-card");

        let editor = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(12)
            .build();
        editor.append(&title_list);
        editor.append(&detail_heading);
        editor.append(&detail_scroller);
        let clamp = adw::Clamp::builder()
            .maximum_size(900)
            .margin_start(18)
            .margin_end(18)
            .margin_top(12)
            .margin_bottom(18)
            .child(&editor)
            .build();

        let toolbar = adw::ToolbarView::new();
        toolbar.add_top_bar(&header);
        toolbar.set_content(Some(&clamp));
        // Not poppable by swipe gestures, Escape or the mouse's back button, so a stray swipe
        // doesn't leave the note; the back button and Escape shortcut pop it programmatically.
        let page = adw::NavigationPage::builder()
            .title("Note")
            .can_pop(false)
            .child(&toolbar)
            .build();

        if let Some(note) = &note {
            title_row.set_text(note.title.as_deref().unwrap_or_default());
            // Loading the note isn't an edit that Ctrl+Z should undo.
            detail_buffer.begin_irreversible_action();
            detail_buffer.set_text(&note.detail);
            detail_buffer.end_irreversible_action();
            detail_buffer.place_cursor(&detail_buffer.start_iter());
        }

        let this = Rc::new(Self {
            notes: Rc::downgrade(notes),
            page,
            window_title,
            back_button,
            delete_button,
            save_button,
            title_row,
            detail_view,
            detail_buffer,
            note: RefCell::new(note),
            deleted: Cell::new(false),
        });
        this.connect_signals();
        this.update_state();
        this
    }

    fn connect_signals(self: &Rc<Self>) {
        let this = Rc::clone(self);

        self.back_button.connect_clicked(clone!(
            #[weak]
            this,
            move |_| this.go_back()
        ));
        self.save_button.connect_clicked(clone!(
            #[weak]
            this,
            move |_| this.save()
        ));
        self.delete_button.connect_clicked(clone!(
            #[weak]
            this,
            move |_| this.confirm_delete()
        ));
        self.title_row.connect_changed(clone!(
            #[weak]
            this,
            move |_| this.update_state()
        ));
        self.title_row.connect_entry_activated(clone!(
            #[weak]
            this,
            move |_| {
                this.detail_view.grab_focus();
            }
        ));
        self.detail_buffer.connect_changed(clone!(
            #[weak]
            this,
            move |_| this.update_state()
        ));
        self.page.connect_shown(clone!(
            #[weak]
            this,
            move |_| {
                if this.note.borrow().is_some() {
                    this.detail_view.grab_focus();
                } else {
                    this.title_row.grab_focus();
                }
            }
        ));
        // Leaving the page, by the back button, a gesture or the section menu, keeps the changes.
        self.page.connect_hiding(clone!(
            #[weak]
            this,
            move |_| this.save()
        ));

        let weak = Rc::downgrade(self);
        glib::timeout_add_seconds_local(AUTOSAVE_SECONDS, move || {
            let Some(this) = weak.upgrade() else {
                return glib::ControlFlow::Break;
            };
            this.save();
            glib::ControlFlow::Continue
        });

        let shortcuts = gtk::ShortcutController::new();
        shortcuts.set_propagation_phase(gtk::PropagationPhase::Capture);
        shortcuts.add_shortcut(gtk::Shortcut::new(
            gtk::ShortcutTrigger::parse_string("<Control>s"),
            Some(gtk::CallbackAction::new(clone!(
                #[weak]
                this,
                #[upgrade_or]
                glib::Propagation::Proceed,
                move |_, _| {
                    this.save();
                    glib::Propagation::Stop
                }
            ))),
        ));
        shortcuts.add_shortcut(gtk::Shortcut::new(
            gtk::ShortcutTrigger::parse_string("Escape"),
            Some(gtk::CallbackAction::new(clone!(
                #[weak]
                this,
                #[upgrade_or]
                glib::Propagation::Proceed,
                move |_, _| {
                    this.go_back();
                    glib::Propagation::Stop
                }
            ))),
        ));
        self.page.add_controller(shortcuts);
    }

    pub fn page(&self) -> &adw::NavigationPage {
        &self.page
    }

    /// Returns to the page the editor was opened from. Hiding the page saves the note.
    fn go_back(&self) {
        if let Some(notes) = self.notes.upgrade() {
            notes.navigation().pop();
        }
    }

    /// The title (`None` when blank) and detail as saved.
    fn saved_content(&self) -> (Option<String>, String) {
        self.note
            .borrow()
            .as_ref()
            .map(|n| (n.title.clone(), n.detail.clone()))
            .unwrap_or_default()
    }

    /// The title (`None` when blank) and detail as currently typed.
    fn edited_content(&self) -> (Option<String>, String) {
        let title = self.title_row.text();
        let title = Some(title.trim())
            .filter(|t| !t.is_empty())
            .map(str::to_owned);
        let buffer = &self.detail_buffer;
        let detail = buffer.text(&buffer.start_iter(), &buffer.end_iter(), false);
        (title, detail.to_string())
    }

    fn is_dirty(&self) -> bool {
        !self.deleted.get() && self.edited_content() != self.saved_content()
    }

    fn update_state(&self) {
        let dirty = self.is_dirty();
        let saved = self.note.borrow().is_some();
        self.save_button.set_sensitive(dirty);
        self.delete_button.set_visible(saved);
        self.window_title
            .set_title(if saved { "Edit Note" } else { "New Note" });
        self.window_title.set_subtitle(if dirty {
            "Unsaved changes"
        } else if saved {
            "Saved"
        } else {
            ""
        });
    }

    /// Saves unsaved changes, if any. A new note is only created once something is typed.
    pub fn save(&self) {
        if !self.is_dirty() {
            return;
        }
        let Some(notes) = self.notes.upgrade() else {
            return;
        };
        let (title, detail) = self.edited_content();
        let existing = self.note.borrow().as_ref().map(|n| n.id);
        let result = match existing {
            Some(id) => notes.update_note(id, title.as_deref(), &detail),
            None if title.is_none() && detail.trim().is_empty() => return,
            None => notes.create_note(title.as_deref(), &detail),
        };
        match result {
            Ok(note) => *self.note.borrow_mut() = Some(note),
            Err(e) => notes.toast(&format!("Could not save note: {e}")),
        }
        self.update_state();
    }

    fn confirm_delete(self: &Rc<Self>) {
        let (Some(notes), Some(note)) = (self.notes.upgrade(), self.note.borrow().clone()) else {
            return;
        };
        let this = Rc::downgrade(self);
        notes.confirm_delete(&note, move || {
            if let Some(this) = this.upgrade() {
                this.deleted.set(true);
                this.update_state();
                // Not to the deleted note's view, if it was opened from there.
                if let Some(notes) = this.notes.upgrade() {
                    notes.navigation().pop_to_page(notes.page());
                }
            }
        });
    }
}
