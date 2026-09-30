//! Grid of project cards, with menus for editing and deleting each project.

use std::cell::RefCell;
use std::rc::Rc;

use adw::prelude::*;
use gtk::gio;
use gtk::glib::{self, clone};

use super::deleted_projects::{self, DeletedProjectsPage};
use super::{pointer, project_dialog};
use crate::db::Store;
use crate::model::Project;

/// Tag of the projects page in the navigation view, for returning to it.
pub const PAGE_TAG: &str = "projects";

const CARD_WIDTH: i32 = 240;

pub struct ProjectsPage {
    store: Rc<RefCell<Store>>,
    window: adw::ApplicationWindow,
    toasts: adw::ToastOverlay,
    page: adw::NavigationPage,
    new_button: adw::SplitButton,
    empty_new_button: gtk::Button,
    /// Pushed on top of this page from the "New Project" button's menu.
    _deleted: Rc<DeletedProjectsPage>,
    /// Shows either the "cards" grid or the "empty" status page.
    content: gtk::Stack,
    grid: gtk::FlowBox,
    /// Projects in the order of the grid's children.
    projects: RefCell<Vec<Project>>,
    /// Called when a project card is activated.
    on_open: Box<dyn Fn(Project)>,
}

impl ProjectsPage {
    /// `window` and `toasts` belong to the main window, which the page is shown in. The page
    /// adds itself and the deleted projects page to `navigation`.
    pub fn new(
        window: &adw::ApplicationWindow,
        toasts: &adw::ToastOverlay,
        navigation: &adw::NavigationView,
        section_menu: &gtk::MenuButton,
        store: Rc<RefCell<Store>>,
        on_open: impl Fn(Project) + 'static,
    ) -> Rc<Self> {
        let menu = gio::Menu::new();
        let deleted_item = gio::MenuItem::new(Some("Deleted Projects"), None);
        deleted_item.set_action_and_target_value(
            Some("navigation.push"),
            Some(&deleted_projects::PAGE_TAG.to_variant()),
        );
        menu.append_item(&deleted_item);
        let new_button = adw::SplitButton::builder()
            .label("New Project")
            .tooltip_text("Add a project")
            .dropdown_tooltip("More")
            .menu_model(&menu)
            .css_classes(["suggested-action"])
            .build();
        // Also covers the split button's drop-down half.
        new_button.set_cursor_from_name(Some("pointer"));
        pointer::set_on_menu_items(new_button.popover());
        let header = adw::HeaderBar::new();
        header.pack_start(section_menu);
        header.pack_end(&new_button);

        let grid = gtk::FlowBox::builder()
            .selection_mode(gtk::SelectionMode::None)
            .activate_on_single_click(true)
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
            .css_classes(["project-grid"])
            .build();
        let grid_scroller = gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Never)
            .vexpand(true)
            .child(&grid)
            .build();

        let empty_new_button = gtk::Button::builder()
            .label("New Project")
            .halign(gtk::Align::Center)
            .css_classes(["pill", "suggested-action"])
            .build();
        empty_new_button.set_cursor_from_name(Some("pointer"));
        let empty = adw::StatusPage::builder()
            .title("No Projects")
            .description("Create a project to start tracking its tickets.")
            .child(&empty_new_button)
            .build();

        let content = gtk::Stack::new();
        content.add_named(&grid_scroller, Some("cards"));
        content.add_named(&empty, Some("empty"));

        let toolbar = adw::ToolbarView::new();
        toolbar.add_top_bar(&header);
        toolbar.set_content(Some(&content));
        let page = adw::NavigationPage::builder()
            .title("Projects")
            .tag(PAGE_TAG)
            .child(&toolbar)
            .build();

        let deleted = DeletedProjectsPage::new(toasts, Rc::clone(&store));
        navigation.add(&page);
        navigation.add(deleted.page());

