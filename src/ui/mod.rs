mod board;
mod code_spans;
mod deleted_notes;
mod deleted_projects;
mod deleted_tickets;
mod dialog;
mod markdown_view;
mod note_editor;
mod note_view;
mod notes;
mod pointer;
mod project_dialog;
mod projects;

use std::cell::RefCell;
use std::rc::Rc;

use adw::prelude::*;
use gtk::glib::{self, clone};
use gtk::{gdk, gio};

use crate::db::Store;
use crate::model::Project;
use board::Board;
use notes::NotesPage;
use projects::ProjectsPage;

/// Sections of the window: the name in the content stack, and the label in the section menu.
const SECTIONS: [(&str, &str); 2] = [("projects", "Projects"), ("notes", "Notes")];

pub fn build(app: &adw::Application, store: Store) {
    load_css();
    let shell = Shell::new(app, store);
    shell.window.present();
    // Don't start with a button focused, where a stray Enter would open a modal.
    gtk::prelude::GtkWindowExt::set_focus(&shell.window, None::<&gtk::Widget>);

    // Signal handlers only hold weak references, so the window owns the shell until it is
    // destroyed; otherwise the shell would be dropped here and every handler would no-op.
    let keep_alive = RefCell::new(Some(Rc::clone(&shell)));
    shell.window.connect_destroy(move |_| {
        keep_alive.take();
    });
}

fn load_css() {
    let provider = gtk::CssProvider::new();
    provider.load_from_string(include_str!("style.css"));
    if let Some(display) = gdk::Display::default() {
        gtk::style_context_add_provider_for_display(
            &display,
            &provider,
            gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
        );
        // Dialogs open and close instantly. libadwaita has no per-dialog switch, only this
        // app-wide setting, so it also turns off toast and view-switch transitions.
        gtk::Settings::for_display(&display).set_gtk_enable_animations(false);
    }
}

/// Window action that switches to the section named by its string parameter.
const SECTION_ACTION: &str = "section";

/// The main window: the selected section's content, switched with each page's section menu.
struct Shell {
    window: adw::ApplicationWindow,
    store: Rc<RefCell<Store>>,
    toasts: adw::ToastOverlay,
    sections: gtk::Stack,
    /// The projects page, with a project's board pushed on top of it while one is open.
    navigation: adw::NavigationView,
    projects: Rc<ProjectsPage>,
    board: RefCell<Option<Rc<Board>>>,
    /// The notes page, with a note's editor or the deleted notes pushed on top of it.
    notes_navigation: adw::NavigationView,
    notes: Rc<NotesPage>,
}

impl Shell {
    fn new(app: &adw::Application, store: Store) -> Rc<Self> {
        let window = adw::ApplicationWindow::builder()
            .application(app)
            .title("Onix")
            .default_width(1100)
            .default_height(700)
            .width_request(600)
            .height_request(400)
            .build();
        let store = Rc::new(RefCell::new(store));
        let toasts = adw::ToastOverlay::new();

        let navigation = adw::NavigationView::new();
        let sections = gtk::Stack::new();

        let notes_navigation = adw::NavigationView::new();
        let notes = NotesPage::new(
            &window,
            &toasts,
            &notes_navigation,
            &section_menu(SECTIONS[1].1),
            Rc::clone(&store),
        );

        sections.add_named(&navigation, Some(SECTIONS[0].0));
        sections.add_named(&notes_navigation, Some(SECTIONS[1].0));
        toasts.set_child(Some(&sections));
        window.set_content(Some(&toasts));

        let shell = Rc::new_cyclic(|weak: &std::rc::Weak<Self>| {
            let weak = weak.clone();
            let projects = ProjectsPage::new(
                &window,
                &toasts,
                &navigation,
                &section_menu(SECTIONS[0].1),
                Rc::clone(&store),
                move |project| {
                    if let Some(shell) = weak.upgrade() {
                        shell.open_board(project);
                    }
                },
            );
            Self {
                window,
                store,
                toasts,
                sections,
                navigation,
                projects,
                board: RefCell::default(),
                notes_navigation,
                notes,
            }
        });
        shell.connect_signals();
        shell
    }

