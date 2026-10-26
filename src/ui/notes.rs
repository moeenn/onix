//! Grid of note cards with a search bar, and the note view and editor pushed on top of it.

use std::cell::RefCell;
use std::rc::Rc;

use adw::prelude::*;
use gtk::glib::{self, clone};
use gtk::{gdk, gio};

use super::deleted_notes::{self, DeletedNotesPage};
use super::note_editor::NoteEditor;
use super::note_view::NoteView;
use super::pointer;
use crate::db::Store;
use crate::model::Note;

/// Tag of the notes page in the navigation view, for returning to it.
pub const PAGE_TAG: &str = "notes";

/// Matches the width of the columns on a project's board.
const CARD_WIDTH: i32 = 400;
const SEARCH_DEBOUNCE_MS: u32 = 250;
/// How much of a note's detail its card shows.
const PREVIEW_CHARS: usize = 60;

pub struct NotesPage {
    store: Rc<RefCell<Store>>,
    window: adw::ApplicationWindow,
    toasts: adw::ToastOverlay,
    page: adw::NavigationPage,
    navigation: adw::NavigationView,
    new_button: gtk::Button,
    empty_new_button: gtk::Button,
    search: gtk::SearchEntry,
    /// Pushed on top of this page from the search bar's menu.
    _deleted: Rc<DeletedNotesPage>,
    /// Shows the "cards" grid, the "empty" status page, or "no-results" while searching.
    content: gtk::Stack,
    grid: gtk::FlowBox,
    /// Card widgets in the grid's order, paired with the note they show.
    entries: RefCell<Vec<(gtk::FlowBoxChild, Note)>>,
    query: RefCell<String>,
    /// Kept while pushed, so its handlers stay alive.
    view: RefCell<Option<Rc<NoteView>>>,
    /// Kept while pushed, so its handlers stay alive.
    editor: RefCell<Option<Rc<NoteEditor>>>,
}

impl NotesPage {
    /// `window` and `toasts` belong to the main window, which the page is shown in. The page
    /// adds itself and the deleted notes page to `navigation`.
    pub fn new(
        window: &adw::ApplicationWindow,
        toasts: &adw::ToastOverlay,
        navigation: &adw::NavigationView,
        section_menu: &gtk::MenuButton,
        store: Rc<RefCell<Store>>,
    ) -> Rc<Self> {
        let new_button = gtk::Button::builder()
            .label("New Note")
            .tooltip_text("Add a note (Ctrl+N)")
            .css_classes(["suggested-action"])
            .build();
        new_button.set_cursor_from_name(Some("pointer"));
        let header = adw::HeaderBar::new();
        header.pack_start(section_menu);
        header.pack_end(&new_button);

        // GtkSearchEntry emits `search-changed` only after `search-delay` ms without typing.
        let search = gtk::SearchEntry::builder()
            .placeholder_text("Press / to search notes")
            .search_delay(SEARCH_DEBOUNCE_MS)
            .hexpand(true)
            .build();
        let search_bar = gtk::Box::builder()
            .margin_start(12)
            .margin_end(12)
            .margin_top(6)
            .margin_bottom(6)
            .build();
        search_bar.append(&search);

        let menu = gio::Menu::new();
        let deleted_item = gio::MenuItem::new(Some("Deleted Notes"), None);
        deleted_item.set_action_and_target_value(
            Some("navigation.push"),
            Some(&deleted_notes::PAGE_TAG.to_variant()),
        );
        menu.append_item(&deleted_item);
        // A text label rather than an icon, so the button shows regardless of icon theme.
        let menu_button = gtk::MenuButton::builder()
            .child(&gtk::Label::new(Some("⋯")))
            .menu_model(&menu)
            .tooltip_text("More")
            .margin_start(6)
            .build();
        menu_button.set_cursor_from_name(Some("pointer"));
        pointer::set_on_menu_items(menu_button.popover());
        pointer::mark_destructive_menu_items(menu_button.popover(), &["Deleted Notes"]);
        search_bar.append(&menu_button);

        let grid = card_grid();
        grid.set_activate_on_single_click(true);
        let grid_scroller = gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Never)
            .vexpand(true)
            .child(&grid)
            .build();

        let empty_new_button = gtk::Button::builder()
            .label("New Note")
            .halign(gtk::Align::Center)
            .css_classes(["pill", "suggested-action"])
            .build();
        empty_new_button.set_cursor_from_name(Some("pointer"));
        let empty = adw::StatusPage::builder()
            .title("No Notes")
            .description("Write a note to keep it here.")
            .child(&empty_new_button)
            .build();
        let no_results = adw::StatusPage::builder()
            .title("No Results")
            .description("No notes match your search.")
            .build();