        let this = Rc::new(Self {
            store,
            window: window.clone(),
            toasts: toasts.clone(),
            page,
            new_button,
            empty_new_button,
            _deleted: deleted,
            content,
            grid,
            projects: RefCell::default(),
            on_open: Box::new(on_open),
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
            move |_| project_dialog::open(&this, None)
        ));
        self.empty_new_button.connect_clicked(clone!(
            #[weak]
            this,
            move |_| project_dialog::open(&this, None)
        ));
        // Picks up projects restored on the deleted projects page.
        self.page.connect_showing(clone!(
            #[weak]
            this,
            move |_| this.refresh()
        ));
        self.grid.connect_child_activated(clone!(
            #[weak]
            this,
            move |_, child| {
                let project = usize::try_from(child.index())
                    .ok()
                    .and_then(|i| this.projects.borrow().get(i).cloned());
                if let Some(project) = project {
                    (this.on_open)(project);
                }
            }
        ));
    }

    pub fn page(&self) -> &adw::NavigationPage {
        &self.page
    }

    pub fn window(&self) -> &adw::ApplicationWindow {
        &self.window
    }

    pub fn toast(&self, message: &str) {
        pointer::add_toast(&self.toasts, message);
    }

    /// Reloads the cards from the database.
    fn refresh(self: &Rc<Self>) {
        let projects = match self.store.borrow().list_projects() {
            Ok(projects) => projects,
            Err(e) => {
                self.toast(&format!("Could not load projects: {e}"));
                return;
            }
        };
        self.grid.remove_all();
        for project in &projects {
            let child = gtk::FlowBoxChild::builder()
                .child(&self.build_card(project))
                .build();
            child.set_cursor_from_name(Some("pointer"));
            child.update_property(&[gtk::accessible::Property::Label(&project.name)]);
            self.grid.append(&child);
        }
        self.content.set_visible_child_name(if projects.is_empty() {
            "empty"
        } else {
            "cards"
        });
        *self.projects.borrow_mut() = projects;
    }

    fn build_card(self: &Rc<Self>, project: &Project) -> gtk::Widget {
        let this = Rc::clone(self);

        let name = gtk::Label::builder()
            .label(&project.name)
            .wrap(true)
            .wrap_mode(gtk::pango::WrapMode::WordChar)
            .max_width_chars(1)
            .xalign(0.0)
            .hexpand(true)
            .valign(gtk::Align::Center)
            .css_classes(["title-4"])
            .build();

        let menu = gio::Menu::new();
        menu.append(Some("Edit"), Some("project.edit"));
        menu.append(Some("Delete"), Some("project.delete"));
        // A text label rather than an icon, so the button shows regardless of icon theme.
        let menu_button = gtk::MenuButton::builder()
            .child(&gtk::Label::new(Some("⋯")))
            .menu_model(&menu)
            .tooltip_text("Project options")
            .valign(gtk::Align::Center)
            .css_classes(["flat"])
            .build();
        menu_button.set_cursor_from_name(Some("pointer"));
        pointer::set_on_menu_items(menu_button.popover());

        let edit = gio::SimpleAction::new("edit", None);
        edit.connect_activate(clone!(
            #[weak]
            this,
            #[strong]
            project,
            move |_, _| project_dialog::open(&this, Some(project.clone()))
        ));
        let delete = gio::SimpleAction::new("delete", None);
        delete.connect_activate(clone!(
            #[weak]
            this,
            #[strong]
            project,
            move |_, _| this.confirm_delete(&project)
        ));
        let actions = gio::SimpleActionGroup::new();
        actions.add_action(&edit);
        actions.add_action(&delete);

        let card = gtk::Box::builder()
            .spacing(8)
            .width_request(CARD_WIDTH)
            .css_classes(["card", "activatable", "project-card"])
            .build();
        card.append(&name);
        card.append(&menu_button);
        card.insert_action_group("project", Some(&actions));
        card.upcast()
    }

    fn confirm_delete(self: &Rc<Self>, project: &Project) {
        let alert = adw::AlertDialog::new(
            Some("Delete project?"),
            Some(&format!(
                "“{}” and all of its tickets will be removed.",
                project.name
            )),
        );
        alert.add_responses(&[("cancel", "Cancel"), ("delete", "Delete")]);
        alert.set_response_appearance("delete", adw::ResponseAppearance::Destructive);
        alert.set_default_response(Some("cancel"));
        alert.set_close_response("cancel");
        alert.connect_map(|alert| pointer::set_on_buttons(alert.upcast_ref()));
        let this = Rc::clone(self);
        let id = project.id;
        // Check the id rather than connecting to the "delete" detail only, so no other
        // response can ever trigger the deletion.
        alert.connect_response(
            None,
            clone!(
                #[weak]
                this,
                move |_, response| {
                    if response == "delete" {
                        this.delete_project(id);
                    }
                }
            ),
        );
        alert.present(Some(&self.window));
    }

    fn delete_project(self: &Rc<Self>, id: i64) {
        let result = self.store.borrow_mut().delete_project(id);
        match result {
            Ok(()) => self.toast("Project deleted"),
            Err(e) => self.toast(&format!("Could not delete project: {e}")),
        }
        self.refresh();
    }

    pub fn create_project(self: &Rc<Self>, name: &str) -> Result<Project, String> {
        let project = self
            .store
            .borrow_mut()
            .create_project(name)
            .map_err(|e| e.to_string())?;
        self.refresh();
        Ok(project)
    }

    pub fn update_project(self: &Rc<Self>, id: i64, name: &str) -> Result<Project, String> {
        let result = self
            .store
            .borrow_mut()
            .update_project(id, name)
            .map_err(|e| e.to_string());
        // Also on failure, in case the project was deleted meanwhile.
        self.refresh();
        result
    }
}
