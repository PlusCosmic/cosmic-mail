//! UI behaviour tests: the real view and key handling on Rune's headless
//! runtime, over the demo mailbox (no backend, no network).

use rune::{Env, Key, KeyEvent, Mods, Runtime};

use super::view::{on_key, view};
use super::{demo, Filter, Modal, Ui};
use crate::events::AppEvent;
use crate::sync::SyncState;

fn runtime(state: Ui) -> Runtime<Ui> {
    let env = Env {
        system_dark: true,
        scale: 1.0,
    };
    let mut rt = Runtime::new(state, view, (1280.0, 800.0), env);
    rt.on_key = Some(on_key);
    rt.frame(0.0);
    rt
}

fn step(rt: &mut Runtime<Ui>) {
    for _ in 0..3 {
        rt.frame(1.0 / 60.0);
    }
}

fn key(rt: &mut Runtime<Ui>, k: Key) {
    rt.key(KeyEvent {
        key: k,
        mods: Mods::default(),
    });
    step(rt);
}

fn typed(rt: &mut Runtime<Ui>, s: &str) {
    for c in s.chars() {
        key(rt, Key::Text(c.to_string()));
    }
}

fn ctrl(rt: &mut Runtime<Ui>, c: char) {
    rt.key(KeyEvent {
        key: Key::Command(c),
        mods: Mods {
            cmd: true,
            ctrl: true,
            ..Mods::default()
        },
    });
    step(rt);
}

fn click(rt: &mut Runtime<Ui>, key: &str) {
    let (x, y) = rt
        .find(key)
        .unwrap_or_else(|| panic!("no element keyed {key:?}"));
    rt.pointer_down(x, y);
    rt.pointer_up(x, y);
    step(rt);
}

fn poll(rt: &mut Runtime<Ui>, now: f64) {
    rt.update(|s, tx| {
        s.poll(now, tx);
    });
    step(rt);
}

#[test]
fn the_demo_mailbox_shows_every_pane() {
    let rt = runtime(demo::state());
    for key in [
        "rail",
        "header",
        "rows",
        "reader",
        "footer",
        "msg-101",
        "reader-subject",
    ] {
        assert!(rt.find(key).is_some(), "{key} is on screen");
    }
    assert!(
        rt.find("folders").is_none(),
        "no folder column in the unified view"
    );
}

#[test]
fn welcome_screen_without_accounts() {
    let mut s = demo::state();
    (demo::SCENES
        .iter()
        .find(|(n, _)| *n == "welcome")
        .unwrap()
        .1)(&mut s);
    let mut rt = runtime(s);
    assert!(rt.find("welcome").is_some());
    click(&mut rt, "welcome-add");
    assert!(matches!(rt.state.modal, Some(Modal::AddAccount(_))));
}

#[test]
fn j_and_k_walk_the_visible_messages() {
    let mut rt = runtime(demo::state());
    assert_eq!(rt.state.selected_message, Some(101));
    typed(&mut rt, "j");
    assert_eq!(rt.state.selected_message, Some(102));
    typed(&mut rt, "jj");
    assert_eq!(rt.state.selected_message, Some(104));
    typed(&mut rt, "k");
    assert_eq!(rt.state.selected_message, Some(103));
    key(&mut rt, Key::Down);
    assert_eq!(rt.state.selected_message, Some(104));
    typed(&mut rt, "gg");
    assert_eq!(rt.state.selected_message, Some(101));
    // Without a backend no more pages load: G lands on the last loaded row.
    typed(&mut rt, "G");
    assert_eq!(rt.state.selected_message, Some(108));
}

#[test]
fn clicking_a_row_selects_it() {
    let mut rt = runtime(demo::state());
    click(&mut rt, "msg-105");
    assert_eq!(rt.state.selected_message, Some(105));
}

#[test]
fn filter_chips_narrow_the_list_but_keep_the_selection() {
    let mut rt = runtime(demo::state());
    click(&mut rt, "filter-flagged");
    assert_eq!(rt.state.filter, Filter::Flagged);
    let ids: Vec<i64> = rt.state.visible_messages().iter().map(|m| m.id).collect();
    assert_eq!(ids, vec![102, 106]);
    click(&mut rt, "filter-unread");
    let ids: Vec<i64> = rt.state.visible_messages().iter().map(|m| m.id).collect();
    assert_eq!(ids, vec![101, 102, 103]);
}

