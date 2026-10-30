//! Page showing a note with its details rendered, from which it can be edited or deleted, or
//! for a deleted note, restored.

use std::cell::RefCell;
use std::ops::Range;
use std::rc::{Rc, Weak};
use std::time::Duration;

use adw::prelude::*;
use gtk::glib::{self, clone};
use gtk::pango;

use super::dialog::format_time;
use super::markdown_view;
use super::notes::{NotesPage, display_title};
use crate::markdown;
use crate::model::Note;

/// Width (px) of the search box shown over the note.
pub const SEARCH_WIDTH: i32 = 280;

const COPY_ICON: &str = "edit-copy-symbolic";
/// How long the copy button shows a check mark after copying.
const COPIED_FEEDBACK: Duration = Duration::from_millis(1500);

/// What the buttons of a deleted note's view do.
struct DeletedActions {
    restore: Box<dyn Fn()>,
    purge: Box<dyn Fn()>,
}

pub struct NoteView {
    /// Empty for a deleted note, which can't be edited, deleted or reloaded from here.
    notes: Weak<NotesPage>,
    /// Set for a deleted note, in place of editing and deleting it.
    deleted: Option<DeletedActions>,
    /// For callbacks created after construction, such as the rendered checkboxes.
    this: Weak<Self>,
    page: adw::NavigationPage,
    back_button: gtk::Button,
    edit_button: gtk::Button,
    delete_button: gtk::Button,
    restore_button: gtk::Button,
    purge_button: gtk::Button,
    copy_button: gtk::Button,
    title_label: gtk::Label,
    meta_label: gtk::Label,
    /// Holds the rendered markdown, rebuilt each time the note changes.
    details: gtk::Box,
    /// Holds the search entry in the top-right corner, revealed by Ctrl+F.
    search_bar: gtk::Revealer,
    search: gtk::SearchEntry,
    match_count: gtk::Label,
    note: RefCell<Note>,
}

impl NoteView {
    pub fn new(notes: &Rc<NotesPage>, note: Note) -> Rc<Self> {
        Self::build(Rc::downgrade(notes), note, None)
    }

    /// A read-only view of a deleted note, with buttons that call `on_restore` and
    /// `on_purge`.
    pub fn deleted(
        note: Note,
        on_restore: impl Fn() + 'static,
        on_purge: impl Fn() + 'static,
    ) -> Rc<Self> {
        let actions = DeletedActions {
            restore: Box::new(on_restore),
            purge: Box::new(on_purge),
        };
        Self::build(Weak::new(), note, Some(actions))
    }

