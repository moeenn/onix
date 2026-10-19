//! Grid of deleted notes, each with a menu for restoring or permanently deleting it.

use std::cell::RefCell;
use std::rc::Rc;

use adw::prelude::*;
use gtk::gio;
use gtk::glib::{self, clone};

use super::dialog::format_time;
use super::note_view::NoteView;
use super::notes::{
    card_grid, delete_key_controller, display_title, focus_card_near, grid_cards, note_card,
};
use super::pointer;
use crate::db::Store;
use crate::model::Note;

/// Tag of the page in the navigation view, for the `navigation.push` action.
pub const PAGE_TAG: &str = "deleted-notes";

pub struct DeletedNotesPage {
    store: Rc<RefCell<Store>>,
    toasts: adw::ToastOverlay,
    page: adw::NavigationPage,
    navigation: adw::NavigationView,
    /// Shows either the "cards" grid or the "empty" status page.
    content: gtk::Stack,
    grid: gtk::FlowBox,
    /// The deleted notes in the grid's order.
    notes: RefCell<Vec<Note>>,
    /// Kept while pushed, so its handlers stay alive.
    view: RefCell<Option<Rc<NoteView>>>,
}

impl DeletedNotesPage {
    /// `toasts` belongs to the main window, which the page is shown in, and `navigation` is
    /// the view the page is added to, for showing a note on top of it.
    pub fn new(
        toasts: &adw::ToastOverlay,
        navigation: &adw::NavigationView,
        store: Rc<RefCell<Store>>,
    ) -> Rc<Self> {
        let grid = card_grid();
        grid.set_activate_on_single_click(true);
        let scroller = gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Never)
            .vexpand(true)
            .child(&grid)
            .build();
        let empty = adw::StatusPage::builder()
            .title("No Deleted Notes")
            .description("Notes you delete can be restored from here.")
            .build();

        let content = gtk::Stack::new();
        content.add_named(&scroller, Some("cards"));
        content.add_named(&empty, Some("empty"));

        let toolbar = adw::ToolbarView::new();
        toolbar.add_top_bar(&adw::HeaderBar::new());
        toolbar.set_content(Some(&content));
        let page = adw::NavigationPage::builder()
            .title("Deleted Notes")
            .tag(PAGE_TAG)
            .child(&toolbar)
            .build();
        // The back button is built by libadwaita.
        page.connect_shown(|page| pointer::set_on_buttons(page.upcast_ref()));

        let shortcuts = gtk::ShortcutController::new();
        shortcuts.add_shortcut(gtk::Shortcut::new(
            gtk::ShortcutTrigger::parse_string("<Alt>Left"),
            Some(gtk::NamedAction::new("navigation.pop")),
        ));
        page.add_controller(shortcuts);