    fn connect_signals(self: &Rc<Self>) {
        let shell = Rc::clone(self);

        let section = gio::SimpleAction::new_stateful(
            SECTION_ACTION,
            Some(glib::VariantTy::STRING),
            &SECTIONS[0].0.to_variant(),
        );
        section.connect_activate(clone!(
            #[weak]
            shell,
            move |action, parameter| {
                let Some(name) = parameter.and_then(|p| p.str()).and_then(|p| {
                    SECTIONS
                        .iter()
                        .map(|(name, _)| name)
                        .find(|name| **name == p)
                }) else {
                    return;
                };
                action.set_state(&name.to_variant());
                shell.sections.set_visible_child_name(name);
                // Clicking "Projects" from a board goes back to the project cards.
                if *name == SECTIONS[0].0 {
                    shell.navigation.pop_to_tag(projects::PAGE_TAG);
                }
                // Likewise "Notes" from a note's editor, which saves the note.
                if *name == SECTIONS[1].0 {
                    shell.notes_navigation.pop_to_tag(notes::PAGE_TAG);
                }
            }
        ));
        self.window.add_action(&section);

        self.navigation.connect_popped(clone!(
            #[weak]
            shell,
            move |_, page| {
                let popped = shell
                    .board
                    .borrow()
                    .as_ref()
                    .is_some_and(|board| board.page() == page);
                if popped {
                    shell.board.take();
                }
            }
        ));

        let focus_search = gtk::Shortcut::new(
            gtk::ShortcutTrigger::parse_string("<Control>f"),
            Some(gtk::CallbackAction::new(clone!(
                #[weak]
                shell,
                #[upgrade_or]
                glib::Propagation::Proceed,
                move |_, _| {
                    if shell.window.visible_dialog().is_some() {
                        return glib::Propagation::Proceed;
                    }
                    if let Some(board) = shell.visible_board() {
                        board.focus_search();
                        return glib::Propagation::Stop;
                    }
                    if shell.notes_page_visible() {
                        shell.notes.focus_search();
                        return glib::Propagation::Stop;
                    }
                    glib::Propagation::Proceed
                }
            ))),
        );
        let shortcuts = gtk::ShortcutController::new();
        shortcuts.add_shortcut(focus_search);
        self.window.add_controller(shortcuts);

        // Autosave only runs every few seconds; keep what was typed since.
        self.window.connect_close_request(clone!(
            #[weak]
            shell,
            #[upgrade_or]
            glib::Propagation::Proceed,
            move |_| {
                shell.notes.save_open_editor();
                glib::Propagation::Proceed
            }
        ));
    }

    /// Shows the board of `project` on top of the project cards.
    fn open_board(&self, project: Project) {
        self.navigation.pop_to_page(self.projects.page());
        let board = Board::new(
            &self.window,
            &self.toasts,
            &self.navigation,
            &section_menu(SECTIONS[0].1),
            Rc::clone(&self.store),
            project,
        );
        self.navigation.push(board.page());
        *self.board.borrow_mut() = Some(board);
    }

    /// The open board, if it is what the window currently shows.
    fn visible_board(&self) -> Option<Rc<Board>> {
        let board = self.board.borrow().clone()?;
        let shown = self.sections.visible_child_name().as_deref() == Some(SECTIONS[0].0)
            && self.navigation.visible_page().as_ref() == Some(board.page());
        shown.then_some(board)
    }

    /// Whether the window shows the note cards.
    fn notes_page_visible(&self) -> bool {
        self.sections.visible_child_name().as_deref() == Some(SECTIONS[1].0)
            && self.notes_navigation.visible_page().as_ref() == Some(self.notes.page())
    }
}

/// A header bar drop-down that switches between the window's sections, labeled with
/// `label`, the section of the page it is on. Every content page gets its own.
fn section_menu(label: &str) -> gtk::MenuButton {
    let menu = gio::Menu::new();
    for (name, label) in SECTIONS {
        let item = gio::MenuItem::new(Some(label), None);
        item.set_action_and_target_value(
            Some(&format!("win.{SECTION_ACTION}")),
            Some(&name.to_variant()),
        );
        menu.append_item(&item);
    }
    let button = gtk::MenuButton::builder()
        .label(label)
        .always_show_arrow(true)
        .menu_model(&menu)
        .tooltip_text("Switch Section")
        .build();
    button.set_cursor_from_name(Some("pointer"));
    pointer::set_on_menu_items(button.popover());
    button
}