    fn build(
        notes: Weak<NotesPage>,
        note: Note,
        deleted_actions: Option<DeletedActions>,
    ) -> Rc<Self> {
        let deleted = deleted_actions.is_some();
        let edit_button = gtk::Button::builder()
            .label("Edit")
            .tooltip_text("Edit note (E)")
            .css_classes(["suggested-action"])
            .build();
        let delete_button = gtk::Button::builder()
            .label("Delete")
            .tooltip_text("Delete note")
            .css_classes(["destructive-action"])
            .build();
        let restore_button = gtk::Button::builder()
            .label("Restore")
            .tooltip_text("Restore note")
            .css_classes(["suggested-action"])
            .build();
        let purge_button = gtk::Button::builder()
            .label("Delete Permanently")
            .tooltip_text("Erase note for good")
            .css_classes(["destructive-action"])
            .build();
        // The page can't be popped by gestures, so libadwaita shows no back button of its own.
        let back_button = gtk::Button::builder()
            .icon_name("go-previous-symbolic")
            .tooltip_text("Back (Escape)")
            .build();
        let copy_button = gtk::Button::builder()
            .icon_name(COPY_ICON)
            .tooltip_text("Copy note to clipboard (Ctrl+C)")
            .valign(gtk::Align::Start)
            .halign(gtk::Align::End)
            .hexpand(true)
            .css_classes(["flat", "circular"])
            .build();
        for button in [
            &back_button,
            &copy_button,
            &edit_button,
            &delete_button,
            &restore_button,
            &purge_button,
        ] {
            button.set_cursor_from_name(Some("pointer"));
        }
        edit_button.set_visible(!deleted);
        delete_button.set_visible(!deleted);
        restore_button.set_visible(deleted);
        purge_button.set_visible(deleted);
        let header = adw::HeaderBar::builder().show_back_button(false).build();
        header.pack_start(&back_button);
        header.pack_end(&edit_button);
        header.pack_end(&restore_button);
        header.pack_end(&delete_button);
        header.pack_end(&purge_button);

        let title_label = gtk::Label::builder()
            .wrap(true)
            .wrap_mode(gtk::pango::WrapMode::WordChar)
            .xalign(0.0)
            .selectable(true)
            .hexpand(true)
            .css_classes(["title-2"])
            .build();
        // The button stays on the right even when the note has no title to show.
        let title_row = gtk::Box::builder().spacing(12).build();
        title_row.append(&title_label);
        title_row.append(&copy_button);
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
        content.append(&title_row);
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

        let search = gtk::SearchEntry::builder()
            .placeholder_text("Find in note")
            .hexpand(true)
            .build();
        let match_count = gtk::Label::builder()
            .visible(false)
            .css_classes(["dim-label", "caption", "numeric"])
            .build();
        let search_box = gtk::Box::builder()
            .spacing(8)
            .width_request(SEARCH_WIDTH)
            .css_classes(["card", "note-search"])
            .build();
        search_box.append(&search);
        search_box.append(&match_count);
        let search_bar = gtk::Revealer::builder()
            .transition_type(gtk::RevealerTransitionType::Crossfade)
            .halign(gtk::Align::End)
            .valign(gtk::Align::Start)
            .margin_top(12)
            .margin_end(12)
            .child(&search_box)
            .build();
        let overlay = gtk::Overlay::builder().child(&scroller).build();
        overlay.add_overlay(&search_bar);

        let toolbar = adw::ToolbarView::new();
        toolbar.add_top_bar(&header);
        toolbar.set_content(Some(&overlay));
        // Not poppable by swipe gestures, Escape or the mouse's back button, so a stray
        // two-finger swipe doesn't leave the note; the back button and the Escape and Alt+Left
        // shortcuts pop it programmatically.
        let page = adw::NavigationPage::builder()
            .title("Note")
            .can_pop(false)
            .child(&toolbar)
            .build();

        let this = Rc::new_cyclic(|weak| Self {
            notes,
            deleted: deleted_actions,
            this: weak.clone(),
            page,
            back_button,
            edit_button,
            delete_button,
            restore_button,
            purge_button,
            copy_button,
            title_label,
            meta_label,
            details,
            search_bar,
            search,
            match_count,
            note: RefCell::new(note),
        });
        this.connect_signals();
        this.render();
        this
    }

