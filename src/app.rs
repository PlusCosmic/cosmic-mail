//! Process startup: wire the backend to the Rune UI.
//!
//! One process per session ([`desktop::claim_instance`]) owns background sync
//! for every account. Its window is only a view: closing it hides it, the tray
//! and later launches bring it back, and `--background` starts without one.
//! Headless Rune runs (`--snap`, `--tree`, `--script`, ...) render the built-in
//! demo mailbox instead and never touch the profile, the network or the
//! single-instance socket.

use std::sync::Arc;

use rune::{App, AppSpec, Tx, Waker};

use crate::backend::Backend;
use crate::desktop;
#[cfg(unix)]
use crate::desktop::{Instance, InstanceRequest};
use crate::events::{AppEvent, Events};
use crate::store;
use crate::sync;
use crate::ui::{demo, view, Inbox, Ui};

/// The UI fonts: the foundation's Instrument Sans for text and Fraunces for display, bundled
/// so every desktop renders the same face.
pub use rune_theme_foundation::{FAMILIES, FONTS};

/// The Wayland app id / X11 class (matches the desktop entry and hyprctl rules).
pub const APP_ID: &str = "cosmic-mail";

fn setup(s: &mut Ui, waker: Waker) {
    s.inbox.set_waker(waker.clone());
    s.set_image_cache(Arc::new(rune::image::ImageCache::new(waker)));
    s.init();
}

fn poll(s: &mut Ui, now: f64, tx: &mut Tx) -> bool {
    s.poll(now, tx)
}

/// The window's spec (shared by the app and headless runs).
pub fn spec() -> AppSpec<Ui> {
    AppSpec {
        scenes: demo::SCENES,
        fonts: FONTS,
        families: FAMILIES,
        min_size: Some((900.0, 560.0)),
        setup: Some(setup),
        poll: Some(poll),
        on_key: Some(view::on_key),
        ..AppSpec::new("Cosmic Mail", (1280.0, 800.0), demo::state, view::view)
    }
}

/// Debug builds only: `COSMIC_MAIL_DEMO=1` opens a real window on the demo mailbox (`--scene`
/// picks the state), with no profile, network, tray or single-instance hand-off, so it runs
/// beside an installed Cosmic Mail. For trying window behaviour (the titlebar, resizing) by hand.
fn demo_window() -> bool {
    cfg!(debug_assertions) && std::env::var_os("COSMIC_MAIL_DEMO").is_some_and(|v| v == "1")
}

/// The app around [`spec`]: on Windows and macOS the view draws the titlebar
/// (`view::titlebar`) under the window's own controls; Linux keeps its native
/// frame, which Rune leaves in place.
fn app() -> App<Ui> {
    App::new(spec()).titlebar(rune::Titlebar::new(view::TITLEBAR_HEIGHT))
}