#[test]
fn slash_focuses_the_filter_and_escape_clears_it() {
    let mut rt = runtime(demo::state());
    typed(&mut rt, "/");
    assert!(rt.state.query.focused);
    typed(&mut rt, "hetzner");
    assert_eq!(rt.state.query.text, "hetzner");
    let ids: Vec<i64> = rt.state.visible_messages().iter().map(|m| m.id).collect();
    assert_eq!(ids, vec![105]);
    // Typing into the field never reached the j/k shortcuts.
    assert_eq!(rt.state.selected_message, Some(101));
    key(&mut rt, Key::Escape);
    assert_eq!(rt.state.query.text, "");
    assert!(!rt.state.query.focused);
}

#[test]
fn enter_in_the_filter_runs_a_full_cache_search() {
    let mut rt = runtime(demo::state());
    typed(&mut rt, "/");
    typed(&mut rt, "invoice");
    key(&mut rt, Key::Enter);
    assert!(rt.state.search_active);
    assert_eq!(rt.state.search_query, "invoice");
    assert_eq!(rt.state.query.text, "", "the live filter clears");
    assert!(rt.find("search-status").is_some());
    click(&mut rt, "clear-search");
    assert!(!rt.state.search_active);
}

#[test]
fn the_palette_ranks_and_runs_commands() {
    let mut rt = runtime(demo::state());
    ctrl(&mut rt, 'k');
    assert!(matches!(rt.state.modal, Some(Modal::Palette(_))));
    typed(&mut rt, "archive");
    let first = match &rt.state.modal {
        Some(Modal::Palette(p)) => p.commands[p.results()[0]].title.clone(),
        _ => panic!("palette closed"),
    };
    assert_eq!(first, "Archive message");
    key(&mut rt, Key::Enter);
    assert!(rt.state.modal.is_none());
    assert!(
        rt.state.messages.iter().all(|m| m.id != 101),
        "archived optimistically"
    );
    assert_eq!(
        rt.state.selected_message,
        Some(102),
        "the next message takes over"
    );
}

#[test]
fn escape_closes_the_palette() {
    let mut rt = runtime(demo::state());
    ctrl(&mut rt, 'k');
    key(&mut rt, Key::Escape);
    assert!(rt.state.modal.is_none());
}

#[test]
fn the_move_picker_lists_the_other_folders() {
    let mut s = demo::state();
    s.open_move_picker();
    let Some(Modal::Palette(p)) = &s.modal else {
        panic!("no picker")
    };
    let titles: Vec<&str> = p.commands.iter().map(|c| c.title.as_str()).collect();
    assert!(titles.contains(&"Move to — Archive"));
    assert!(
        !titles.iter().any(|t| t.ends_with("INBOX")),
        "not the current folder"
    );
}

#[test]
fn compose_requires_a_recipient() {
    let mut rt = runtime(demo::state());
    typed(&mut rt, "c");
    assert!(matches!(rt.state.modal, Some(Modal::Compose(_))));
    click(&mut rt, "compose-send");
    match &rt.state.modal {
        Some(Modal::Compose(f)) => {
            assert_eq!(f.error.as_deref(), Some("Add at least one recipient"));
            assert!(!f.busy);
        }
        _ => panic!("compose closed"),
    }
    click(&mut rt, "compose-cancel");
    assert!(rt.state.modal.is_none());
}

#[test]
fn reply_seeds_the_sender_and_quotes() {
    let mut rt = runtime(demo::state());
    typed(&mut rt, "r");
    match &rt.state.modal {
        Some(Modal::Compose(f)) => {
            assert_eq!(f.to.text, "orders@partsshop.example");
            assert_eq!(f.subject.text, "Re: Your order has shipped");
            assert!(f.body.text.contains("> Your order has shipped."));
            assert_eq!(f.account_id, "a1");
        }
        _ => panic!("no reply"),
    }
}

#[test]
fn flag_and_read_toggle_optimistically() {
    let mut rt = runtime(demo::state());
    typed(&mut rt, "f");
    assert!(rt.state.messages.iter().any(|m| m.id == 101 && m.flagged));
    let unread_before = rt.state.unified_unread();
    typed(&mut rt, "u");
    assert!(rt.state.messages.iter().any(|m| m.id == 101 && m.seen));
    assert_eq!(rt.state.unified_unread(), unread_before - 1);
}

#[test]
fn delete_moves_the_selection_to_the_next_message() {
    let mut rt = runtime(demo::state());
    typed(&mut rt, "G");
    typed(&mut rt, "d");
    assert!(rt.state.messages.iter().all(|m| m.id != 108));
    assert_eq!(
        rt.state.selected_message,
        Some(107),
        "the last falls back to the previous"
    );
}

