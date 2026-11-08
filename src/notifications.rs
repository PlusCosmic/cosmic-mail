//! Desktop notifications via `notify-rust` (rendered by mako on omarchy).
//!
//! Notifications carry the app-name "Cosmic Mail" (so users can theme/script
//! per app-name in mako) and a `dev.pluscosmic.mail` desktop-entry hint. Each
//! notification exposes a single `default` action; activating it emits
//! [`AppEvent::Activate`], which shows and focuses the main window. Action handling is spawned so waiting on the user does not
//! block the sync engine.

use crate::events::{AppEvent, Events};
use notify_rust::{Hint, Notification, NotificationResponse};

const APP_NAME: &str = "Cosmic Mail";
const DESKTOP_ENTRY: &str = "dev.pluscosmic.mail";

/// A single new-message notification's content.
pub struct NewMail {
    pub from_name: String,
    pub subject: String,
}

/// Send notifications for a batch of new messages that arrived in an inbox
/// during one sync pass. Per the contract: 1..=3 messages produce one
/// notification each; more than 3 collapse into a single summary notification.
pub fn notify_new_mail(events: &Events, account_email: &str, new: &[NewMail]) {
    if new.is_empty() {
        return;
    }

    if new.len() > 3 {
        let summary = format!("{} new messages", new.len());
        let body = format!("in {account_email}");
        show(events, &summary, &body);
    } else {
        for m in new {
            let summary = format!("New mail — {}", m.from_name);
            show(events, &summary, &m.subject);
        }
    }
}

/// Send a sample notification (used by the `test_notification` command).
pub fn test(events: &Events) {
    show(
        events,
        "Cosmic Mail",
        "Notifications are working. Click to focus the window.",
    );
}

/// Build, show, and wire up the `default` action for a notification.
fn show(events: &Events, summary: &str, body: &str) {
    let mut notification = Notification::new();
    notification
        .appname(APP_NAME)
        .summary(summary)
        .body(body)
        .icon(DESKTOP_ENTRY)
        .hint(Hint::DesktopEntry(DESKTOP_ENTRY.to_string()))
        .action("default", "Open");

    match notification.show() {
        Ok(handle) => {
            let events = events.clone();
            // wait_for_response blocks until the user acts or the notification
            // closes; run it off the sync/async runtime on a dedicated thread.
            std::thread::spawn(move || {
                if let Err(err) =
                    handle.wait_for_response(|response: &NotificationResponse| match response {
                        NotificationResponse::Default | NotificationResponse::Action(_) => {
                            tracing::debug!(?response, "notification action received");
                            events.emit(AppEvent::Activate);
                        }
                        NotificationResponse::Closed(reason) => {
                            tracing::debug!(?reason, "notification closed");
                        }
                        NotificationResponse::Reply(_) => {}
                    })
                {
                    tracing::warn!(error = %err, "notification action listener failed");
                }
            });
        }
        Err(err) => {
            tracing::warn!(error = %err, "failed to show notification");
        }
    }
}
