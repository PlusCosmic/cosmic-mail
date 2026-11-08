// Prevents additional console window on Windows in release, DO NOT REMOVE!!
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    // WebKitGTK's DMA-BUF renderer crashes the Wayland connection ("Error 71
    // (Protocol error) dispatching to Wayland display") on NVIDIA + wlroots
    // compositors like Hyprland. Disable it unless the user overrides.
    // Must be set before GTK/WebKit initialize (i.e. before tauri runs).
    #[cfg(target_os = "linux")]
    if std::env::var_os("WEBKIT_DISABLE_DMABUF_RENDERER").is_none() {
        std::env::set_var("WEBKIT_DISABLE_DMABUF_RENDERER", "1");
    }

    cosmic_mail_lib::run()
}