#[test]
fn digit_keys_switch_accounts() {
    let mut rt = runtime(demo::state());
    typed(&mut rt, "2");
    assert!(!rt.state.unified);
    assert_eq!(rt.state.selected_account.as_deref(), Some("a1"));
    assert_eq!(rt.state.selected_folder, Some(1));
    assert!(rt.find("folders").is_some(), "the folder column appears");
    typed(&mut rt, "1");
    assert!(rt.state.unified);
}

#[test]
fn rail_and_folder_clicks_navigate() {
    let mut rt = runtime(demo::state());
    click(&mut rt, "rail-account-a2");
    assert_eq!(rt.state.selected_folder, Some(11));
    click(&mut rt, "folder-12");
    assert_eq!(rt.state.selected_folder, Some(12));
    click(&mut rt, "rail-unified");
    assert!(rt.state.unified);
}

#[test]
fn new_inbox_mail_is_prepended_live() {
    let mut rt = runtime(demo::state());
    let mut fresh = rt.state.messages[0].clone();
    fresh.id = 999;
    fresh.subject = "Brand new".into();
    fresh.seen = false;
    let before = rt.state.unified_unread();
    rt.state.inbox.event(AppEvent::NewMessages {
        account_id: "a1".into(),
        folder_id: 1,
        messages: vec![fresh],
    });
    poll(&mut rt, 1.0);
    assert_eq!(rt.state.messages[0].id, 999);
    assert_eq!(rt.state.unified_unread(), before + 1);
    assert!(rt.find("msg-999").is_some());
}

#[test]
fn dead_credentials_toast_once_and_clear_on_recovery() {
    let mut rt = runtime(demo::state());
    for _ in 0..3 {
        rt.state.inbox.event(AppEvent::SyncState {
            account_id: "a2".into(),
            state: SyncState::Error,
            error: Some("invalid_grant".into()),
            needs_reauth: true,
        });
    }
    poll(&mut rt, 1.0);
    assert!(rt.state.reauth_required.contains("a2"));
    assert_eq!(rt.state.toasts.len(), 1, "one toast, not one per retry");
    assert!(rt
        .find(&format!("toast-{}", rt.state.toasts[0].id))
        .is_some());

    rt.state.inbox.event(AppEvent::SyncState {
        account_id: "a2".into(),
        state: SyncState::Syncing,
        error: None,
        needs_reauth: false,
    });
    poll(&mut rt, 2.0);
    assert!(!rt.state.reauth_required.contains("a2"));
}

#[test]
fn toasts_expire() {
    let mut rt = runtime(demo::state());
    rt.state.toast("Message sent", false);
    poll(&mut rt, 10.0);
    assert_eq!(rt.state.toasts.len(), 1);
    poll(&mut rt, 13.0);
    assert_eq!(rt.state.toasts.len(), 1, "info toasts last 4 s");
    poll(&mut rt, 14.5);
    assert!(rt.state.toasts.is_empty());
}

#[test]
fn settings_pick_the_scheme_and_appearance() {
    let mut rt = runtime(demo::state());
    click(&mut rt, "rail-settings");
    click(&mut rt, "scheme-oxblood");
    assert_eq!(rt.state.scheme, super::ColorScheme::Oxblood);
    click(&mut rt, "appearance-light");
    assert_eq!(rt.state.appearance, super::theme::Appearance::Light);
    let p = rt.state.palette(true);
    assert!(!p.dark, "a fixed light appearance ignores the desktop");
    assert_eq!(
        p,
        super::theme::Palette::new(super::ColorScheme::Oxblood, false)
    );
}

#[test]
fn settings_removal_needs_two_clicks() {
    let mut rt = runtime(demo::state());
    click(&mut rt, "rail-settings");
    assert!(matches!(rt.state.modal, Some(Modal::Settings(_))));
    click(&mut rt, "remove-a1");
    assert!(rt.find("confirm-remove-a1").is_some(), "armed, not removed");
    click(&mut rt, "cancel-remove-a1");
    assert!(rt.find("confirm-remove-a1").is_none());
    assert_eq!(rt.state.accounts.len(), 2);
}