    fn connect_signals(self: &Rc<Self>) {
        let this = Rc::clone(self);
        self.back_button.connect_clicked(clone!(
            #[weak]
            this,
            move |_| this.go_back()
        ));
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
        self.restore_button.connect_clicked(clone!(
            #[weak]
            this,
            move |_| {
                if let Some(deleted) = &this.deleted {
                    (deleted.restore)();
                }
            }
        ));
        self.purge_button.connect_clicked(clone!(
            #[weak]
            this,
            move |_| {
                if let Some(deleted) = &this.deleted {
                    (deleted.purge)();
                }
            }
        ));
        self.copy_button.connect_clicked(clone!(
            #[weak]
            this,
            move |_| this.copy_to_clipboard()
        ));
        // Picks up changes made in the editor pushed on top of this page.
        self.page.connect_showing(clone!(
            #[weak]
            this,
            move |_| this.reload()
        ));
        // Otherwise the first selectable label, the title or the details' first line, takes
        // the focus, which selects all of its text. The selection outlives the focus, so it is
        // cleared too.
        self.page.connect_shown(clone!(
            #[weak]
            this,
            move |_| {
                this.main_button().grab_focus();
                for label in this.labels() {
                    label.select_region(0, 0);
                }
            }
        ));

        // Bubble phase, so an "e" typed into the search entry still reaches the entry first.
        let shortcuts = gtk::ShortcutController::new();
        shortcuts.add_shortcut(gtk::Shortcut::new(
            gtk::ShortcutTrigger::parse_string("<Control>e|e"),
            Some(gtk::CallbackAction::new(clone!(
                #[weak]
                this,
                #[upgrade_or]
                glib::Propagation::Proceed,
                move |_, _| {
                    if this.deleted.is_some() {
                        return glib::Propagation::Proceed;
                    }
                    this.edit();
                    glib::Propagation::Stop
                }
            ))),
        ));
        shortcuts.add_shortcut(gtk::Shortcut::new(
            gtk::ShortcutTrigger::parse_string("<Alt>Left"),
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
        // Bubble phase too, so Ctrl+C in the search entry or a label with a selection copies
        // that text instead.
        shortcuts.add_shortcut(gtk::Shortcut::new(
            gtk::ShortcutTrigger::parse_string("<Control>c"),
            Some(gtk::CallbackAction::new(clone!(
                #[weak]
                this,
                #[upgrade_or]
                glib::Propagation::Proceed,
                move |_, _| {
                    this.copy_to_clipboard();
                    glib::Propagation::Stop
                }
            ))),
        ));
        shortcuts.add_shortcut(gtk::Shortcut::new(
            gtk::ShortcutTrigger::parse_string("<Control>f"),
            Some(gtk::CallbackAction::new(clone!(
                #[weak]
                this,
                #[upgrade_or]
                glib::Propagation::Proceed,
                move |_, _| {
                    this.open_search();
                    glib::Propagation::Stop
                }
            ))),
        ));
        self.page.add_controller(shortcuts);

        // Capture phase, so Escape closes an open search, rather than going back, wherever the
        // focus is on the page.
        let escape = gtk::ShortcutController::new();
        escape.set_propagation_phase(gtk::PropagationPhase::Capture);
        escape.add_shortcut(gtk::Shortcut::new(
            gtk::ShortcutTrigger::parse_string("Escape"),
            Some(gtk::CallbackAction::new(clone!(
                #[weak]
                this,
                #[upgrade_or]
                glib::Propagation::Proceed,
                move |_, _| {
                    if this.search_bar.reveals_child() {
                        this.close_search();
                    } else {
                        this.go_back();
                    }
                    glib::Propagation::Stop
                }
            ))),
        ));
        self.page.add_controller(escape);

        self.search.connect_search_changed(clone!(
            #[weak]
            this,
            move |_| this.highlight_matches()
        ));
        // Also closed by the Escape shortcut above, before the entry sees the key.
        self.search.connect_stop_search(clone!(
            #[weak]
            this,
            move |_| this.close_search()
        ));
    }

    /// Returns to the page the note was opened from.
    fn go_back(&self) {
        if let Some(navigation) = self
            .page
            .ancestor(adw::NavigationView::static_type())
            .and_downcast::<adw::NavigationView>()
        {
            navigation.pop();
        }
    }

    fn open_search(&self) {
        self.search_bar.set_reveal_child(true);
        self.search.grab_focus();
    }

    fn close_search(&self) {
        self.search.set_text("");
        self.search_bar.set_reveal_child(false);
        self.highlight_matches();
        self.main_button().grab_focus();
    }

    /// Edit, or Restore for a deleted note.
    fn main_button(&self) -> &gtk::Button {
        if self.deleted.is_some() {
            &self.restore_button
        } else {
            &self.edit_button
        }
    }

    /// The title and every label in the rendered details.
    fn labels(&self) -> Vec<gtk::Label> {
        let mut labels = vec![self.title_label.clone()];
        collect_labels(self.details.upcast_ref(), &mut labels);
        labels
    }

    /// Highlights the search text in the title and details, and shows how often it occurs.
    fn highlight_matches(&self) {
        let query = self.search.text();
        let count: usize = self
            .labels()
            .iter()
            .map(|label| highlight(label, &query))
            .sum();
        self.match_count.set_label(&match count {
            0 => "No matches".to_owned(),
            1 => "1 match".to_owned(),
            n => format!("{n} matches"),
        });
        self.match_count.set_visible(!query.is_empty());
    }

    /// Copies the note's details, as markdown, and briefly shows a check mark on the button.
    fn copy_to_clipboard(&self) {
        self.copy_button
            .clipboard()
            .set_text(&self.note.borrow().detail);
        self.copy_button.set_icon_name("object-select-symbolic");
        let button = self.copy_button.downgrade();
        glib::timeout_add_local_once(COPIED_FEEDBACK, move || {
            if let Some(button) = button.upgrade() {
                button.set_icon_name(COPY_ICON);
            }
        });
    }

    pub fn page(&self) -> &adw::NavigationPage {
        &self.page
    }

    fn edit(&self) {
        if self.deleted.is_some() {
            return;
        }
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
            let rendered = markdown_view::build(&note.detail, on_task_toggled);
            if self.deleted.is_some() {
                // A deleted note can't be changed, so its checkboxes only show their state.
                disable_checkboxes(rendered.upcast_ref());
            }
            self.details.append(&rendered);
        }
        // The labels were rebuilt, so their highlights are gone.
        drop(note);
        self.highlight_matches();
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

/// Stops the checkboxes inside `widget` from being clicked or focused, without dimming them.
fn disable_checkboxes(widget: &gtk::Widget) {
    let mut child = widget.first_child();
    while let Some(current) = child {
        if current.is::<gtk::CheckButton>() {
            current.set_can_target(false);
            current.set_focusable(false);
            current.set_cursor(None);
        } else {
            disable_checkboxes(&current);
        }
        child = current.next_sibling();
    }
}

/// Adds every label inside `widget` to `labels`.
fn collect_labels(widget: &gtk::Widget, labels: &mut Vec<gtk::Label>) {
    let mut child = widget.first_child();
    while let Some(current) = child {
        if let Some(label) = current.downcast_ref::<gtk::Label>() {
            labels.push(label.clone());
        } else {
            collect_labels(&current, labels);
        }
        child = current.next_sibling();
    }
}

/// Highlights the occurrences of `query` in `label`'s text, returning how many there are.
/// These attributes are merged with the label's markup rather than replacing it.
fn highlight(label: &gtk::Label, query: &str) -> usize {
    let matches = find_matches(&label.text(), query);
    if matches.is_empty() {
        label.set_attributes(None);
        return 0;
    }
    let attributes = pango::AttrList::new();
    for range in &matches {
        let (start, end) = (range.start as u32, range.end as u32);
        let mut background = pango::AttrColor::new_background(0xf6f6, 0xd3d3, 0x2d2d);
        background.set_start_index(start);
        background.set_end_index(end);
        attributes.insert(background);
        // Dark text, so the highlight stays readable in the dark style too.
        let mut foreground = pango::AttrColor::new_foreground(0, 0, 0);
        foreground.set_start_index(start);
        foreground.set_end_index(end);
        attributes.insert(foreground);
    }
    label.set_attributes(Some(&attributes));
    matches.len()
}

/// Byte ranges in `text` of the non-overlapping, case-insensitive occurrences of `query`.
pub fn find_matches(text: &str, query: &str) -> Vec<Range<usize>> {
    let query: String = query.chars().flat_map(char::to_lowercase).collect();
    if query.is_empty() {
        return Vec::new();
    }
    // Lowercasing can change a character's length, so each byte of the lowercased text
    // records where its character starts in `text`.
    let mut lowered = String::with_capacity(text.len());
    let mut origins = Vec::with_capacity(text.len());
    for (index, c) in text.char_indices() {
        for lower in c.to_lowercase() {
            lowered.push(lower);
            origins.resize(lowered.len(), index);
        }
    }
    let origin = |at: usize| origins.get(at).copied().unwrap_or(text.len());
    lowered
        .match_indices(&query)
        .map(|(start, found)| origin(start)..origin(start + found.len()))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::find_matches;

    #[test]
    fn find_matches_ignores_case() {
        assert_eq!(
            find_matches("Note: a NOTE, notes", "note"),
            [0..4, 8..12, 14..18]
        );
        assert_eq!(find_matches("aaaa", "aa"), [0..2, 2..4]);
        assert!(find_matches("note", "").is_empty());
        assert!(find_matches("note", "budget").is_empty());
    }

    #[test]
    fn find_matches_returns_ranges_in_the_original_text() {
        // "İ" lowercases to two characters, which must not shift the later match.
        let text = "İ Straße STRASSE";
        let found = find_matches(text, "straße");
        assert_eq!(found.len(), 1);
        assert_eq!(&text[found[0].clone()], "Straße");
        assert_eq!(&text[find_matches(text, "i̇")[0].clone()], "İ");
    }
}