/// Application entry point.
pub fn run() {
    rune::tooling::set_app_id(APP_ID);

    // Both rustls crypto backends end up in the dependency tree (lettre ->
    // ring, reqwest/oauth2 -> aws-lc-rs), so rustls cannot pick a process
    // default itself and would panic on the first TLS connection.
    if rustls::crypto::ring::default_provider()
        .install_default()
        .is_err()
    {
        tracing::debug!("a rustls CryptoProvider was already installed");
    }

    let _ = tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "cosmic_mail=info,warn".into()),
        )
        .try_init();

    tracing::info!(version = crate::VERSION, "starting Cosmic Mail");

    if rune::tooling::is_headless() || demo_window() {
        // Snapshots and scripted runs, or a debug demo window: the demo mailbox, nothing live.
        app().run();
        return;
    }

    let args: Vec<std::ffi::OsString> = std::env::args_os().collect();
    let background = desktop::is_background_launch(&args);
    #[cfg(unix)]
    let listener = {
        let request =
            InstanceRequest::for_launch(&args, std::env::var("XDG_ACTIVATION_TOKEN").ok());
        match desktop::claim_instance(&desktop::instance_socket_path(), &request) {
            Ok(Instance::Primary(listener)) => Some(listener),
            Ok(Instance::Secondary) => {
                tracing::info!("cosmic-mail is already running; handed the launch over");
                return;
            }
            Err(err) => {
                tracing::warn!(error = %err, "single-instance socket unavailable; continuing");
                None
            }
        }
    };
    // Windows: Rune's per-user named pipe. A later launch brings the running
    // window forward and returns false here.
    #[cfg(windows)]
    if !rune::single_instance("dev.pluscosmic.mail") {
        tracing::info!("cosmic-mail is already running; handed the launch over");
        return;
    }

    let runtime = match tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .thread_name("cosmic-mail-rt")
        .build()
    {
        Ok(rt) => rt,
        Err(err) => {
            eprintln!("cosmic-mail: could not start the async runtime: {err}");
            std::process::exit(1);
        }
    };

    let conn = match store::open() {
        Ok(conn) => conn,
        Err(err) => {
            eprintln!("cosmic-mail: could not open the mail database: {err:#}");
            std::process::exit(1);
        }
    };
    // Heal preview snippets of already-cached bodies with the current snippet
    // logic before anything reads them (cached bodies are never re-fetched).
    match store::heal_cached_snippets(&conn, sync::imap::snippet_for_body) {
        Ok(0) => {}
        Ok(count) => tracing::info!(count, "healed stale message snippets"),
        Err(err) => tracing::warn!(error = %err, "snippet healing failed"),
    }

    let mut app = app().keep_running().window_options(rune::WindowOptions {
        // Linux keeps the platform title bar (move, minimise, close) unless
        // the user turned it off for a tiling WM that draws its own borders.
        // Decorations are fixed at window creation: a change in Settings
        // applies on the next start. Windows and macOS always have a frame:
        // the app draws its titlebar into it (`view::titlebar`).
        borderless: crate::settings::HAS_TITLE_BAR_SETTING
            && !crate::settings::load_settings().window_title_bar,
        ..Default::default()
    });
    if background {
        app = app.windowless();
    }
    let handle = app.handle();

    let inbox = Inbox::default();
    inbox.set_handle(handle.clone());
    let events = {
        let inbox = inbox.clone();
        let handle = handle.clone();
        Events::new(move |event| match event {
            // Activation must work while the window is closed: go straight
            // to the window rather than through UI state.
            AppEvent::Activate => {
                handle.open();
                desktop::hyprland_focus();
            }
            other => inbox.event(other),
        })
    };
    let backend = Backend::new(conn, events.clone(), runtime.handle().clone());

    #[cfg(unix)]
    if let Some(listener) = listener {
        let handle = handle.clone();
        let backend = backend.clone();
        desktop::serve_instance(listener, move |request, reply| match request {
            InstanceRequest::Activate { token: Some(token) } => {
                handle.activate(token);
                desktop::hyprland_focus();
            }
            InstanceRequest::Activate { token: None } => {
                handle.open();
                desktop::hyprland_focus();
            }
            InstanceRequest::Background => {}
            // Postmaster's hand-off. The add validates by connecting, so it runs on the runtime
            // and answers when done; the serving thread moves on to the next connection.
            InstanceRequest::AddAccount(input) => {
                let backend = backend.clone();
                backend.runtime().clone().spawn(async move {
                    let email = input.email.clone();
                    let result = backend.add_handed_over_account(*input).await;
                    match &result {
                        Ok(_) => {
                            tracing::info!(%email, "added an account handed over by Postmaster")
                        }
                        Err(err) => {
                            tracing::warn!(%email, error = %err, "refused a handed-over account")
                        }
                    }
                    reply.send(result.map(|a| a.id).map_err(|e| e.to_string()));
                });
            }
            InstanceRequest::RemoveAccount { email } => {
                let result = backend.remove_account_by_email(&email);
                if result.is_ok() {
                    tracing::info!(%email, "removed an account at Postmaster's request");
                }
                reply.send(result.map(|()| String::new()).map_err(|e| e.to_string()));
            }
        });
    }

    let _tray = tray(handle.clone(), backend.clone());

    backend.start_all_syncs();

    let prefs_path = crate::paths::config_dir().map(|d| d.join("cosmic-mail").join("ui.json"));
    {
        let backend = backend.clone();
        let handle = handle.clone();
        app = app.init(move || {
            let mut ui = Ui::new(Some(backend), inbox);
            ui.set_window(handle);
            if let Some(path) = prefs_path {
                ui.set_prefs(rune::prefs::Prefs::at(path));
            }
            ui
        });
    }
    app.run();
    drop(runtime);
}

