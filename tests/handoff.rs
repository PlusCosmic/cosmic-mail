//! Postmaster's hand-off as the backend sees it: what's refused before anything is stored, and
//! removal by address. One test, since it points `XDG_CONFIG_HOME`/`XDG_DATA_HOME` at a
//! throwaway profile and the environment is per process. Adding for real needs the keyring and
//! an IMAP server; that path is the existing `add_imap_account`.

use std::sync::{Arc, Mutex};

use cosmic_mail::backend::Backend;
use cosmic_mail::events::{AppEvent, Events};
use cosmic_mail::store;
use cosmic_mail::wire::ImapAccountInput;

fn input(email: &str) -> ImapAccountInput {
    ImapAccountInput {
        email: email.into(),
        display_name: "Lucy".into(),
        imap_host: "127.0.0.1".into(),
        imap_port: 9,
        smtp_host: "127.0.0.1".into(),
        smtp_port: 9,
        username: email.into(),
        password: "unused".into(),
    }
}

#[test]
fn handed_over_accounts_are_checked_and_removed_by_address() {
    let profile = tempfile::tempdir().expect("temp profile");
    let config = profile.path().join("config");
    let data = profile.path().join("data");
    std::fs::create_dir_all(config.join("cosmic-mail")).expect("config dir");
    std::fs::create_dir_all(&data).expect("data dir");
    std::fs::write(
        config.join("cosmic-mail/accounts.json"),
        r#"[{"id":"acct-1","email":"Post@Leach.Family","displayName":"Post","kind":"imap",
            "imapHost":"imap.purelymail.com","imapPort":993,"smtpHost":"smtp.purelymail.com",
            "smtpPort":465,"username":"post@leach.family"}]"#,
    )
    .expect("accounts.json");
    std::env::set_var("XDG_CONFIG_HOME", &config);
    std::env::set_var("XDG_DATA_HOME", &data);

    let tokio = tokio::runtime::Runtime::new().expect("tokio");
    let events = Arc::new(Mutex::new(Vec::new()));
    let sink = {
        let events = events.clone();
        Events::new(move |e| events.lock().expect("events").push(e))
    };
    let backend = Backend::new(store::open().expect("db"), sink, tokio.handle().clone());

    // Already there, whatever the case: refused before the keyring or the network.
    let err = tokio
        .block_on(backend.add_handed_over_account(input("post@leach.family")))
        .expect_err("duplicate");
    assert!(err.to_string().contains("already in Cosmic Mail"), "{err}");

    let err = backend
        .remove_account_by_email("sam@leach.family")
        .expect_err("unknown");
    assert!(err.to_string().contains("isn't in Cosmic Mail"), "{err}");
    assert!(events.lock().expect("events").is_empty());

    backend
        .remove_account_by_email("POST@leach.family")
        .expect("removed");
    assert!(backend.list_accounts().expect("accounts").is_empty());
    assert_eq!(
        *events.lock().expect("events"),
        vec![AppEvent::AccountsChanged]
    );
}
