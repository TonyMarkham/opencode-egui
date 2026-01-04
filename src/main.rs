#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
pub mod auth;
pub mod audio;
pub mod client;
mod config;
pub mod discovery;
pub mod error; // contains api, events, discovery, spawn submodules
pub mod models_dev;
pub mod startup;
pub mod types;

#[cfg(test)]
mod tests;

use clap::Parser;
use eframe::egui;

#[derive(Parser, Debug)]
#[command(name = "opencode-egui")]
#[command(about = "OpenCode EGUI Client", long_about = None)]
struct Args {
    /// Port number to connect to OpenCode server
    #[arg(short, long)]
    port: Option<u16>,
}

fn main() -> eframe::Result {
    let args = Args::parse();

    // Store the port globally so it can be accessed during app initialization
    if let Some(port) = args.port {
        discovery::set_override_port(port);
    }

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1024.0, 720.0])
            .with_title("OpenCode EGUI"),
        ..Default::default()
    };

    eframe::run_native(
        "OpenCode EGUI",
        options,
        Box::new(|cc| Ok(Box::new(app::OpenCodeApp::new(cc)))),
    )
}