#[test]
fn the_remote_image_banner_only_shows_for_remote_images() {
    let mut s = demo::state();
    assert!(!s.body_has_remote_images());
    s.body_lines =
        super::html::render(r#"<p>Hi</p><img src="https://t.example/x.png" alt="logo">"#);
    let mut rt = runtime(s);
    assert!(rt.find("remote-content").is_some());
    click(&mut rt, "remote-toggle");
    assert!(rt.state.remote_allowed.contains(&101));
}

#[test]
fn images_only_blitz_finds_are_still_fetched() {
    // GitHub notifications put the avatar in a flex `<div>`; html2text drops it,
    // so the image list must come from the Blitz layout too.
    let html = r#"<div style="display: flex; flex-wrap: wrap; white-space: pre-wrap; align-items: center; "><img height="20" width="20" style="border-radius:50%" src="https://avatars.example/u?s=20&amp;v=4" /><strong>bot</strong> left a comment</div><p>Body</p>"#;
    assert!(super::html::render(html)
        .iter()
        .all(|l| l.images.is_empty()));
    let mut s = demo::state();
    if let Some(b) = s.body.as_mut() {
        b.html = Some(html.into());
    }
    s.body_lines = super::html::render(html);
    s.reset_mail_layout();
    let rt = runtime(s);
    assert_eq!(
        rt.state.body_image_srcs(),
        vec!["https://avatars.example/u?s=20&v=4".to_string()]
    );
    assert!(rt.state.body_has_remote_images());
    assert!(
        rt.find("remote-content").is_some(),
        "the consent banner shows"
    );
}

#[test]
fn html_mail_is_laid_out_as_a_page_with_its_own_layout() {
    let rt = runtime(demo::state());
    let elements = rt.elements();
    let page = elements
        .iter()
        .find(|e| e.key.as_deref() == Some("mail-doc"))
        .expect("the HTML body draws as a laid-out page");
    let text: Vec<&str> = elements.iter().filter_map(|e| e.text.as_deref()).collect();
    assert!(
        !text.contains(&"Your order #114-5521 has shipped"),
        "the hidden preheader stays hidden: {text:?}"
    );
    // The nav bar's cells sit side by side on one row, inside the page.
    let rect = |want: &str| {
        elements
            .iter()
            .find(|e| e.text.as_deref() == Some(want))
            .map(|e| e.rect)
            .unwrap_or_else(|| panic!("no {want:?} in {text:?}"))
    };
    let (orders, help) = (rect("Your orders"), rect("Help"));
    assert!(help[0] > orders[0] && (help[1] - orders[1]).abs() < 1.0);
    assert!(orders[0] >= page.rect[0]);
}

#[test]
fn every_scene_renders() {
    for (name, scene) in demo::SCENES {
        let mut s = demo::state();
        scene(&mut s);
        let rt = runtime(s);
        assert!(rt.find("app").is_some(), "scene {name} renders");
    }
}

#[test]
fn without_a_drawn_titlebar_search_stays_in_the_header() {
    // A test runtime reports no chrome insets, as a Linux window with its native frame does.
    let rt = runtime(demo::state());
    assert!(rt.find("titlebar").is_none(), "no band without insets");
    assert!(rt.find("search-input").is_some(), "search in the header");
}

#[test]
#[cfg(any(target_os = "windows", target_os = "macos"))]
fn windows_and_macos_offer_no_title_bar_setting() {
    let mut rt = runtime(demo::state());
    click(&mut rt, "rail-settings");
    assert!(rt.find("setting-title-bar").is_none());
}

#[test]
#[cfg(not(any(target_os = "windows", target_os = "macos")))]
fn the_title_bar_setting_toggles() {
    let mut rt = runtime(demo::state());
    assert!(rt.state.settings.window_title_bar, "on by default");
    click(&mut rt, "rail-settings");
    click(&mut rt, "setting-title-bar");
    assert!(!rt.state.settings.window_title_bar);
    click(&mut rt, "setting-title-bar");
    assert!(rt.state.settings.window_title_bar);
}

#[test]
fn without_a_backend_nothing_stays_loading() {
    // Scenes run `init` with no backend; the welcome scene must still show.
    let mut s = demo::state();
    (demo::SCENES
        .iter()
        .find(|(n, _)| *n == "welcome")
        .unwrap()
        .1)(&mut s);
    s.init();
    assert!(!s.loading_accounts);
    assert!(runtime(s).find("welcome").is_some());
}

#[test]
fn the_notification_sound_setting_toggles() {
    let mut rt = runtime(demo::state());
    assert!(rt.state.settings.notification_sound, "on by default");
    click(&mut rt, "rail-settings");
    assert!(rt.find("test-notification").is_some());
    click(&mut rt, "setting-notification-sound");
    assert!(!rt.state.settings.notification_sound);
    click(&mut rt, "setting-notification-sound");
    assert!(rt.state.settings.notification_sound);
}
