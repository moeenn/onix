mod db;
mod markdown;
mod model;
mod ui;

use std::cell::RefCell;

use adw::prelude::*;
use clap::Parser;
use gtk::{gio, glib};

/// Also the name of the installed desktop file and icon, so desktops can match them to the
/// app's windows.
const APP_ID: &str = "dev.onix.Onix";

/// Kanban boards for projects and their tickets, stored in `~/.config/onix/data.db`.
#[derive(Parser)]
#[command(version, about)]
struct Cli {}

fn main() -> glib::ExitCode {
    Cli::parse();

    let path = db::default_path();
    let store = match db::Store::open(&path) {
        Ok(store) => store,
        Err(e) => {
            eprintln!("error: {}: {e}", path.display());
            return glib::ExitCode::FAILURE;
        }
    };

    let app = adw::Application::builder()
        .application_id(APP_ID)
        .flags(gio::ApplicationFlags::NON_UNIQUE)
        .build();

    let opened = RefCell::new(Some(store));
    app.connect_activate(move |app| {
        // Matches the installed icon, for window managers that don't look it up by app ID.
        gtk::Window::set_default_icon_name(APP_ID);
        if let Some(store) = opened.take() {
            ui::build(app, store);
        }
    });

    // Arguments were already handled by clap; don't let GApplication parse them again.
    let program: Vec<String> = std::env::args().take(1).collect();
    app.run_with_args(&program)
}
