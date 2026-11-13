#![cfg_attr(windows, windows_subsystem = "windows")]

fn main() {
    settings::run().expect("eframe failed");
}
