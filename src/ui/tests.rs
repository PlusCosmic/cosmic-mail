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

/// Scrolls `key` into view first, as a user would, so targets below a scroll fold
/// (Settings on a short window) stay clickable.
fn click(rt: &mut Runtime<Ui>, key: &str) {
    let k = key.to_string();
    rt.update(move |_, tx| {
        tx.scroll_into_view(k);
    });
    step(rt);
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
fn blocked_images_keep_their_box_and_show_their_alt_text() {
    let html = r#"<p>Hello</p><img src="https://cdn.example/hero.png" width="600" height="200"
                  alt="Spring sale" style="display:block;width:100%;height:auto"><p>After</p>"#;
    let mut s = demo::state();
    if let Some(b) = s.body.as_mut() {
        b.html = Some(html.into());
    }
    s.body_lines = super::html::render(html);
    s.reset_mail_layout();
    let rt = runtime(s);
    assert!(
        rt.state.body_images.is_empty(),
        "remote images stay blocked"
    );
    let elements = rt.elements();
    let rect = |want: &str| {
        elements
            .iter()
            .find(|e| e.text.as_deref() == Some(want))
            .map(|e| e.rect)
            .unwrap_or_else(|| panic!("no {want:?}"))
    };
    let (hello, alt, after) = (rect("Hello"), rect("Spring sale"), rect("After"));
    assert!(
        alt[1] > hello[1] && after[1] > alt[1] + 100.0,
        "the 3:1 box is held"
    );
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

/// Press on the text keyed `from` and drag to the centre of the element keyed `to`.
fn drag_select(rt: &mut Runtime<Ui>, from: &str, to: &str) {
    let (r, _) = rt.presented(from).expect("drag start");
    let (x0, y0) = (r[0] + 2.0, r[1] + r[3] * 0.5);
    let (x1, y1) = rt.find(to).expect("drag end");
    rt.pointer_move(x0, y0);
    rt.pointer_down(x0, y0);
    for i in 1..=10 {
        let t = i as f32 / 10.0;
        rt.pointer_move(x0 + (x1 - x0) * t, y0 + (y1 - y0) * t);
        rt.frame(1.0 / 60.0);
    }
    rt.pointer_up(x1, y1);
    step(rt);
}

#[test]
fn a_drag_selects_across_the_lines_of_an_html_mail() {
    let mut rt = runtime(demo::state());
    // The demo mail's lines are their own text elements; md-16 is "Order #114-5521 …",
    // md-20 a line of the order table below it.
    drag_select(&mut rt, "md-16", "md-20");
    let picked = rt.selected_text().unwrap_or_default();
    assert!(
        picked.starts_with("Order #114-5521") && picked.contains("Track parcel"),
        "the selection crosses lines: {picked:?}"
    );
}

#[test]
fn a_drag_selects_from_the_subject_into_the_body() {
    let mut rt = runtime(demo::state());
    drag_select(&mut rt, "reader-subject", "md-16");
    let picked = rt.selected_text().unwrap_or_default();
    assert!(
        picked.contains("Your order has shipped!"),
        "subject and body are one selection: {picked:?}"
    );
}

fn update_info(version: &str, ready: bool) -> crate::update::UpdateInfo {
    crate::update::UpdateInfo {
        version: version.into(),
        current_version: "0.1.149".into(),
        release_url: format!("https://github.com/PlusCosmic/cosmic-mail/releases/tag/v{version}"),
        method: crate::update::UpdateMethod::Installer,
        ready,
    }
}

#[test]
fn an_available_update_toasts_once_and_shows_in_settings() {
    let mut rt = runtime(demo::state());
    for _ in 0..2 {
        rt.state
            .inbox
            .event(AppEvent::UpdateAvailable(update_info("0.1.160", false)));
    }
    poll(&mut rt, 1.0);
    assert_eq!(rt.state.toasts.len(), 1, "one toast per version");
    assert!(rt.state.toasts[0].message.contains("0.1.160"));

    // A newer one toasts again.
    rt.state
        .inbox
        .event(AppEvent::UpdateAvailable(update_info("0.1.161", false)));
    poll(&mut rt, 2.0);
    assert_eq!(rt.state.toasts.len(), 2);

    click(&mut rt, "rail-settings");
    assert!(rt.find("update-install").is_some());
    assert!(rt.find("update-download").is_none());
}

#[test]
fn a_ready_automatic_update_waits_for_the_window_to_close() {
    let mut rt = runtime(demo::state());
    rt.state.settings.updates = crate::update::UpdateMode::Auto;
    rt.state
        .inbox
        .event(AppEvent::UpdateAvailable(update_info("0.1.160", true)));
    poll(&mut rt, 1.0);
    // No window handle here, so it counts as open: the user is told, nothing restarts.
    assert_eq!(rt.state.toasts.len(), 1);
    assert!(rt.state.toasts[0].message.contains("close the window"));
    assert!(rt.state.update.as_ref().is_some_and(|u| u.ready));
}

#[test]
fn a_copy_that_cant_replace_itself_offers_the_download_page() {
    let mut s = demo::state();
    s.update = Some(crate::update::UpdateInfo {
        method: crate::update::UpdateMethod::Download,
        ..update_info("0.1.160", false)
    });
    let mut rt = runtime(s);
    click(&mut rt, "rail-settings");
    assert!(rt.find("update-download").is_some());
    assert!(rt.find("update-install").is_none());
}

#[test]
fn the_update_mode_setting_switches() {
    use crate::update::UpdateMode;
    let mut rt = runtime(demo::state());
    assert_eq!(
        rt.state.settings.updates,
        UpdateMode::Notify,
        "notify by default"
    );
    click(&mut rt, "rail-settings");
    rt.state.reveal = Some("update-mode".into());
    poll(&mut rt, 1.0);
    click(&mut rt, "update-mode-off");
    assert_eq!(rt.state.settings.updates, UpdateMode::Off);
    if crate::update::CAN_INSTALL {
        click(&mut rt, "update-mode-auto");
        assert_eq!(rt.state.settings.updates, UpdateMode::Auto);
    } else {
        assert!(rt.find("update-mode-auto").is_none());
    }
}

#[test]
fn account_tags_tell_same_domain_accounts_apart() {
    use super::view::chrome::account_tag;
    use crate::accounts::{AccountKind, AccountPublic};
    let acc = |id: &str, email: &str, kind| AccountPublic {
        id: id.into(),
        email: email.into(),
        display_name: "Harry".into(),
        kind,
    };
    let mut s = demo::state();
    s.accounts = vec![
        acc("a", "harry@pluscosmic.dev", AccountKind::Imap),
        acc("b", "harry2@pluscosmic.dev", AccountKind::Imap),
        acc("c", "harry@reelshelf.app", AccountKind::Imap),
        acc("d", "one@gmail.com", AccountKind::Gmail),
    ];
    let tags: Vec<String> = ["a", "b", "c", "d"]
        .iter()
        .map(|id| account_tag(&s, id))
        .collect();
    assert_eq!(
        tags,
        [
            "harry@pluscosmic",
            "harry2@pluscosmic",
            "reelshelf",
            "Gmail"
        ]
    );

    s.accounts
        .push(acc("e", "two@gmail.com", AccountKind::Gmail));
    assert_eq!(account_tag(&s, "d"), "one@gmail");
    assert_eq!(account_tag(&s, "e"), "two@gmail");
}

#[test]
fn settings_edits_an_accounts_display_name() {
    let mut rt = runtime(demo::state());
    click(&mut rt, "rail-settings");
    let name = |rt: &Runtime<Ui>| {
        rt.state
            .accounts
            .iter()
            .find(|a| a.id == "a1")
            .map(|a| a.display_name.clone())
    };

    // Escape leaves the name as it was.
    click(&mut rt, "rename-account-a1");
    assert!(rt.find("rename-a1").is_some());
    typed(&mut rt, " Leach");
    key(&mut rt, Key::Escape);
    assert!(rt.find("rename-a1").is_none());
    assert_eq!(name(&rt).as_deref(), Some("Harry"));

    // Enter saves it, trimmed.
    click(&mut rt, "rename-account-a1");
    typed(&mut rt, " Leach  ");
    key(&mut rt, Key::Enter);
    assert!(rt.find("rename-a1").is_none());
    assert_eq!(name(&rt).as_deref(), Some("Harry Leach"));
    assert!(matches!(rt.state.modal, Some(Modal::Settings(_))));
}

#[test]
fn settings_picks_the_account_marker() {
    use crate::settings::AccountMarker;
    let mut rt = runtime(demo::state());
    assert_eq!(rt.state.settings.account_marker, AccountMarker::RightInset);
    click(&mut rt, "rail-settings");
    for (key, marker) in [
        ("account-marker-label", AccountMarker::Label),
        ("account-marker-leftLine", AccountMarker::LeftLine),
        ("account-marker-leftInset", AccountMarker::LeftInset),
        ("account-marker-rightLine", AccountMarker::RightLine),
        ("account-marker-rightInset", AccountMarker::RightInset),
    ] {
        click(&mut rt, key);
        assert_eq!(rt.state.settings.account_marker, marker);
    }
}

#[test]
fn accounts_take_distinct_colours_in_order() {
    let s = demo::state();
    let p = s.palette(true);
    let a = p.slot(s.account_slot("a1"));
    let b = p.slot(s.account_slot("a2"));
    assert_eq!(a, p.accent, "the first account wears the accent");
    assert_ne!(a, b);
}

#[test]
fn clicking_through_unread_messages_updates_indicators_and_counts_immediately() {
    let mut rt = runtime(demo::state());
    let unread_before = rt.state.unified_unread();
    // Demo starts with 101, 102, 103 unread and 101 selected (not yet clicked).
    click(&mut rt, "msg-102");
    assert_eq!(rt.state.selected_message, Some(102));
    assert!(rt.state.messages.iter().any(|m| m.id == 102 && m.seen));
    assert_eq!(rt.state.unified_unread(), unread_before - 1);

    click(&mut rt, "msg-103");
    assert_eq!(rt.state.selected_message, Some(103));
    assert!(rt.state.messages.iter().any(|m| m.id == 103 && m.seen));
    assert_eq!(rt.state.unified_unread(), unread_before - 2);
}

#[test]
fn add_account_tls_toggle_syncs_default_ports_and_preserves_custom_ports() {
    use crate::accounts::ImapTls;

    let mut rt = runtime(demo::state());
    click(&mut rt, "rail-add");
    assert!(matches!(rt.state.modal, Some(Modal::AddAccount(_))));

    // The redundant eyebrow copy is gone.
    assert!(
        rt.elements()
            .iter()
            .filter_map(|e| e.text.as_deref())
            .all(|t| !t.contains("ANYTHING ELSE USES IMAP")),
        "redundant add-account eyebrow should be removed"
    );

    // Starts on SSL/TLS : 993.
    match &rt.state.modal {
        Some(Modal::AddAccount(f)) => {
            assert_eq!(f.imap_tls, ImapTls::Implicit);
            assert_eq!(f.imap_port.text, "993");
        }
        _ => panic!("expected add-account modal"),
    }

    // Switching to STARTTLS swaps default 993 -> 143, and switching back swaps 143 -> 993.
    click(&mut rt, "add-imap-tls-starttls");
    match &rt.state.modal {
        Some(Modal::AddAccount(f)) => {
            assert_eq!(f.imap_tls, ImapTls::Starttls);
            assert_eq!(f.imap_port.text, "143");
        }
        _ => panic!("expected add-account modal"),
    }
    click(&mut rt, "add-imap-tls-implicit");
    match &rt.state.modal {
        Some(Modal::AddAccount(f)) => {
            assert_eq!(f.imap_tls, ImapTls::Implicit);
            assert_eq!(f.imap_port.text, "993");
        }
        _ => panic!("expected add-account modal"),
    }

    // Custom ports (e.g. Proton Bridge 1143) stay intact when toggling security mode.
    rt.update(|s, _| {
        if let Some(Modal::AddAccount(f)) = &mut s.modal {
            f.imap_port.replace_text("1143");
            s.imap_port_edited();
        }
    });
    step(&mut rt);
    click(&mut rt, "add-imap-tls-starttls");
    match &rt.state.modal {
        Some(Modal::AddAccount(f)) => {
            assert_eq!(f.imap_tls, ImapTls::Starttls);
            assert_eq!(f.imap_port.text, "1143");
        }
        _ => panic!("expected add-account modal"),
    }

    // Typing well-known port 993/143 mirrors back to the security mode.
    rt.update(|s, _| {
        if let Some(Modal::AddAccount(f)) = &mut s.modal {
            f.imap_port.replace_text("993");
            s.imap_port_edited();
        }
    });
    step(&mut rt);
    match &rt.state.modal {
        Some(Modal::AddAccount(f)) => assert_eq!(f.imap_tls, ImapTls::Implicit),
        _ => panic!("expected add-account modal"),
    }
    rt.update(|s, _| {
        if let Some(Modal::AddAccount(f)) = &mut s.modal {
            f.imap_port.replace_text("143");
            s.imap_port_edited();
        }
    });
    step(&mut rt);
    match &rt.state.modal {
        Some(Modal::AddAccount(f)) => assert_eq!(f.imap_tls, ImapTls::Starttls),
        _ => panic!("expected add-account modal"),
    }
}

#[test]
fn backfill_button_tracks_incomplete_folders_unified_inbox_and_jump_to_bottom() {
    let mut s = demo::state();
    (demo::SCENES
        .iter()
        .find(|(n, _)| *n == "backfill")
        .unwrap()
        .1)(&mut s);
    let mut rt = runtime(s);

    assert!(rt.state.has_server_history());
    assert_eq!(rt.state.remaining_server_count(), 1_297);
    assert!(rt.find("load-more").is_some());
    assert!(rt.find("load-more-hint").is_some());
    let has_text = |rt: &Runtime<Ui>, needle: &str| {
        rt.elements()
            .iter()
            .filter_map(|e| e.text.as_deref())
            .any(|t| t.contains(needle))
    };
    assert!(has_text(&rt, "Load older messages"));
    assert!(has_text(&rt, "1,297 older on server"));
    let expected_count = format!("{}+ shown", rt.state.visible_messages().len());
    assert!(has_text(&rt, &expected_count));

    // `G` only jumps to the bottom of already-cached mail; it never triggers
    // an automatic server backfill.
    typed(&mut rt, "G");
    assert!(!rt.state.loading_older);

    // Unified inbox aggregates incomplete inbox folders across accounts.
    let mut u = demo::state();
    u.has_more = false;
    if let Some(f) = u
        .folders
        .get_mut("a1")
        .and_then(|l| l.iter_mut().find(|f| f.role == "inbox"))
    {
        f.total_count = 600;
        f.cached_count = 200;
        f.history_complete = false;
    }
    if let Some(f) = u
        .folders
        .get_mut("a2")
        .and_then(|l| l.iter_mut().find(|f| f.role == "inbox"))
    {
        f.total_count = 450;
        f.cached_count = 200;
        f.history_complete = false;
    }
    let mut rt_u = runtime(u);
    assert!(rt_u.state.has_server_history());
    assert_eq!(rt_u.state.remaining_server_count(), 650);
    assert!(has_text(&rt_u, "650 older on server"));

    // Local FTS search never offers server backfill.
    typed(&mut rt_u, "/");
    typed(&mut rt_u, "invoice");
    key(&mut rt_u, Key::Enter);
    assert!(!rt_u.state.has_server_history());
    assert_eq!(rt_u.state.remaining_server_count(), 0);
}