        let content = gtk::Stack::new();
        content.add_named(&grid_scroller, Some("cards"));
        content.add_named(&empty, Some("empty"));
        content.add_named(&no_results, Some("no-results"));

        let toolbar = adw::ToolbarView::new();
        toolbar.add_top_bar(&header);
        toolbar.add_top_bar(&search_bar);
        toolbar.set_content(Some(&content));
        let page = adw::NavigationPage::builder()
            .title("Notes")
            .tag(PAGE_TAG)
            .child(&toolbar)
            .build();

        let deleted = DeletedNotesPage::new(toasts, navigation, Rc::clone(&store));
        navigation.add(&page);
        navigation.add(deleted.page());

        let this = Rc::new(Self {
            store,
            window: window.clone(),
            toasts: toasts.clone(),
            page,
            navigation: navigation.clone(),
            new_button,
            empty_new_button,
            search,
            _deleted: deleted,
            content,
            grid,
            entries: RefCell::default(),
            query: RefCell::default(),
            view: RefCell::default(),
            editor: RefCell::default(),
        });
        this.connect_signals();
        this.refresh();
        this
    }

    fn connect_signals(self: &Rc<Self>) {
        let this = Rc::clone(self);
        self.new_button.connect_clicked(clone!(
            #[weak]
            this,
            move |_| this.open_editor(None)
        ));
        self.empty_new_button.connect_clicked(clone!(
            #[weak]
            this,
            move |_| this.open_editor(None)
        ));
        self.search.connect_search_changed(clone!(
            #[weak]
            this,
            move |entry| {
                *this.query.borrow_mut() = entry.text().trim().to_lowercase();
                this.apply_filter();
            }
        ));
        self.search.connect_stop_search(|entry| entry.set_text(""));

        // Bubble phase, so a "/" typed into a text field still reaches that field first.
        let shortcuts = gtk::ShortcutController::new();
        shortcuts.add_shortcut(gtk::Shortcut::new(
            gtk::ShortcutTrigger::parse_string("slash"),
            Some(gtk::CallbackAction::new(clone!(
                #[weak]
                this,
                #[upgrade_or]
                glib::Propagation::Proceed,
                move |_, _| {
                    this.focus_search();
                    glib::Propagation::Stop
                }
            ))),
        ));
        shortcuts.add_shortcut(gtk::Shortcut::new(
            gtk::ShortcutTrigger::parse_string("<Control>n"),
            Some(gtk::CallbackAction::new(clone!(
                #[weak]
                this,
                #[upgrade_or]
                glib::Propagation::Proceed,
                move |_, _| {
                    this.open_editor(None);
                    glib::Propagation::Stop
                }
            ))),
        ));
        self.page.add_controller(shortcuts);
        // Picks up notes restored on the deleted notes page.
        self.page.connect_showing(clone!(
            #[weak]
            this,
            move |_| this.refresh()
        ));
        self.grid.connect_child_activated(clone!(
            #[weak]
            this,
            move |_, child| {
                let note = usize::try_from(child.index())
                    .ok()
                    .and_then(|i| this.entries.borrow().get(i).map(|(_, n)| n.id));
                if let Some(id) = note {
                    this.open_note(id);
                }
            }
        ));
        self.grid.add_controller(delete_key_controller(clone!(
            #[weak]
            this,
            move |index| this.confirm_delete_card(index)
        )));
        self.navigation.connect_popped(clone!(
            #[weak]
            this,
            move |_, page| {
                let editor_popped = this
                    .editor
                    .borrow()
                    .as_ref()
                    .is_some_and(|editor| editor.page() == page);
                if editor_popped {
                    this.editor.take();
                }
                let view_popped = this
                    .view
                    .borrow()
                    .as_ref()
                    .is_some_and(|view| view.page() == page);
                if view_popped {
                    this.view.take();
                }
            }
        ));
    }

    pub fn page(&self) -> &adw::NavigationPage {
        &self.page
    }

    pub fn navigation(&self) -> &adw::NavigationView {
        &self.navigation
    }

    pub fn focus_search(&self) {
        self.search.grab_focus();
    }

    pub fn toast(&self, message: &str) {
        pointer::add_toast(&self.toasts, message);
    }

    /// Saves the open editor's unsaved changes, if any, such as before the window closes.
    pub fn save_open_editor(&self) {
        let editor = self.editor.borrow().clone();
        if let Some(editor) = editor {
            editor.save_on_leave();
        }
    }

    pub fn load_note(&self, id: i64) -> Result<Note, String> {
        self.store.borrow().get_note(id).map_err(|e| e.to_string())
    }

    /// Shows `id`'s note with its details rendered.
    fn open_note(self: &Rc<Self>, id: i64) {
        match self.load_note(id) {
            Ok(note) => {
                self.navigation.pop_to_page(&self.page);
                let view = NoteView::new(self, note);
                self.navigation.push(view.page());
                *self.view.borrow_mut() = Some(view);
            }
            Err(e) => {
                self.toast(&e);
                self.refresh();
            }
        }
    }

    /// Shows the editor for `note`, or for a new note, on top of the note cards.
    fn open_editor(self: &Rc<Self>, note: Option<Note>) {
        self.navigation.pop_to_page(&self.page);
        self.push_editor(note);
    }

    /// Shows the editor for `note`, or for a new note, on top of the visible page.
    pub fn push_editor(self: &Rc<Self>, note: Option<Note>) {
        let editor = NoteEditor::new(self, note);
        self.navigation.push(editor.page());
        *self.editor.borrow_mut() = Some(editor);
    }

    /// Reloads the cards from the database.
    fn refresh(self: &Rc<Self>) {
        let notes = match self.store.borrow().list_notes() {
            Ok(notes) => notes,
            Err(e) => {
                self.toast(&format!("Could not load notes: {e}"));
                return;
            }
        };
        self.grid.remove_all();
        let entries: Vec<(gtk::FlowBoxChild, Note)> = notes
            .into_iter()
            .map(|note| {
                let child = gtk::FlowBoxChild::builder()
                    .child(&self.build_card(&note))
                    .build();
                child.set_cursor_from_name(Some("pointer"));
                child.update_property(&[gtk::accessible::Property::Label(&display_title(&note))]);
                self.grid.append(&child);
                (child, note)
            })
            .collect();
        *self.entries.borrow_mut() = entries;
        self.apply_filter();
    }

    /// Case-insensitive substring match on the title or detail. `query` is already lowercased.
    fn matches(note: &Note, query: &str) -> bool {
        note.title
            .as_deref()
            .is_some_and(|title| title.to_lowercase().contains(query))
            || note.detail.to_lowercase().contains(query)
    }

    fn apply_filter(&self) {
        let query = self.query.borrow();
        let entries = self.entries.borrow();
        let mut shown = 0;
        for (child, note) in entries.iter() {
            let visible = query.is_empty() || Self::matches(note, &query);
            child.set_visible(visible);
            shown += usize::from(visible);
        }
        self.content.set_visible_child_name(if entries.is_empty() {
            "empty"
        } else if shown == 0 {
            "no-results"
        } else {
            "cards"
        });
    }

    fn build_card(self: &Rc<Self>, note: &Note) -> gtk::Widget {
        let this = Rc::clone(self);
        let menu = gio::Menu::new();
        menu.append(Some("Delete"), Some("note.delete"));
        let delete = gio::SimpleAction::new("delete", None);
        delete.connect_activate(clone!(
            #[weak]
            this,
            #[strong]
            note,
            move |_, _| this.confirm_delete(&note, || {})
        ));
        let actions = gio::SimpleActionGroup::new();
        actions.add_action(&delete);

        let card = note_card(note, None, &menu);
        card.add_css_class("activatable");
        card.insert_action_group("note", Some(&actions));
        card.upcast()
    }

    /// Asks before deleting the note of the card at `index`, then keeps the focus on the grid.
    fn confirm_delete_card(self: &Rc<Self>, index: usize) {
        let Some(note) = self.entries.borrow().get(index).map(|(_, n)| n.clone()) else {
            return;
        };
        let weak = Rc::downgrade(self);
        self.confirm_delete(&note, move || {
            if let Some(this) = weak.upgrade() {
                let cards: Vec<_> = this
                    .entries
                    .borrow()
                    .iter()
                    .map(|(c, _)| c.clone())
                    .collect();
                focus_card_near(&cards, index);
            }
        });
    }

    /// Asks before deleting `note`, and calls `on_deleted` once it is.
    pub fn confirm_delete(self: &Rc<Self>, note: &Note, on_deleted: impl Fn() + 'static) {
        let alert = adw::AlertDialog::new(
            Some("Delete note?"),
            Some(&format!(
                "“{}” can be restored from Deleted Notes.",
                display_title(note)
            )),
        );
        alert.add_responses(&[("cancel", "Cancel"), ("delete", "Delete")]);
        alert.set_response_appearance("delete", adw::ResponseAppearance::Destructive);
        alert.set_default_response(Some("cancel"));
        alert.set_close_response("cancel");
        alert.connect_map(|alert| pointer::set_on_buttons(alert.upcast_ref()));
        let this = Rc::clone(self);
        let id = note.id;
        // Check the id rather than connecting to the "delete" detail only, so no other
        // response can ever trigger the deletion.
        alert.connect_response(
            None,
            clone!(
                #[weak]
                this,
                move |_, response| {
                    if response == "delete" && this.delete_note(id) {
                        on_deleted();
                    }
                }
            ),
        );
        alert.present(Some(&self.window));
    }

    /// Whether the note was deleted.
    fn delete_note(self: &Rc<Self>, id: i64) -> bool {
        let result = self.store.borrow_mut().delete_note(id);
        let deleted = match result {
            Ok(()) => {
                self.toast("Note deleted");
                true
            }
            Err(e) => {
                self.toast(&format!("Could not delete note: {e}"));
                false
            }
        };
        self.refresh();
        deleted
    }

    pub fn create_note(self: &Rc<Self>, title: Option<&str>, detail: &str) -> Result<Note, String> {
        let note = self
            .store
            .borrow_mut()
            .create_note(title, detail)
            .map_err(|e| e.to_string())?;
        self.refresh();
        Ok(note)
    }

    pub fn update_note(
        self: &Rc<Self>,
        id: i64,
        title: Option<&str>,
        detail: &str,
    ) -> Result<Note, String> {
        let result = self
            .store
            .borrow_mut()
            .update_note(id, title, detail)
            .map_err(|e| e.to_string());
        // Also on failure, in case the note was deleted meanwhile.
        self.refresh();
        result
    }
}

