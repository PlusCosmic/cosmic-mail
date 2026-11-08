//! End to end: the real Rune view over the real backend, syncing a hermetic
//! GreenMail server. Nothing is mocked between the keystrokes and IMAP.
//!
//! Needs the fixture up and seeded (`e2e/README.md`):
//!
//! ```sh
//! docker compose -f e2e/docker-compose.yml up -d && node e2e/seed.mjs
//! cargo test --test e2e_greenmail -- --ignored
//! ```
//!
//! The run uses a throwaway profile (`XDG_CONFIG_HOME`/`XDG_DATA_HOME`), so it
//! never touches real accounts or mail. Debug builds only: the IMAP password
//! and the fixture's CA come from `COSMIC_MAIL_TEST_IMAP_PASSWORD` and
//! `COSMIC_MAIL_EXTRA_CA` instead of the keyring and system trust store.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use cosmic_mail::backend::Backend;
use cosmic_mail::events::Events;
use cosmic_mail::store;
use cosmic_mail::ui::view::{on_key, view};
use cosmic_mail::ui::{Inbox, Modal, Ui};
use rune::{Env, Key, KeyEvent, Mods, Runtime};

const TIMEOUT: Duration = Duration::from_secs(90);

struct Harness {
    rt: Runtime<Ui>,
    t: f64,
    _profile: tempfile::TempDir,
    _tokio: tokio::runtime::Runtime,
}

impl Harness {
    fn start() -> Harness {
        let repo = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let profile = tempfile::tempdir().expect("temp profile");
        let config = profile.path().join("config");
        let data = profile.path().join("data");
        std::fs::create_dir_all(config.join("cosmic-mail")).expect("config dir");
        std::fs::create_dir_all(&data).expect("data dir");
        std::fs::copy(
            repo.join("e2e/profile/accounts.json"),
            config.join("cosmic-mail/accounts.json"),
        )
        .expect("copy fixture profile");
        // Single test per process: nothing else reads the environment yet.
        std::env::set_var("XDG_CONFIG_HOME", &config);
        std::env::set_var("XDG_DATA_HOME", &data);
        std::env::set_var("COSMIC_MAIL_EXTRA_CA", repo.join("e2e/fixtures/tls/ca.pem"));
        std::env::set_var("COSMIC_MAIL_TEST_IMAP_PASSWORD", "test-pass");
        let _ = rustls::crypto::ring::default_provider().install_default();

        let tokio = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .expect("tokio");
        let inbox = Inbox::default();
        let events = {
            let inbox = inbox.clone();
            Events::new(move |ev| inbox.event(ev))
        };
        let backend = Backend::new(
            store::open().expect("open db"),
            events,
            tokio.handle().clone(),
        );
        backend.start_all_syncs();

        let ui = Ui::new(Some(backend), inbox);
        let env = Env {
            system_dark: true,
            scale: 1.0,
        };
        let mut rt = Runtime::new(ui, view, (1280.0, 800.0), env);
        rt.on_key = Some(on_key);
        rt.frame(0.0);
        rt.update(|s, _| s.init());
        Harness {
            rt,
            t: 0.0,
            _profile: profile,
            _tokio: tokio,
        }
    }

    /// One frame: land backend results, then draw.
    fn tick(&mut self) {
        self.t += 1.0 / 60.0;
        let now = self.t;
        self.rt.update(|s, tx| {
            s.poll(now, tx);
        });
        self.rt.frame(1.0 / 60.0);
    }

    /// Run frames until `done` holds, or fail with `what`.
    fn wait(&mut self, what: &str, done: impl Fn(&Ui) -> bool) {
        let start = Instant::now();
        while !done(&self.rt.state) {
            assert!(
                start.elapsed() < TIMEOUT,
                "timed out waiting for {what}; toasts: {:?}",
                self.rt
                    .state
                    .toasts
                    .iter()
                    .map(|t| &t.message)
                    .collect::<Vec<_>>()
            );
            self.tick();
            std::thread::sleep(Duration::from_millis(20));
        }
        for _ in 0..3 {
            self.tick();
        }
    }

    fn click(&mut self, key: &str) {
        let (x, y) = self
            .rt
            .find(key)
            .unwrap_or_else(|| panic!("no element keyed {key:?}"));
        self.rt.pointer_down(x, y);
        self.rt.pointer_up(x, y);
        self.tick();
    }

    fn typed(&mut self, s: &str) {
        for c in s.chars() {
            self.key(Key::Text(c.to_string()));
        }
    }

    fn key(&mut self, key: Key) {
        self.rt.key(KeyEvent {
            key,
            mods: Mods::default(),
        });
        self.tick();
    }

    /// The text shown by the element keyed `key` (or its descendants).
    fn text_of(&self, key: &str) -> String {
        let elements = self.rt.elements();
        let Some(root) = elements.iter().position(|e| e.key.as_deref() == Some(key)) else {
            return String::new();
        };
        let prefix = elements[root].path.clone();
        elements
            .iter()
            .filter(|e| e.path == prefix || e.path.starts_with(&format!("{prefix}/")))
            .filter_map(|e| e.text.clone())
            .collect::<Vec<_>>()
            .join(" ")
    }

