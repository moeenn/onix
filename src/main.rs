mod db;
mod markdown;
mod model;
mod ui;

use std::cell::RefCell;

use adw::prelude::*;
use clap::Parser;
use gtk::{gio, glib};

/// Kanban boards for projects and their tickets, stored in `~/.config/orgx/data.db`.
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
        .application_id("dev.orgx.Orgx")
        .flags(gio::ApplicationFlags::NON_UNIQUE)
        .build();

    let opened = RefCell::new(Some(store));
    app.connect_activate(move |app| {
        if let Some(store) = opened.take() {
            ui::build(app, store);
        }
    });

    // Arguments were already handled by clap; don't let GApplication parse them again.
    let program: Vec<String> = std::env::args().take(1).collect();
    app.run_with_args(&program)
}