/// The process-lifetime tray icon: Open, Sync now, Quit.
fn tray(handle: rune::AppHandle<Ui>, backend: Arc<Backend>) -> Option<rune::tray::Tray> {
    use rune::tray::{Tray, TrayEvent, TrayIcon, TrayItem, TraySpec};

    let icon = match decode_icon(include_bytes!("../assets/icon-32.png")) {
        Some((w, h, rgba)) => TrayIcon::rgba(w, h, rgba),
        None => {
            tracing::warn!("could not decode the tray icon");
            return None;
        }
    };
    let spec = TraySpec::new(APP_ID, "Cosmic Mail", icon)
        .tooltip("Cosmic Mail")
        .menu(vec![
            TrayItem::action("open", "Open Cosmic Mail"),
            TrayItem::action("sync", "Sync now"),
            TrayItem::Separator,
            TrayItem::action("quit", "Quit"),
        ]);
    let result = Tray::new(spec, move |event| match event {
        TrayEvent::Activate { token } => {
            match token {
                Some(token) => handle.activate(token),
                None => handle.open(),
            }
            desktop::hyprland_focus();
        }
        TrayEvent::Menu(id) => match id.as_str() {
            "open" => {
                handle.open();
                desktop::hyprland_focus();
            }
            "sync" => match backend.sync_all_accounts() {
                Ok(count) => tracing::info!(accounts = count, "tray requested sync"),
                Err(err) => tracing::warn!(error = %err, "tray sync request failed"),
            },
            "quit" => {
                tracing::info!("tray requested application exit");
                handle.quit();
            }
            _ => {}
        },
        _ => {}
    });
    match result {
        Ok(tray) => Some(tray),
        Err(err) => {
            // No StatusNotifier host (a bare session, CI) must not stop startup.
            tracing::warn!(error = %err, "tray setup failed; continuing without a tray");
            None
        }
    }
}

/// Decode a PNG into straight RGBA8.
fn decode_icon(bytes: &[u8]) -> Option<(u32, u32, Vec<u8>)> {
    let mut decoder = png::Decoder::new(std::io::Cursor::new(bytes));
    decoder.set_transformations(png::Transformations::EXPAND | png::Transformations::STRIP_16);
    let mut reader = decoder.read_info().ok()?;
    let mut buf = vec![0; reader.output_buffer_size()?];
    let info = reader.next_frame(&mut buf).ok()?;
    let pixels = &buf[..info.buffer_size()];
    let rgba = match info.color_type {
        png::ColorType::Rgba => pixels.to_vec(),
        png::ColorType::Rgb => pixels
            .as_chunks::<3>()
            .0
            .iter()
            .flat_map(|[r, g, b]| [*r, *g, *b, 255])
            .collect(),
        png::ColorType::GrayscaleAlpha => pixels
            .as_chunks::<2>()
            .0
            .iter()
            .flat_map(|[g, a]| [*g, *g, *g, *a])
            .collect(),
        png::ColorType::Grayscale => pixels.iter().flat_map(|&g| [g, g, g, 255]).collect(),
        png::ColorType::Indexed => return None,
    };
    Some((info.width, info.height, rgba))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tray_icon_decodes_to_rgba() {
        let (w, h, rgba) = decode_icon(include_bytes!("../assets/icon-32.png")).unwrap();
        assert_eq!(rgba.len(), (w * h * 4) as usize);
        assert!(w > 0 && h > 0);
    }

    #[test]
    fn spec_is_a_resizable_three_pane_window() {
        let spec = spec();
        assert_eq!(spec.min_size, Some((900.0, 560.0)));
        assert!(spec.scenes.iter().any(|(n, _)| *n == "default"));
    }
}