/// A grid of equally sized note cards.
pub fn card_grid() -> gtk::FlowBox {
    gtk::FlowBox::builder()
        .selection_mode(gtk::SelectionMode::None)
        .homogeneous(true)
        .min_children_per_line(1)
        .max_children_per_line(8)
        .column_spacing(12)
        .row_spacing(12)
        .margin_start(18)
        .margin_end(18)
        .margin_top(18)
        .margin_bottom(18)
        .valign(gtk::Align::Start)
        .css_classes(["note-grid"])
        .build()
}

/// Calls `on_delete` with the index of the grid's focused card when Delete is pressed, with
/// the focus on the card or on a button inside it.
pub fn delete_key_controller(on_delete: impl Fn(usize) + 'static) -> gtk::EventControllerKey {
    let keys = gtk::EventControllerKey::new();
    keys.connect_key_pressed(move |keys, key, _, state| {
        if !is_plain(state) || !matches!(key, gdk::Key::Delete | gdk::Key::KP_Delete) {
            return glib::Propagation::Proceed;
        }
        let grid = keys.widget();
        let index = grid
            .as_ref()
            .and_then(|grid| grid.root())
            .and_then(|root| root.focus())
            .and_then(|focus| focus.ancestor(gtk::FlowBoxChild::static_type()))
            .filter(|child| child.parent() == grid)
            .and_downcast::<gtk::FlowBoxChild>()
            .and_then(|child| usize::try_from(child.index()).ok());
        match index {
            Some(index) => {
                on_delete(index);
                glib::Propagation::Stop
            }
            None => glib::Propagation::Proceed,
        }
    });
    keys
}

