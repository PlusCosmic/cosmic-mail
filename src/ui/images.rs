//! Images inside HTML mail, and opening links.
//!
//! `data:` images (including the `cid:` parts the backend inlines as data URIs)
//! decode locally and always show. Remote (`http(s)`) images are tracking
//! beacons as often as content, so they are fetched only with consent — per
//! message, or globally via "Always download remote images". Nothing else in a
//! message ever touches the network.

use std::sync::Arc;
use std::time::Duration;

use rune::image::{Image, ImageCache};

use super::html::{decode_data_uri, is_remote};
use super::Ui;

/// Remote images fetched per message, at most.
const MAX_REMOTE_IMAGES: usize = 40;
/// Largest remote image accepted.
const MAX_REMOTE_IMAGE_BYTES: usize = 8 * 1024 * 1024;

/// One image referenced by the open message.
#[derive(Clone)]
pub enum BodyImage {
    /// Being fetched.
    Loading,
    /// Handed to the decoder (its size is known once decoded).
    Ready(Image),
    /// Fetch or decode failed: the alt text shows instead.
    Failed,
}

/// A short stable hash for element keys.
pub fn short_hash(s: &str) -> String {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    s.hash(&mut h);
    format!("{:016x}", h.finish())
}

impl Ui {
    /// The decoder images go through (set once the window has a waker).
    pub fn set_image_cache(&mut self, cache: Arc<ImageCache>) {
        self.image_cache = Some(cache);
    }

    /// Whether the open message references any remote image.
    pub fn body_has_remote_images(&self) -> bool {
        self.body_image_srcs().iter().any(|src| is_remote(src))
    }

    /// Every image `src` the open message shows: the visible `<img>`s of its Blitz
    /// layout once that has landed, plus those the styled-text lines found (the
    /// fallback rendering, and all there is before the first layout). html2text
    /// misses some images Blitz lays out — GitHub's avatar inside a flex `<div>` —
    /// so the lines alone are not enough.
    pub(crate) fn body_image_srcs(&self) -> Vec<String> {
        let mut srcs: Vec<String> = self
            .body_lines
            .iter()
            .flat_map(|l| l.images.iter().map(|i| i.src.clone()))
            .collect();
        srcs.extend(self.mail_layout_image_srcs());
        let mut seen = std::collections::HashSet::new();
        srcs.retain(|s| seen.insert(s.clone()));
        srcs
    }

    /// Decode the open message's inline images, and fetch its remote ones
    /// when allowed. Called whenever the body or the consent changes.
    pub fn prepare_body_images(&mut self) {
        let Some(id) = self.body.as_ref().map(|b| b.id) else {
            self.body_images.clear();
            return;
        };
        let Some(cache) = self.image_cache.clone() else {
            return;
        };
        let srcs = self.body_image_srcs();
        let allow_remote = self.remote_images_allowed(id);
        let mut remote = Vec::new();
        for src in srcs {
            if self.body_images.contains_key(&src) {
                continue;
            }
            if is_remote(&src) {
                if allow_remote && remote.len() < MAX_REMOTE_IMAGES {
                    remote.push(src);
                }
            } else if let Some((_, bytes)) = decode_data_uri(&src) {
                let image = cache.load(bytes);
                self.body_images.insert(src, BodyImage::Ready(image));
            }
        }
        for src in remote {
            self.body_images.insert(src.clone(), BodyImage::Loading);
            let cache = cache.clone();
            self.spawn(
                move |_b| async move {
                    let result = fetch_image(&src).await;
                    (src, result)
                },
                move |ui, (src, result), _| {
                    if ui.body.as_ref().map(|b| b.id) != Some(id) {
                        return; // another message opened meanwhile
                    }
                    let entry = match result {
                        Ok(bytes) => BodyImage::Ready(cache.load(bytes)),
                        Err(err) => {
                            tracing::debug!(error = %err, "remote image failed");
                            BodyImage::Failed
                        }
                    };
                    ui.body_images.insert(src, entry);
                },
            );
        }
    }

    /// Allow or block remote images for one message (this session only).
    pub fn toggle_remote_images(&mut self, id: i64) {
        if !self.remote_allowed.remove(&id) {
            self.remote_allowed.insert(id);
            self.prepare_body_images();
        } else {
            self.body_images.retain(|src, _| !is_remote(src));
        }
    }

    /// How many images have finished decoding (`poll` redraws when it moves).
    pub(crate) fn decoded_images(&self) -> usize {
        self.body_images
            .values()
            .filter(|i| matches!(i, BodyImage::Ready(img) if img.size().is_some()))
            .count()
    }

    /// Open a link from mail content: only absolute `http(s)` URLs, in the
    /// browser. Everything else is ignored.
    pub fn open_link(&mut self, url: &str) {
        if crate::desktop::openable_url(url).is_none() {
            return;
        }
        if self.backend.is_none() {
            return; // scenes and tests never launch a browser
        }
        if let Err(err) = crate::desktop::open_url(url) {
            self.error("Could not open link", err);
        }
    }
}

/// GET a remote image: no cookies, bounded size and time.
async fn fetch_image(src: &str) -> anyhow::Result<Vec<u8>> {
    use anyhow::{bail, Context};
    let url = if src.trim_start().starts_with("//") {
        format!("https:{}", src.trim())
    } else {
        src.trim().to_string()
    };
    if crate::desktop::openable_url(&url).is_none() {
        bail!("not an http(s) image");
    }
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(15))
        .redirect(reqwest::redirect::Policy::limited(5))
        .build()
        .context("building image client")?;
    let mut response = client
        .get(&url)
        .send()
        .await
        .context("fetching image")?
        .error_for_status()
        .context("image request failed")?;
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await.context("reading image")? {
        bytes.extend_from_slice(&chunk);
        if bytes.len() > MAX_REMOTE_IMAGE_BYTES {
            bail!("image too large");
        }
    }
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use rune::image::ImageCache;

    fn encoded(format: image::ImageFormat) -> Vec<u8> {
        let img = image::RgbaImage::from_pixel(3, 2, image::Rgba([200, 60, 60, 255]));
        let mut out = std::io::Cursor::new(Vec::new());
        img.write_to(&mut out, format).expect("encode");
        out.into_inner()
    }

    #[test]
    fn gif_and_webp_mail_images_decode() {
        // Rune's decoder reads them only because this crate enables the `image` features.
        let cache = ImageCache::new(Arc::new(|| {}));
        for format in [image::ImageFormat::Gif, image::ImageFormat::WebP] {
            let img = cache.load(encoded(format));
            assert_eq!(img.wait().ok(), Some([3, 2]), "{format:?}");
        }
    }
}