        let this = Rc::new(Self {
            store,
            toasts: toasts.clone(),
            page,
            navigation: navigation.clone(),
            content,
            grid,
            notes: RefCell::default(),
            view: RefCell::default(),
        });
        this.page.connect_showing(clone!(
            #[weak]
            this,
            move |_| this.refresh()
        ));
        this.grid.connect_child_activated(clone!(
            #[weak]
            this,
            move |_, child| {
                let note = usize::try_from(child.index())
                    .ok()
                    .and_then(|i| this.notes.borrow().get(i).cloned());
                if let Some(note) = note {
                    this.open_note(note);
                }
            }
        ));
        this.grid.add_controller(delete_key_controller(clone!(
            #[weak]
            this,
            move |index| {
                let Some(id) = this.notes.borrow().get(index).map(|n| n.id) else {
                    return;
                };
                // Keeps the focus on the grid once the card is gone.
                let weak = Rc::downgrade(&this);
                this.confirm_purge(id, move || {
                    if let Some(this) = weak.upgrade() {
                        focus_card_near(&grid_cards(&this.grid), index);
                    }
                });
            }
        )));
        this.install_actions();
        this
    }

    pub fn page(&self) -> &adw::NavigationPage {
        &self.page
    }

    /// Shows `note` read-only, with buttons for restoring it or deleting it for good.
    fn open_note(self: &Rc<Self>, note: Note) {
        let id = note.id;
        let restore = Rc::downgrade(self);
        let purge = Rc::downgrade(self);
        let view = NoteView::deleted(
            note,
            move || {
                if let Some(this) = restore.upgrade() {
                    this.restore(id);
                    this.navigation.pop_to_page(&this.page);
                }
            },
            move || {
                if let Some(this) = purge.upgrade() {
                    let back = Rc::downgrade(&this);
                    this.confirm_purge(id, move || {
                        if let Some(this) = back.upgrade() {
                            this.navigation.pop_to_page(&this.page);
                        }
                    });
                }
            },
        );
        self.navigation.push(view.page());
        *self.view.borrow_mut() = Some(view);
    }

    fn refresh(self: &Rc<Self>) {
        let notes = match self.store.borrow().list_deleted_notes() {
            Ok(notes) => notes,
            Err(e) => {
                pointer::add_toast(&self.toasts, &format!("Could not load notes: {e}"));
                return;
            }
        };
        self.grid.remove_all();
        for (note, deleted_at) in &notes {
            let card = note_card(
                note,
                Some(&format!("Deleted {}", format_time(*deleted_at))),
                &Self::build_menu(note),
            );
            card.add_css_class("activatable");
            let child = gtk::FlowBoxChild::builder().child(&card).build();
            child.set_cursor_from_name(Some("pointer"));
            child.update_property(&[gtk::accessible::Property::Label(&display_title(note))]);
            self.grid.append(&child);
        }
        self.content
            .set_visible_child_name(if notes.is_empty() { "empty" } else { "cards" });
        *self.notes.borrow_mut() = notes.into_iter().map(|(note, _)| note).collect();
    }

    /// The card's menu, whose options act on `note`.
    fn build_menu(note: &Note) -> gio::Menu {
        let menu = gio::Menu::new();
        // Targets set as typed values: in a detailed name like "restore(5)", the id would be
        // parsed as an int32, which doesn't match the actions' int64 and disables them.
        for (label, action) in [
            ("Restore", "deleted-note.restore"),
            ("Delete Permanently", "deleted-note.purge"),
        ] {
            let item = gio::MenuItem::new(Some(label), None);
            item.set_action_and_target_value(Some(action), Some(&note.id.to_variant()));
            menu.append_item(&item);
        }
        menu
    }

    /// The actions of the cards' menus. They live on the page and take the note's id, so one
    /// set serves every card.
    fn install_actions(self: &Rc<Self>) {
        let this = Rc::clone(self);
        let restore = gio::SimpleAction::new("restore", Some(glib::VariantTy::INT64));
        restore.connect_activate(clone!(
            #[weak]
            this,
            move |_, id| {
                let Some(id) = id.and_then(|id| id.get::<i64>()) else {
                    return;
                };
                // The grid is rebuilt afterwards; let the menu finish closing first.
                let this = Rc::clone(&this);
                glib::idle_add_local_once(move || this.restore(id));
            }
        ));
        let purge = gio::SimpleAction::new("purge", Some(glib::VariantTy::INT64));
        purge.connect_activate(clone!(
            #[weak]
            this,
            move |_, id| {
                if let Some(id) = id.and_then(|id| id.get::<i64>()) {
                    this.confirm_purge(id, || {});
                }
            }
        ));
        let actions = gio::SimpleActionGroup::new();
        actions.add_action(&restore);
        actions.add_action(&purge);
        self.page
            .insert_action_group("deleted-note", Some(&actions));
    }

    fn restore(self: &Rc<Self>, id: i64) {
        let result = self.store.borrow_mut().restore_note(id);
        let message = match result {
            Ok(note) => format!("“{}” restored", display_title(&note)),
            Err(e) => format!("Could not restore note: {e}"),
        };
        pointer::add_toast(&self.toasts, &message);
        self.refresh();
    }

    /// Asks before deleting the note for good, and calls `on_purged` once it is.
    fn confirm_purge(self: &Rc<Self>, id: i64, on_purged: impl Fn() + 'static) {
        let alert = adw::AlertDialog::new(
            Some("Permanently delete note?"),
            Some("The note will be erased. This cannot be undone."),
        );
        alert.add_responses(&[("cancel", "Cancel"), ("purge", "Delete Permanently")]);
        alert.set_response_appearance("purge", adw::ResponseAppearance::Destructive);
        alert.set_default_response(Some("cancel"));
        alert.set_close_response("cancel");
        alert.connect_map(|alert| pointer::set_on_buttons(alert.upcast_ref()));
        let this = Rc::clone(self);
        // Check the id rather than connecting to the "purge" detail only, so no other
        // response can ever trigger the deletion.
        alert.connect_response(
            None,
            clone!(
                #[weak]
                this,
                move |_, response| {
                    if response == "purge" && this.purge(id) {
                        on_purged();
                    }
                }
            ),
        );
        // Not the page itself, which is hidden while a note is shown on top of it.
        alert.present(Some(&self.navigation));
    }

    /// Whether the note was deleted.
    fn purge(self: &Rc<Self>, id: i64) -> bool {
        let result = self.store.borrow_mut().purge_note(id);
        let (message, purged) = match result {
            Ok(()) => ("Note permanently deleted".to_owned(), true),
            Err(e) => (format!("Could not delete note: {e}"), false),
        };
        pointer::add_toast(&self.toasts, &message);
        self.refresh();
        purged
    }
}