/// The grid's cards, in order.
pub fn grid_cards(grid: &gtk::FlowBox) -> Vec<gtk::FlowBoxChild> {
    let mut cards = Vec::new();
    let mut child = grid.first_child();
    while let Some(current) = child {
        child = current.next_sibling();
        if let Ok(card) = current.downcast::<gtk::FlowBoxChild>() {
            cards.push(card);
        }
    }
    cards
}

/// Whether no modifier such as Ctrl or Shift is held. Caps Lock and Num Lock don't count.
pub fn is_plain(state: gdk::ModifierType) -> bool {
    !state.intersects(
        gdk::ModifierType::SHIFT_MASK
            | gdk::ModifierType::CONTROL_MASK
            | gdk::ModifierType::ALT_MASK
            | gdk::ModifierType::SUPER_MASK
            | gdk::ModifierType::META_MASK,
    )
}

/// Focuses the first visible card from `index` on, or else the last visible one before it,
/// such as after the card at `index` was removed. Waits a turn, so a dialog closing meanwhile
/// doesn't hand the focus back elsewhere.
pub fn focus_card_near(cards: &[gtk::FlowBoxChild], index: usize) {
    let after = cards.iter().skip(index).find(|c| c.is_visible());
    let card = after.or_else(|| cards.iter().take(index).rev().find(|c| c.is_visible()));
    if let Some(card) = card.cloned() {
        glib::idle_add_local_once(move || {
            card.grab_focus();
        });
    }
}

