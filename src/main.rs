// Release builds on Windows are GUI apps: no console window behind the UI.
// Debug builds keep the console so logs stay visible while developing.
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

fn main() {
    cosmic_mail::run();
}
