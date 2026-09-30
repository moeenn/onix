//! Page showing a note with its details rendered, from which it can be edited or deleted.

use std::cell::RefCell;
use std::ops::Range;
use std::rc::{Rc, Weak};

use adw::prelude::*;
use gtk::glib::{self, clone};

use super::dialog::format_time;
use super::markdown_view;
use super::notes::{NotesPage, display_title};
use crate::markdown;
use crate::model::Note;

pub struct NoteView {
    notes: Weak<NotesPage>,
    /// For callbacks created after construction, such as the rendered checkboxes.
    this: Weak<Self>,
    page: adw::NavigationPage,
    edit_button: gtk::Button,
    delete_button: gtk::Button,
    title_label: gtk::Label,
    meta_label: gtk::Label,
    /// Holds the rendered markdown, rebuilt each time the note changes.
    details: gtk::Box,
    note: RefCell<Note>,
}

impl NoteView {
    pub fn new(notes: &Rc<NotesPage>, note: Note) -> Rc<Self> {
        let edit_button = gtk::Button::builder()
            .label("Edit")
            .tooltip_text("Edit note (Ctrl+E)")
            .css_classes(["suggested-action"])
            .build();
        let delete_button = gtk::Button::builder()
            .label("Delete")
            .tooltip_text("Delete note")
            .css_classes(["destructive-action"])
            .build();
        for button in [&edit_button, &delete_button] {
            button.set_cursor_from_name(Some("pointer"));
        }
        let header = adw::HeaderBar::new();
        header.pack_end(&edit_button);
        header.pack_end(&delete_button);

        let title_label = gtk::Label::builder()
            .wrap(true)
            .wrap_mode(gtk::pango::WrapMode::WordChar)
            .xalign(0.0)
            .selectable(true)
            .css_classes(["title-2"])
            .build();
        let meta_label = gtk::Label::builder()
            .wrap(true)
            .xalign(0.0)
            .css_classes(["dim-label", "caption"])
            .build();
        let details = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .css_classes(["ticket-preview"])
            .build();
        let content = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(12)
            .build();
        content.append(&title_label);
        content.append(&meta_label);
        content.append(&gtk::Separator::new(gtk::Orientation::Horizontal));
        content.append(&details);
        let clamp = adw::Clamp::builder()
            .maximum_size(900)
            .margin_start(24)
            .margin_end(24)
            .margin_top(12)
            .margin_bottom(24)
            .child(&content)
            .build();
        let scroller = gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Never)
            .vexpand(true)
            .child(&clamp)
            .build();

        let toolbar = adw::ToolbarView::new();
        toolbar.add_top_bar(&header);
        toolbar.set_content(Some(&scroller));
        let page = adw::NavigationPage::builder()
            .title("Note")
            .child(&toolbar)
            .build();

        let this = Rc::new_cyclic(|weak| Self {
            notes: Rc::downgrade(notes),
            this: weak.clone(),
            page,
            edit_button,
            delete_button,
            title_label,
            meta_label,
            details,
            note: RefCell::new(note),
        });
        this.connect_signals();
        this.render();
        this
    }

    fn connect_signals(self: &Rc<Self>) {
        let this = Rc::clone(self);
        self.edit_button.connect_clicked(clone!(
            #[weak]
            this,
            move |_| this.edit()
        ));
        self.delete_button.connect_clicked(clone!(
            #[weak]
            this,
            move |_| this.confirm_delete()
        ));
        // Picks up changes made in the editor pushed on top of this page.
        self.page.connect_showing(clone!(
            #[weak]
            this,
            move |_| this.reload()
        ));
        // Otherwise the selectable title takes the focus, which selects all of its text.
        self.page.connect_shown(clone!(
            #[weak]
            this,
            move |_| {
                this.edit_button.grab_focus();
            }
        ));

        let shortcuts = gtk::ShortcutController::new();
        shortcuts.add_shortcut(gtk::Shortcut::new(
            gtk::ShortcutTrigger::parse_string("<Control>e"),
            Some(gtk::CallbackAction::new(clone!(
                #[weak]
                this,
                #[upgrade_or]
                glib::Propagation::Proceed,
                move |_, _| {
                    this.edit();
                    glib::Propagation::Stop
                }
            ))),
        ));
        self.page.add_controller(shortcuts);
    }

    pub fn page(&self) -> &adw::NavigationPage {
        &self.page
    }

    fn edit(&self) {
        if let Some(notes) = self.notes.upgrade() {
            notes.push_editor(Some(self.note.borrow().clone()));
        }
    }

    /// Reads the note again from the database, returning to the note cards if it is gone.
    fn reload(&self) {
        let Some(notes) = self.notes.upgrade() else {
            return;
        };
        let id = self.note.borrow().id;
        match notes.load_note(id) {
            Ok(note) => {
                *self.note.borrow_mut() = note;
                self.render();
            }
            Err(e) => {
                notes.toast(&e);
                // Popping while the page is still being shown is ignored, so wait a turn.
                glib::idle_add_local_once(move || {
                    notes.navigation().pop_to_page(notes.page());
                });
            }
        }
    }

    fn render(&self) {
        let note = self.note.borrow();
        self.page.set_title(&display_title(&note));
        self.title_label
            .set_label(note.title.as_deref().unwrap_or_default());
        self.title_label.set_visible(note.title.is_some());
        self.meta_label
            .set_label(&format!("Updated {}", format_time(note.updated_at)));
        while let Some(child) = self.details.first_child() {
            self.details.remove(&child);
        }
        if note.detail.trim().is_empty() {
            let empty = gtk::Label::builder()
                .label("No details.")
                .xalign(0.0)
                .css_classes(["dim-label"])
                .build();
            self.details.append(&empty);
        } else {
            let this = self.this.clone();
            let on_task_toggled = Rc::new(move |marker: &Range<usize>, checked: bool| {
                if let Some(this) = this.upgrade() {
                    this.set_task(marker, checked);
                }
            });
            self.details
                .append(&markdown_view::build(&note.detail, on_task_toggled));
        }
    }

    /// Saves a checkbox clicked in the details by rewriting its marker in the markdown.
    fn set_task(&self, marker: &Range<usize>, checked: bool) {
        let Some(notes) = self.notes.upgrade() else {
            return;
        };
        let note = self.note.borrow().clone();
        let detail = markdown::set_task(&note.detail, marker, checked);
        match notes.update_note(note.id, note.title.as_deref(), &detail) {
            Ok(updated) => {
                self.meta_label
                    .set_label(&format!("Updated {}", format_time(updated.updated_at)));
                *self.note.borrow_mut() = updated;
            }
            Err(e) => {
                notes.toast(&format!("Could not update task: {e}"));
                // Re-render from the stored note so the checkbox shows the saved state.
                let this = self.this.clone();
                glib::idle_add_local_once(move || {
                    if let Some(this) = this.upgrade() {
                        this.render();
                    }
                });
            }
        }
    }

    fn confirm_delete(&self) {
        let Some(notes) = self.notes.upgrade() else {
            return;
        };
        let note = self.note.borrow().clone();
        let weak = Rc::downgrade(&notes);
        notes.confirm_delete(&note, move || {
            if let Some(notes) = weak.upgrade() {
                notes.navigation().pop_to_page(notes.page());
            }
        });
    }
}