/// A card with the note's title in bold, a `meta` caption (if given), and a menu of `menu`'s
/// options. Notes saved before titles were required show the start of their detail instead.
pub fn note_card(note: &Note, meta: Option<&str>, menu: &gio::Menu) -> gtk::Box {
    let text = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(4)
        .hexpand(true)
        .build();
    // A tiny max width lets the label wrap to the card instead of widening it.
    let title = gtk::Label::builder()
        .label(display_title(note))
        .wrap(true)
        .wrap_mode(gtk::pango::WrapMode::WordChar)
        .max_width_chars(1)
        .xalign(0.0)
        .vexpand(true)
        .valign(gtk::Align::Start)
        .css_classes(["heading"])
        .build();
    text.append(&title);
    if let Some(meta) = meta {
        let meta = gtk::Label::builder()
            .label(meta)
            .xalign(0.0)
            .ellipsize(gtk::pango::EllipsizeMode::End)
            .css_classes(["dim-label", "caption"])
            .build();
        text.append(&meta);
    }

    // A text label rather than an icon, so the button shows regardless of icon theme.
    let menu_button = gtk::MenuButton::builder()
        .child(&gtk::Label::new(Some("⋯")))
        .menu_model(menu)
        .tooltip_text("Note options")
        .valign(gtk::Align::Start)
        .css_classes(["flat"])
        .build();
    menu_button.set_cursor_from_name(Some("pointer"));
    pointer::set_on_menu_items(menu_button.popover());
    pointer::mark_destructive_menu_items(menu_button.popover(), &["Delete", "Delete Permanently"]);

    let card = gtk::Box::builder()
        .spacing(8)
        .width_request(CARD_WIDTH)
        .css_classes(["card", "note-card"])
        .build();
    card.append(&text);
    card.append(&menu_button);
    card
}

/// How a note is named in messages: its title, or the start of its detail.
pub fn display_title(note: &Note) -> String {
    match &note.title {
        Some(title) => title.clone(),
        None => match preview(&note.detail) {
            excerpt if excerpt.is_empty() => "Untitled note".to_owned(),
            excerpt => excerpt,
        },
    }
}

/// The first `PREVIEW_CHARS` characters of `detail`, with runs of whitespace (including line
/// breaks) shown as single spaces.
fn preview(detail: &str) -> String {
    let text = detail.split_whitespace().collect::<Vec<_>>().join(" ");
    match text.char_indices().nth(PREVIEW_CHARS) {
        Some((end, _)) => format!("{}…", text[..end].trim_end()),
        None => text,
    }
}

#[cfg(test)]
mod tests {
    use super::{NotesPage, PREVIEW_CHARS, preview};
    use crate::model::Note;

    #[test]
    fn preview_takes_the_first_characters_on_one_line() {
        assert_eq!(
            preview("  # Title\n\n- one\n- two  "),
            "# Title - one - two"
        );
        assert_eq!(preview(""), "");
        let long = "é".repeat(PREVIEW_CHARS + 5);
        let shown = preview(&long);
        assert_eq!(
            shown.chars().count(),
            PREVIEW_CHARS + 1,
            "counts characters"
        );
        assert!(shown.ends_with('…'));
        assert_eq!(
            preview(&"a".repeat(PREVIEW_CHARS)),
            "a".repeat(PREVIEW_CHARS)
        );
    }

    #[test]
    fn search_matches_title_or_detail() {
        let note = Note {
            id: 1,
            title: Some("Meeting Notes".into()),
            detail: "Discuss the Q3 roadmap".into(),
            updated_at: chrono::Utc::now(),
        };
        assert!(NotesPage::matches(&note, "meeting"));
        assert!(NotesPage::matches(&note, "q3 road"));
        assert!(!NotesPage::matches(&note, "budget"));
        let untitled = Note {
            title: None,
            ..note
        };
        assert!(!NotesPage::matches(&untitled, "meeting"));
    }
}
