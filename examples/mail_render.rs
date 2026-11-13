//! Dev tool: draw one HTML mail through the real reader, headless, for the render-corpus
//! comparison against Chrome (`tools/render-corpus/README.md`).
//!
//! usage: mail_render MAIL.html MANIFEST.json OUT_PREFIX [WINDOW_WIDTH]
//!
//! MANIFEST.json maps each `<img src>` (as written in the mail) to a local file holding its
//! bytes: `{"images": {"https://…": {"file": "/abs/path", "w": 600, "h": 200}}}` (`w`/`h`
//! optional, only used to guess the window height). A missing manifest means no remote
//! images. Nothing is fetched.
//!
//! Writes OUT_PREFIX.app.png (the reader's mail column, at scale 1) and OUT_PREFIX.app.json
//! (the Blitz display list the reader drew, plus each image's decode result).

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, OnceLock};

use cosmic_mail::app;
use cosmic_mail::ui::mail_doc::{self, Item, MailLayout};
use cosmic_mail::ui::{demo, html, BodyImage, Ui};
use rune::image::ImageCache;
use rune::{App, AppSpec, Waker};
use serde_json::{json, Value};

/// An `<img src>`, its bytes, and the size the manifest declares for it.
type Source = (String, Vec<u8>, Option<(u32, u32)>);

struct Job {
    html: String,
    images: Vec<Source>,
    out: PathBuf,
    width: f32,
}

static JOB: OnceLock<Job> = OnceLock::new();

fn job() -> &'static Job {
    JOB.get().expect("job")
}

/// The width the reader lays mail out at for a window `window` points wide (default split).
fn mail_width(window: f32) -> f32 {
    let available = window - 60.0 - 9.0; // rail, divider
    let reader = available - (available * 0.42).clamp(320.0, 760.0);
    let w = (reader - 80.0).min(960.0);
    (w.max(320.0) / 8.0).floor() * 8.0
}

fn state() -> Ui {
    let j = job();
    let mut s = demo::state();
    s.shipments.clear();
    s.settings.always_download_remote_images = true; // no consent notice above the mail
    if let Some(body) = s.body.as_mut() {
        body.html = Some(j.html.clone());
        body.text = None;
        body.attachments.clear();
    }
    s.body_lines = html::render(&j.html);
    s.reset_mail_layout();
    s
}

fn setup(s: &mut Ui, waker: Waker) {
    let j = job();
    if let Some(base) = app::spec().setup {
        base(s, waker.clone());
    }
    let cache = Arc::new(ImageCache::new(waker));
    s.set_image_cache(cache.clone());

    let mut status: Vec<Value> = Vec::new();
    for (src, bytes, _) in &j.images {
        let image = cache.load(bytes.clone());
        let result = image.wait().map_err(|e| format!("{e:?}"));
        status.push(json!({
            "src": short(src),
            "bytes": bytes.len(),
            "format": sniff(bytes),
            "size": result.as_ref().ok(),
            "error": result.as_ref().err(),
        }));
        s.body_images.insert(src.clone(), BodyImage::Ready(image));
    }
    // `data:` images (cid parts): the ones the styled-text pass finds, and the ones only the
    // layout finds.
    s.prepare_body_images();
    let first = mail_doc::layout(&j.html, j.width, &HashMap::new());
    for item in &first.items {
        if let Item::Image { src, .. } = item {
            if s.body_images.contains_key(src) {
                continue;
            }
            if let Some((mime, bytes)) = html::decode_data_uri(src) {
                let image = cache.load(bytes.clone());
                let result = image.wait().map_err(|e| format!("{e:?}"));
                status.push(json!({
                    "src": short(src), "bytes": bytes.len(), "format": sniff(&bytes), "mime": mime,
                    "size": result.as_ref().ok(), "error": result.as_ref().err(),
                }));
                s.body_images.insert(src.clone(), BodyImage::Ready(image));
            }
        }
    }
    for img in s.body_images.values() {
        if let BodyImage::Ready(img) = img {
            let _ = img.wait();
        }
    }

    // What the reader will draw: the layout at the decoded sizes.
    let sizes: HashMap<String, (u32, u32)> = s
        .body_images
        .iter()
        .filter_map(|(src, img)| match img {
            BodyImage::Ready(img) => img.size().map(|[w, h]| (src.clone(), (w, h))),
            _ => None,
        })
        .collect();
    let layout = mail_doc::layout(&j.html, j.width, &sizes);
    let dump = dump(&layout, status);
    let path = j.out.with_extension("app.json");
    if let Err(e) = std::fs::write(&path, serde_json::to_vec(&dump).unwrap_or_default()) {
        eprintln!("writing {}: {e}", path.display());
    }
}

