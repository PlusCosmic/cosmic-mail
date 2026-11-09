//! The open message's HTML, laid out by Blitz for the reader.
//!
//! The view asks for the layout at the reader's width ([`Ui::mail_layout`]).
//! A layout is keyed by that width (in 8px steps, so a divider drag doesn't
//! relayout every pixel) and by how many body images have decoded (their sizes
//! feed the layout). In the app a missing layout is computed on a worker thread
//! and lands through the [`Inbox`](super::Inbox); until then the reader keeps
//! drawing the last one for the same message. Without an app handle (headless
//! renders, tests) it is computed in place, so a single frame is complete.

use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::Arc;

use super::mail_doc::{self, MailLayout};
use super::{BodyImage, Ui};

/// (width in px, decoded images).
type Key = (u32, usize);

#[derive(Default)]
pub struct MailState {
    /// Bumped whenever the open body changes, so stale results are dropped.
    generation: u64,
    /// The latest layout for the open body, and what it was laid out for.
    done: Option<(Key, Arc<MailLayout>)>,
    /// The layout being computed off-thread.
    pending: Option<Key>,
}

impl Ui {
    /// Forget the open body's layout (the body changed).
    pub fn reset_mail_layout(&mut self) {
        let state = self.mail.get_mut();
        state.generation += 1;
        state.done = None;
        state.pending = None;
    }

    /// The open message's HTML laid out at `width`, or the closest one already
    /// computed while that is under way. `None` for a message without HTML, or
    /// before its first layout lands.
    pub fn mail_layout(&self, width: f32) -> Option<Arc<MailLayout>> {
        let html = self.body.as_ref()?.html.as_ref()?;
        let width = (width.max(320.0) / 8.0).floor() * 8.0;
        let key = (width as u32, self.decoded_seen);
        let generation = {
            let state = self.mail.borrow();
            if let Some((k, doc)) = &state.done {
                if *k == key || state.pending == Some(key) {
                    return Some(doc.clone());
                }
            } else if state.pending == Some(key) {
                return None;
            }
            state.generation
        };

        let sizes = self.decoded_image_sizes();
        if !self.inbox.has_handle() {
            let doc = Arc::new(mail_doc::layout(html, width, &sizes));
            set_done(&self.mail, key, doc.clone());
            return Some(doc);
        }

        self.mail.borrow_mut().pending = Some(key);
        let html = html.clone();
        let inbox = self.inbox.clone();
        let spawned = std::thread::Builder::new()
            .name("mail-layout".into())
            .spawn(move || {
                let doc = Arc::new(mail_doc::layout(&html, width, &sizes));
                inbox.push(Box::new(move |ui: &mut Ui, _tx| {
                    let state = ui.mail.get_mut();
                    if state.generation == generation {
                        if state.pending == Some(key) {
                            state.pending = None;
                        }
                        state.done = Some((key, doc));
                    }
                }));
            });
        if let Err(err) = spawned {
            tracing::warn!(error = %err, "could not start mail layout");
            self.mail.borrow_mut().pending = None;
        }
        self.mail.borrow().done.as_ref().map(|(_, doc)| doc.clone())
    }

    /// The pixel size of every body image decoded so far, by `src`.
    fn decoded_image_sizes(&self) -> HashMap<String, (u32, u32)> {
        self.body_images
            .iter()
            .filter_map(|(src, img)| match img {
                BodyImage::Ready(img) => img.size().map(|[w, h]| (src.clone(), (w, h))),
                _ => None,
            })
            .collect()
    }
}

fn set_done(state: &RefCell<MailState>, key: Key, doc: Arc<MailLayout>) {
    let mut state = state.borrow_mut();
    state.pending = None;
    state.done = Some((key, doc));
}
