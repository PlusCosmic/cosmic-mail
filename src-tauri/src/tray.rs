//! Persistent desktop tray integration.
//!
//! Linux tray interaction is intentionally menu-driven: Tauri's current
//! AppIndicator backend does not expose portable pointer events, tooltips, or
//! tray bounds there, and cannot replace an attached menu.

use tauri::{
    menu::{Menu, MenuItem},
    tray::TrayIconBuilder,
    App,
};

use crate::{commands, desktop};

const TRAY_ID: &str = "cosmic-mail";
const OPEN_ID: &str = "tray.open";
const SYNC_ID: &str = "tray.sync";
const QUIT_ID: &str = "tray.quit";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TrayAction {
    Open,
    Sync,
    Quit,
}

fn action_for_id(id: &str) -> Option<TrayAction> {
    match id {
        OPEN_ID => Some(TrayAction::Open),
        SYNC_ID => Some(TrayAction::Sync),
        QUIT_ID => Some(TrayAction::Quit),
        _ => None,
    }
}

/// Create the process-lifetime tray icon and its permanently attached menu.
pub fn setup(app: &mut App) -> tauri::Result<()> {
    let open = MenuItem::with_id(app, OPEN_ID, "Open Cosmic Mail", true, None::<&str>)?;
    let sync = MenuItem::with_id(app, SYNC_ID, "Sync now", true, None::<&str>)?;
    let quit = MenuItem::with_id(app, QUIT_ID, "Quit", true, None::<&str>)?;
    let menu = Menu::with_items(app, &[&open, &sync, &quit])?;

    let mut builder = TrayIconBuilder::with_id(TRAY_ID)
        .menu(&menu)
        .on_menu_event(|app, event| match action_for_id(event.id().as_ref()) {
            Some(TrayAction::Open) => desktop::activate_main_window(app),
            Some(TrayAction::Sync) => match commands::sync_all_accounts(app) {
                Ok(count) => tracing::info!(accounts = count, "tray requested sync"),
                Err(err) => tracing::warn!(error = %err, "tray sync request failed"),
            },
            Some(TrayAction::Quit) => {
                tracing::info!("tray requested application exit");
                app.exit(0);
            }
            None => {}
        });

    if let Some(icon) = app.default_window_icon() {
        builder = builder.icon(icon.clone());
    } else {
        tracing::warn!("no default window icon is available for the tray");
    }

    builder.build(app)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_only_tray_menu_ids() {
        assert_eq!(action_for_id(OPEN_ID), Some(TrayAction::Open));
        assert_eq!(action_for_id(SYNC_ID), Some(TrayAction::Sync));
        assert_eq!(action_for_id(QUIT_ID), Some(TrayAction::Quit));
        assert_eq!(action_for_id("open-settings"), None);
    }
}