fn short(src: &str) -> String {
    if src.len() > 300 {
        format!(
            "{}…",
            &src[..src.char_indices().nth(120).map_or(src.len(), |(i, _)| i)]
        )
    } else {
        src.to_string()
    }
}

fn sniff(b: &[u8]) -> &'static str {
    if b.starts_with(b"\x89PNG") {
        "png"
    } else if b.starts_with(b"\xff\xd8") {
        "jpeg"
    } else if b.starts_with(b"GIF8") {
        "gif"
    } else if b.len() > 12 && &b[..4] == b"RIFF" && &b[8..12] == b"WEBP" {
        "webp"
    } else if b.len() > 12 && &b[4..8] == b"ftyp" {
        "avif/heif"
    } else if b.starts_with(b"BM") {
        "bmp"
    } else if b
        .windows(4)
        .take(512)
        .any(|w| w.eq_ignore_ascii_case(b"<svg"))
    {
        "svg"
    } else if b.starts_with(b"\x00\x00\x01\x00") {
        "ico"
    } else {
        "other"
    }
}

fn dump(l: &MailLayout, images: Vec<Value>) -> Value {
    let items: Vec<Value> = l
        .items
        .iter()
        .map(|i| match i {
            Item::Box {
                x,
                y,
                w,
                h,
                fill,
                border,
                radius,
            } => json!({
                "t": "box", "x": x, "y": y, "w": w, "h": h, "fill": fill,
                "border": border.map(|(w, c)| json!({"w": w, "c": c})), "radius": radius,
            }),
            Item::Image {
                x,
                y,
                w,
                h,
                src,
                alt,
                link,
                radius,
            } => json!({
                "t": "img", "x": x, "y": y, "w": w, "h": h, "src": short(src), "alt": alt,
                "link": link.is_some(), "radius": radius,
            }),
            Item::Text {
                x,
                y,
                w,
                line_height,
                size,
                text,
                spans,
            } => json!({
                "t": "text", "x": x, "y": y, "w": w, "lh": line_height, "size": size, "text": text,
                "spans": spans.iter().map(|s| json!({
                    "r": [s.range.start, s.range.end], "weight": s.weight, "italic": s.italic,
                    "color": s.color, "underline": s.underline, "mono": s.mono,
                    "link": s.link.is_some(),
                })).collect::<Vec<_>>(),
            }),
        })
        .collect();
    json!({
        "width": l.width, "content_width": l.content_width, "height": l.height,
        "background": l.background, "items": items, "images": images,
        "fit_scale": (l.width / l.content_width).min(1.0),
    })
}

fn main() {
    let mut args = std::env::args().skip(1);
    let usage = "usage: mail_render MAIL.html MANIFEST.json OUT_PREFIX [WINDOW_WIDTH]";
    let html_path = args.next().expect(usage);
    let manifest_path = args.next().expect(usage);
    let out = PathBuf::from(args.next().expect(usage));
    let window: f32 = args.next().and_then(|w| w.parse().ok()).unwrap_or(1440.0);

    let html =
        String::from_utf8_lossy(&std::fs::read(&html_path).expect("reading mail")).into_owned();
    let manifest: Value = std::fs::read(&manifest_path)
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_else(|| json!({}));
    let mut images = Vec::new();
    if let Some(map) = manifest.get("images").and_then(Value::as_object) {
        for (src, v) in map {
            let Some(bytes) = v
                .get("file")
                .and_then(Value::as_str)
                .and_then(|f| std::fs::read(f).ok())
            else {
                continue;
            };
            let dim = |k: &str| v.get(k).and_then(Value::as_u64).map(|n| n as u32);
            images.push((src.clone(), bytes, dim("w").zip(dim("h"))));
        }
    }
    let width = mail_width(window);

    // The window must hold the whole mail (the snapshot is cropped to the mail column).
    let declared: HashMap<String, (u32, u32)> = images
        .iter()
        .filter_map(|(src, _, size)| size.map(|s| (src.clone(), s)))
        .collect();
    let tall = mail_doc::layout(&html, width, &declared)
        .height
        .max(mail_doc::layout(&html, width, &HashMap::new()).height);
    let height = (tall + 700.0).clamp(900.0, 16000.0);

    let _ = JOB.set(Job {
        html,
        images,
        out: out.clone(),
        width,
    });

    let spec = AppSpec {
        init: state,
        setup: Some(setup),
        size: (window, height),
        ..app::spec()
    };
    let snap = out.with_extension("app.png");
    App::new(spec)
        .args(
            [
                "--snap",
                &snap.to_string_lossy(),
                "--crop",
                "reader-text",
                "--scale",
                "1",
                "--size",
                &format!("{window}x{height}"),
            ]
            .map(String::from)
            .to_vec(),
        )
        .run();
}
