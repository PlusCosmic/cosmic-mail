//! Cosmic Mail backend core: Tauri wiring, state setup, and background tasks.

mod accounts;
mod attachments;
mod auth;
mod autoconfig;
#[cfg(debug_assertions)]
mod automation;
mod commands;
mod desktop;
mod error;
mod notifications;
mod omarchy;
mod send;
mod settings;
mod shipments;
mod state;
mod store;
mod sync;
#[cfg(desktop)]
mod tray;
mod wire;

use tauri::Manager;

use crate::state::AppState;

/// Application entry point invoked from `main.rs`.
#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    // Both rustls crypto backends end up enabled in our dependency tree
    // (lettre -> ring, reqwest/oauth2 -> aws-lc-rs), so rustls cannot pick a
    // process-level default itself and would panic on the first TLS
    // connection. Install one explicitly before any TLS use.
    rustls::crypto::ring::default_provider()
        .install_default()
        .expect("install rustls ring CryptoProvider before any TLS use");

    // Initialize tracing; honor RUST_LOG, default to info for our crate.
    let _ = tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "cosmic_mail_lib=info,warn".into()),
        )
        .try_init();

    let builder = tauri::Builder::default();

    // Register single-instance ownership before every other plugin. On Linux
    // it owns a session D-Bus name; later launcher invocations ask this process
    // to activate its existing window and exit before setup can spawn sync.
    #[cfg(desktop)]
    let builder = builder.plugin(tauri_plugin_single_instance::init(|app, args, _cwd| {
        if !desktop::is_background_launch(args) {
            desktop::activate_main_window(app);
        }
    }));

    builder
        .plugin(tauri_plugin_opener::init())
        .setup(|app| {
            // Open the database and build shared state.
            let conn = store::open()?;

            // Heal preview snippets of already-cached bodies with the current
            // snippet logic. Cached bodies are never re-fetched, so rows
            // snippeted by an older cleanup version would stay wrong forever
            // without this. Runs synchronously here — before any sync task
            // (it must heal even when sync can't run, e.g. no OAuth token)
            // and before the webview fetches its first message pages, so no
            // update events are needed.
            match store::heal_cached_snippets(&conn, sync::imap::snippet_for_body) {
                Ok(0) => {}
                Ok(count) => tracing::info!(count, "healed stale message snippets"),
                Err(err) => tracing::warn!(error = %err, "snippet healing failed"),
            }

            let app_state = AppState::new(conn);
            let db = app_state.db.clone();

            app.manage(app_state);

            // Keep one tray icon and its menu attached for the lifetime of
            // the desktop process, including background-only launches. A
            // missing StatusNotifier host (e.g. a headless CI runner under
            // Xvfb) must not abort startup before the webview comes up, so a
            // tray failure is logged rather than propagated.
            #[cfg(desktop)]
            if let Err(err) = tray::setup(app) {
                tracing::warn!(error = %err, "tray setup failed; continuing without a tray");
            }

            // The window is configured hidden to avoid a login flash. Only an
            // interactive first launch should reveal it; --background is used
            // by the graphical-session service.
            if !desktop::is_background_launch(std::env::args_os()) {
                desktop::activate_main_window(app.handle());
            }

            // Spawn the omarchy theme watcher.
            omarchy::spawn_watcher(app.handle().clone());

            // Debug builds expose a loopback automation bridge for scripted
            // E2E tests (see automation.rs). Never compiled into releases.
            #[cfg(debug_assertions)]
            automation::spawn(app.handle().clone());

            // Spawn sync tasks for existing accounts.
            match accounts::load_accounts() {
                Ok(accounts) => {
                    let state = app.state::<AppState>();
                    for account in accounts {
                        state.sync.start(app.handle().clone(), db.clone(), account);
                    }
                }
                Err(err) => {
                    tracing::warn!(error = %err, "failed to load accounts at startup");
                }
            }

            Ok(())
        })
        .on_window_event(|window, event| {
            if window.label() == desktop::MAIN_WINDOW {
                if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                    api.prevent_close();
                    if let Err(err) = window.hide() {
                        tracing::warn!(error = %err, "could not hide main window");
                    }
                }
            }
        })
        .invoke_handler(tauri::generate_handler![
            commands::get_theme,
            commands::list_accounts,
            commands::add_imap_account,
            commands::start_gmail_oauth,
            commands::reauth_gmail_account,
            commands::remove_account,
            commands::list_folders,
            commands::list_messages,
            commands::list_unified_messages,
            commands::search_messages,
            commands::get_message_body,
            commands::list_shipments_for_message,
            commands::save_attachment,
            commands::mark_read,
            commands::mark_flagged,
            commands::move_message,
            commands::archive_message,
            commands::delete_message,
            commands::send_message,
            commands::sync_folder,
            commands::sync_account,
            commands::test_notification,
            commands::discover_account_config,
            commands::get_settings,
            commands::update_settings,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
