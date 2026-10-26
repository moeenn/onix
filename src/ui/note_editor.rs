//! Page for writing a new note or editing one. Unsaved changes are saved every few seconds,
//! and when leaving the page. A note needs a title to be saved.

use std::cell::{Cell, RefCell};
use std::ops::Range;
use std::rc::{Rc, Weak};

use adw::prelude::*;
use gtk::glib::{self, clone};

use super::dialog::{
    TAB_WIDTH_CHARS, indent_with_spaces, set_tab_width, trim_pasted_text_in_entry,
    trim_pasted_text_in_view,
};
use super::note_view::{SEARCH_WIDTH, find_matches};
use super::notes::NotesPage;
use crate::model::Note;

/// How often unsaved changes are saved.
const AUTOSAVE_SECONDS: u32 = 10;

/// Saved for a note left without a title, so its changes aren't lost.
const FALLBACK_TITLE: &str = "Untitled note";

/// Name of the tag that highlights the search matches in the detail.
const MATCH_TAG: &str = "search-match";

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
    /// Holds the search entry in the top-right corner, revealed by Ctrl+F.
    search_bar: gtk::Revealer,
    search: gtk::SearchEntry,
    match_count: gtk::Label,
    previous_match_button: gtk::Button,
    next_match_button: gtk::Button,
    /// Character ranges in the detail of the search text's occurrences.
    matches: RefCell<Vec<Range<i32>>>,
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

        let title_row = adw::EntryRow::builder().title("Title").build();
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

        // Highlighted text is dark, so it stays readable in the dark style too.
        detail_buffer.create_tag(
            Some(MATCH_TAG),
            &[("background", &"#f6d32d"), ("foreground", &"black")],
        );
        let search = gtk::SearchEntry::builder()
            .placeholder_text("Find in note")
            .hexpand(true)
            .build();
        let match_count = gtk::Label::builder()
            .visible(false)
            .css_classes(["dim-label", "caption", "numeric"])
            .build();
        let previous_match_button = gtk::Button::builder()
            .icon_name("go-up-symbolic")
            .tooltip_text("Previous match (Shift+Enter)")
            .css_classes(["flat", "circular"])
            .build();
        let next_match_button = gtk::Button::builder()
            .icon_name("go-down-symbolic")
            .tooltip_text("Next match (Enter)")
            .css_classes(["flat", "circular"])
            .build();
        for button in [&previous_match_button, &next_match_button] {
            button.set_cursor_from_name(Some("pointer"));
        }
        let search_box = gtk::Box::builder()
            .spacing(4)
            .width_request(SEARCH_WIDTH)
            .css_classes(["card", "note-search"])
            .build();
        search_box.append(&search);
        search_box.append(&match_count);
        search_box.append(&previous_match_button);
        search_box.append(&next_match_button);
        let search_bar = gtk::Revealer::builder()
            .transition_type(gtk::RevealerTransitionType::Crossfade)
            .halign(gtk::Align::End)
            .valign(gtk::Align::Start)
            .margin_top(12)
            .margin_end(12)
            .child(&search_box)
            .build();
        let overlay = gtk::Overlay::builder().child(&clamp).build();
        overlay.add_overlay(&search_bar);

        let toolbar = adw::ToolbarView::new();
        toolbar.add_top_bar(&header);
        toolbar.set_content(Some(&overlay));
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
            search_bar,
            search,
            match_count,
            previous_match_button,
            next_match_button,
            matches: RefCell::default(),
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
            move |_| this.save_or_ask_for_title()
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
            move |_| {
                this.update_state();
                this.highlight_matches();
            }
        ));
        self.search.connect_search_changed(clone!(
            #[weak]
            this,
            move |_| {
                this.highlight_matches();
                this.select_match(true, true);
            }
        ));
        self.search.connect_activate(clone!(
            #[weak]
            this,
            move |_| this.select_match(true, false)
        ));
        self.search.connect_next_match(clone!(
            #[weak]
            this,
            move |_| this.select_match(true, false)
        ));
        self.search.connect_previous_match(clone!(
            #[weak]
            this,
            move |_| this.select_match(false, false)
        ));
        // Also closed by the Escape shortcut below, before the entry sees the key.
        self.search.connect_stop_search(clone!(
            #[weak]
            this,
            move |_| this.close_search()
        ));
        self.next_match_button.connect_clicked(clone!(
            #[weak]
            this,
            move |_| this.select_match(true, false)
        ));
        self.previous_match_button.connect_clicked(clone!(
            #[weak]
            this,
            move |_| this.select_match(false, false)
        ));
        let search_keys = gtk::EventControllerKey::new();
        search_keys.connect_key_pressed(clone!(
            #[weak]
            this,
            #[upgrade_or]
            glib::Propagation::Proceed,
            move |_, key, _, modifiers| {
                let enter = matches!(key, gtk::gdk::Key::Return | gtk::gdk::Key::KP_Enter);
                if enter && modifiers.contains(gtk::gdk::ModifierType::SHIFT_MASK) {
                    this.select_match(false, false);
                    return glib::Propagation::Stop;
                }
                glib::Propagation::Proceed
            }
        ));
        self.search.add_controller(search_keys);
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
            move |_| this.save_on_leave()
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
                    this.save_or_ask_for_title();
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
                    if this.search_bar.reveals_child() {
                        this.close_search();
                    } else {
                        this.go_back();
                    }
                    glib::Propagation::Stop
                }
            ))),
        ));
        // Saves and returns to the note cards, even when opened from the note's view.
        shortcuts.add_shortcut(gtk::Shortcut::new(
            gtk::ShortcutTrigger::parse_string("<Alt>Left"),
            Some(gtk::CallbackAction::new(clone!(
                #[weak]
                this,
                #[upgrade_or]
                glib::Propagation::Proceed,
                move |_, _| {
                    if !this.has_title_or_ask() {
                        return glib::Propagation::Stop;
                    }
                    this.save();
                    if let Some(notes) = this.notes.upgrade() {
                        notes.navigation().pop_to_page(notes.page());
                    }
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
    }

    pub fn page(&self) -> &adw::NavigationPage {
        &self.page
    }

    /// Returns to the page the editor was opened from. Hiding the page saves the note.
    fn go_back(&self) {
        if !self.has_title_or_ask() {
            return;
        }
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

    fn has_title(&self) -> bool {
        !self.title_row.text().trim().is_empty()
    }

    /// Whether the note has a title, or has nothing unsaved that would need one. If not, says
    /// so and focuses the title.
    fn has_title_or_ask(&self) -> bool {
        if self.has_title() || !self.is_dirty() || self.is_blank_new_note() {
            return true;
        }
        if let Some(notes) = self.notes.upgrade() {
            notes.toast("A note needs a title");
        }
        self.title_row.grab_focus();
        false
    }

    /// A new note nothing has been typed into, which isn't saved.
    fn is_blank_new_note(&self) -> bool {
        let (title, detail) = self.edited_content();
        self.note.borrow().is_none() && title.is_none() && detail.trim().is_empty()
    }

    fn update_state(&self) {
        let dirty = self.is_dirty();
        let saved = self.note.borrow().is_some();
        let has_title = self.has_title();
        self.save_button.set_sensitive(dirty && has_title);
        if has_title || self.is_blank_new_note() {
            self.title_row.remove_css_class("error");
        } else {
            self.title_row.add_css_class("error");
        }
        self.delete_button.set_visible(saved);
        self.window_title
            .set_title(if saved { "Edit Note" } else { "New Note" });
        self.window_title.set_subtitle(if dirty && !has_title {
            "Needs a title"
        } else if dirty {
            "Unsaved changes"
        } else if saved {
            "Saved"
        } else {
            ""
        });
    }

    /// Saves unsaved changes, if any, as long as the note has a title. A new note is only
    /// created once something is typed.
    pub fn save(&self) {
        self.save_with(false);
    }

    /// Saves unsaved changes, if any, such as when leaving the page. Without a title, the
    /// note keeps the one it was saved with, or else gets a placeholder, so nothing is lost.
    pub fn save_on_leave(&self) {
        self.save_with(true);
    }

    /// Saves, or says the note needs a title first, as for Ctrl+S and the Save button.
    fn save_or_ask_for_title(&self) {
        if self.has_title_or_ask() {
            self.save();
        }
    }

    fn save_with(&self, leaving: bool) {
        if !self.is_dirty() || self.is_blank_new_note() {
            return;
        }
        let Some(notes) = self.notes.upgrade() else {
            return;
        };
        let (title, detail) = self.edited_content();
        let title = match title {
            Some(title) => title,
            None if leaving => self
                .saved_content()
                .0
                .unwrap_or_else(|| FALLBACK_TITLE.to_owned()),
            None => return,
        };
        let existing = self.note.borrow().as_ref().map(|n| n.id);
        let result = match existing {
            Some(id) => notes.update_note(id, Some(&title), &detail),
            None => notes.create_note(Some(&title), &detail),
        };
        match result {
            Ok(note) => *self.note.borrow_mut() = Some(note),
            Err(e) => notes.toast(&format!("Could not save note: {e}")),
        }
        self.update_state();
    }

    fn open_search(&self) {
        self.search_bar.set_reveal_child(true);
        self.search.grab_focus();
        self.highlight_matches();
    }

    /// Hides the search and returns to the detail, with the last selected match still selected.
    fn close_search(&self) {
        self.search.set_text("");
        self.search_bar.set_reveal_child(false);
        self.highlight_matches();
        self.detail_view.grab_focus();
    }

    /// Highlights the search text in the detail, and shows how often it occurs.
    fn highlight_matches(&self) {
        let buffer = &self.detail_buffer;
        let (start, end) = buffer.bounds();
        buffer.remove_tag_by_name(MATCH_TAG, &start, &end);
        let query = self.search.text();
        let found = if self.search_bar.reveals_child() {
            char_ranges(&buffer.text(&start, &end, false), &query)
        } else {
            Vec::new()
        };
        for range in &found {
            buffer.apply_tag_by_name(
                MATCH_TAG,
                &buffer.iter_at_offset(range.start),
                &buffer.iter_at_offset(range.end),
            );
        }
        *self.matches.borrow_mut() = found;
        self.update_match_count();
    }

    /// Selects the next match after the selection, or the previous one before it, wrapping
    /// around. With `from_start`, the match at the selection's start counts as the next one,
    /// so typing more of the search text keeps the same match selected.
    fn select_match(&self, forward: bool, from_start: bool) {
        let matches = self.matches.borrow();
        let (sel_start, sel_end) = self
            .detail_buffer
            .selection_bounds()
            .map(|(s, e)| (s.offset(), e.offset()))
            .unwrap_or_else(|| {
                let at = self.detail_buffer.iter_at_mark(&self.detail_buffer.get_insert());
                (at.offset(), at.offset())
            });
        let found = if forward {
            let after = if from_start { sel_start } else { sel_end };
            matches
                .iter()
                .find(|m| m.start >= after)
                .or(matches.first())
        } else {
            matches
                .iter()
                .rev()
                .find(|m| m.end <= sel_start)
                .or(matches.last())
        };
        let Some(range) = found.cloned() else {
            return;
        };
        drop(matches);
        let mut start = self.detail_buffer.iter_at_offset(range.start);
        let end = self.detail_buffer.iter_at_offset(range.end);
        self.detail_buffer.select_range(&start, &end);
        self.detail_view
            .scroll_to_iter(&mut start, 0.1, false, 0.0, 0.0);
        self.update_match_count();
    }

    /// Shows "2 of 5" when a match is selected, or else how many there are.
    fn update_match_count(&self) {
        let matches = self.matches.borrow();
        let selected = self.detail_buffer.selection_bounds().and_then(|(s, e)| {
            matches
                .iter()
                .position(|m| m.start == s.offset() && m.end == e.offset())
        });
        self.match_count.set_label(&match (selected, matches.len()) {
            (_, 0) => "No matches".to_owned(),
            (Some(index), n) => format!("{} of {n}", index + 1),
            (None, 1) => "1 match".to_owned(),
            (None, n) => format!("{n} matches"),
        });
        let searching = !self.search.text().is_empty();
        self.match_count.set_visible(searching);
        self.previous_match_button.set_sensitive(!matches.is_empty());
        self.next_match_button.set_sensitive(!matches.is_empty());
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

/// Character offsets in `text` of the case-insensitive occurrences of `query`, as text buffer
/// iterators count them.
fn char_ranges(text: &str, query: &str) -> Vec<Range<i32>> {
    let mut chars = 0;
    let mut last_byte = 0;
    let mut offset = |byte: usize| {
        chars += text[last_byte..byte].chars().count() as i32;
        last_byte = byte;
        chars
    };
    find_matches(text, query)
        .into_iter()
        .map(|range| offset(range.start)..offset(range.end))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::char_ranges;

    #[test]
    fn char_ranges_count_characters_not_bytes() {
        assert_eq!(char_ranges("é note NOTE", "note"), [2..6, 7..11]);
        assert!(char_ranges("note", "").is_empty());
    }
}