    fn message_id(&self, subject: &str) -> i64 {
        self.rt
            .state
            .messages
            .iter()
            .find(|m| m.subject == subject)
            .map(|m| m.id)
            .unwrap_or_else(|| panic!("no message {subject:?}"))
    }
}

#[test]
#[ignore = "needs the GreenMail fixture: see e2e/README.md"]
fn the_ui_syncs_reads_and_acts_on_a_real_mailbox() {
    let mut h = Harness::start();

    // The fixture inbox syncs into the unified list.
    h.wait("the fixture inbox to sync", |s| {
        [
            "Welcome to Cosmic Mail",
            "Weekly project update",
            "Your order has shipped",
        ]
        .iter()
        .all(|subject| s.messages.iter().any(|m| &m.subject == subject))
    });
    assert_eq!(h.rt.state.accounts.len(), 1);
    assert!(h.text_of("header-title").contains("Unified Inbox"));

    // Clicking a message opens it in the reader with its body.
    let welcome = h.message_id("Welcome to Cosmic Mail");
    h.click(&format!("msg-{welcome}"));
    h.wait("the welcome body", |s| {
        s.body.as_ref().is_some_and(|b| b.id == welcome)
    });
    assert_eq!(h.text_of("reader-subject"), "Welcome to Cosmic Mail");
    assert!(h
        .text_of("reader")
        .contains("Thanks for trying Cosmic Mail"));
    // Opening marks it read, on the server too (the next refetch agrees).
    h.wait("the read flag", |s| {
        s.messages.iter().any(|m| m.id == welcome && m.seen)
    });

    // A shipment email surfaces its tracking number.
    let shipped = h.message_id("Your order has shipped");
    h.click(&format!("msg-{shipped}"));
    h.wait("the shipment chip", |s| !s.shipments.is_empty());
    let ship = h.rt.state.shipments[0].clone();
    assert_eq!(ship.carrier, "ups");
    assert_eq!(ship.tracking_number.as_deref(), Some("1Z999AA10123456784"));
    let chip = h.text_of(&format!("ship-{}", ship.id));
    assert!(
        chip.contains("UPS") && chip.contains("1Z999AA10123456784"),
        "{chip}"
    );

    // `f` toggles its flag; the change survives a refetch from the database.
    // (Toggle, not set: a re-run against the same fixture starts flagged.)
    let was_flagged =
        h.rt.state
            .messages
            .iter()
            .any(|m| m.id == shipped && m.flagged);
    h.typed("f");
    let flagged =
        move |s: &Ui| s.messages.iter().any(|m| m.id == shipped && m.flagged) != was_flagged;
    assert!(flagged(&h.rt.state), "optimistic");
    h.rt.update(|s, _| s.refresh_current_page());
    h.wait("the refetched flag", |s| !s.loading_messages);
    for _ in 0..30 {
        h.tick();
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(flagged(&h.rt.state), "persisted");
    assert!(
        h.rt.state.toasts.iter().all(|t| !t.error),
        "{:?}",
        h.rt.state.toasts
    );

    // Delete fails here (the fixture account has no Trash folder): the row
    // comes back and the error is reported.
    let update = h.message_id("Weekly project update");
    h.click(&format!("msg-{update}"));
    h.typed("d");
    assert!(
        h.rt.state.messages.iter().all(|m| m.id != update),
        "optimistic"
    );
    h.wait("the delete rollback", |s| {
        s.messages.iter().any(|m| m.id == update)
    });
    assert!(h
        .rt
        .state
        .toasts
        .iter()
        .any(|t| t.error && t.message.contains("no trash folder")));

    // `/` + Enter searches the whole local cache (FTS5).
    h.key(Key::Escape);
    h.typed("/");
    h.typed("1Z999AA10123456784");
    h.key(Key::Enter);
    h.wait("search results", |s| {
        s.search_active && !s.loading_messages && !s.messages.is_empty()
    });
    assert!(
        h.rt.state
            .messages
            .iter()
            .all(|m| m.subject == "Your order has shipped"),
        "{:?} for {:?}",
        h.rt.state
            .messages
            .iter()
            .map(|m| &m.subject)
            .collect::<Vec<_>>(),
        h.rt.state.search_query
    );

    // The palette reaches the account's folders.
    h.click("clear-search");
    h.typed("2");
    h.wait("the account's folders", |s| s.selected_folder.is_some());
    assert!(h.rt.find("folders").is_some());
    h.rt.key(KeyEvent {
        key: Key::Command('k'),
        mods: Mods {
            cmd: true,
            ctrl: true,
            ..Mods::default()
        },
    });
    h.tick();
    assert!(matches!(h.rt.state.modal, Some(Modal::Palette(_))));
    h.typed("sync current");
    h.key(Key::Enter);
    assert!(h.rt.state.modal.is_none());
}
