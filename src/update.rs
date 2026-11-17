//! Updates from GitHub Releases (`docs/ARCHITECTURE.md`, "Updates").
//!
//! A release build asks GitHub for the newest release that carries this platform's file,
//! downloads it while checking its SHA-256 against the digest GitHub lists, prepares it
//! (unpacks the portable zip, copies the app out of the disk image), and installs it after
//! the app's loop has returned: the Windows installer runs silently, or the portable exe or
//! the macOS bundle is swapped in place. Linux only learns that a newer version exists: its
//! package manager owns the installed files.
//!
//! Everything here is UI-agnostic; `Backend` holds the state and `app.rs` applies a
//! prepared update at exit.

use std::fmt;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;

use anyhow::{bail, ensure, Context, Result};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// Where releases are published.
pub const REPO: &str = "PlusCosmic/cosmic-mail";

/// Set on a process relaunched after an update: it waits for the old one to exit first.
const AFTER_UPDATE_ENV: &str = "COSMIC_MAIL_AFTER_UPDATE";

/// How long a relaunched process waits for the old one to exit.
const WAIT_FOR_PREVIOUS: Duration = Duration::from_secs(30);

/// The prepared macOS bundle, beside the installed one (same volume: the swap is a rename).
const MAC_STAGED: &str = ".cosmic-mail-update.app";
/// The installed macOS bundle, moved aside during the swap.
const MAC_OLD: &str = ".cosmic-mail-old.app";

/// Whether this platform can install updates itself (Settings offers Automatic).
pub const CAN_INSTALL: bool = cfg!(any(target_os = "windows", target_os = "macos"));

/// A release version, `MAJOR.MINOR.PATCH` (`scripts/version.sh`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Version {
    pub major: u64,
    pub minor: u64,
    pub patch: u64,
}

impl Version {
    /// `1.2.3` or a tag `v1.2.3`; anything else (prerelease suffixes too) is `None`.
    pub fn parse(s: &str) -> Option<Version> {
        let s = s.trim();
        let s = s.strip_prefix('v').unwrap_or(s);
        let mut parts = s.split('.');
        let mut next = || -> Option<u64> {
            let part = parts.next()?;
            if part.is_empty() || !part.bytes().all(|b| b.is_ascii_digit()) {
                return None;
            }
            part.parse().ok()
        };
        let version = Version {
            major: next()?,
            minor: next()?,
            patch: next()?,
        };
        parts.next().is_none().then_some(version)
    }
}

impl fmt::Display for Version {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}.{}", self.major, self.minor, self.patch)
    }
}

/// `Settings.updates`: what the background check does.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum UpdateMode {
    /// No background checks (Check now still works).
    Off,
    /// Tell the user about a newer version.
    #[default]
    Notify,
    /// Download and prepare it in the background; it installs when the app exits.
    Auto,
}

/// How this copy of the app gets a newer version.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum UpdateMethod {
    /// Windows, installed by the Inno Setup installer: run the new one silently.
    Installer,
    /// Windows, the unzipped portable copy: swap the exe in place.
    Portable,
    /// macOS, a `.app` bundle in a folder this user can write: swap the bundle.
    MacApp,
    /// The app can't replace itself here: open the release page.
    Download,
    /// Linux and other systems: the package manager owns the installed files.
    PackageManager,
}

impl UpdateMethod {
    /// Whether the app installs updates itself with this method.
    pub fn installs(self) -> bool {
        matches!(
            self,
            UpdateMethod::Installer | UpdateMethod::Portable | UpdateMethod::MacApp
        )
    }
}

/// A newer version, as the UI sees it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateInfo {
    pub version: String,
    pub current_version: String,
    pub release_url: String,
    pub method: UpdateMethod,
    /// Downloaded, verified and prepared: it installs when the app exits.
    pub ready: bool,
}

/// The release file to download.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Asset {
    pub name: String,
    pub url: String,
    pub size: u64,
    pub sha256: [u8; 32],
}

/// What a check found: a newer version and, for a method that needs one, its file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Found {
    pub version: Version,
    pub current: Version,
    pub release_url: String,
    pub method: UpdateMethod,
    pub asset: Option<Asset>,
}

impl Found {
    pub fn info(&self, ready: bool) -> UpdateInfo {
        UpdateInfo {
            version: self.version.to_string(),
            current_version: self.current.to_string(),
            release_url: self.release_url.clone(),
            method: self.method,
            ready,
        }
    }
}

/// An update ready to install at exit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Prepared {
    /// Run this setup silently.
    Installer { setup: PathBuf },
    /// Move these unpacked files over the running exe's folder.
    Portable { files: PathBuf, exe: PathBuf },
    /// Swap `staged` in for `bundle`.
    MacApp { staged: PathBuf, bundle: PathBuf },
}

/// After installing, start the new version: with its window, or in the background.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Relaunch {
    Foreground,
    Background,
}

/// The version this build checks as, or `None` when it never checks: only release builds
/// (`COSMIC_MAIL_VERSION` set at compile time) do. Debug builds take
/// `COSMIC_MAIL_UPDATE_AS=<version>` to try the flow live.
pub fn current_version() -> Option<Version> {
    #[cfg(debug_assertions)]
    if let Some(v) = std::env::var("COSMIC_MAIL_UPDATE_AS")
        .ok()
        .and_then(|v| Version::parse(&v))
    {
        return Some(v);
    }
    option_env!("COSMIC_MAIL_VERSION").and_then(Version::parse)
}

/// Where downloads and unpacked updates go: `<data dir>/cosmic-mail/updates`.
pub fn updates_dir() -> Result<PathBuf> {
    Ok(crate::paths::data_dir()
        .context("could not determine the data directory")?
        .join("cosmic-mail")
        .join("updates"))
}

// ---- Which method --------------------------------------------------------------------

/// How the running executable gets updates (see [`UpdateMethod`]).
pub fn detect_method() -> UpdateMethod {
    match std::env::current_exe() {
        Ok(exe) => method_for(&exe),
        Err(err) => {
            tracing::warn!(error = %err, "could not locate the running executable");
            if cfg!(any(target_os = "windows", target_os = "macos")) {
                UpdateMethod::Download
            } else {
                UpdateMethod::PackageManager
            }
        }
    }
}

#[cfg(target_os = "windows")]
fn method_for(exe: &Path) -> UpdateMethod {
    let Some(dir) = exe.parent() else {
        return UpdateMethod::Download;
    };
    if !writable(dir) {
        UpdateMethod::Download
    } else if dir.join("unins000.exe").exists() {
        UpdateMethod::Installer
    } else {
        UpdateMethod::Portable
    }
}

#[cfg(target_os = "macos")]
fn method_for(exe: &Path) -> UpdateMethod {
    match bundle_of(exe) {
        // Renaming a directory needs write access to it as well as to its parent.
        Some(bundle)
            if bundle.parent().is_some_and(writable) && writable(&bundle.join("Contents")) =>
        {
            UpdateMethod::MacApp
        }
        _ => UpdateMethod::Download,
    }
}

#[cfg(not(any(target_os = "windows", target_os = "macos")))]
fn method_for(_exe: &Path) -> UpdateMethod {
    UpdateMethod::PackageManager
}

/// Whether this user can create files in `dir`.
#[cfg_attr(not(any(target_os = "windows", target_os = "macos")), allow(dead_code))]
fn writable(dir: &Path) -> bool {
    tempfile::Builder::new()
        .prefix(".cosmic-mail-write-test")
        .tempfile_in(dir)
        .is_ok()
}

/// The `.app` bundle around an executable at `X.app/Contents/MacOS/<exe>`.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
fn bundle_of(exe: &Path) -> Option<PathBuf> {
    let macos = exe.parent()?;
    let contents = macos.parent()?;
    let bundle = contents.parent()?;
    let is_bundle = macos.file_name()? == "MacOS"
        && contents.file_name()? == "Contents"
        && bundle.extension()? == "app";
    is_bundle.then(|| bundle.to_path_buf())
}

// ---- Choosing a release --------------------------------------------------------------

/// A release, as the GitHub API lists it (the fields we read).
#[derive(Debug, Clone, Deserialize)]
pub struct ApiRelease {
    pub tag_name: String,
    pub html_url: String,
    #[serde(default)]
    pub draft: bool,
    #[serde(default)]
    pub prerelease: bool,
    #[serde(default)]
    pub assets: Vec<ApiAsset>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ApiAsset {
    pub name: String,
    pub size: u64,
    pub browser_download_url: String,
    #[serde(default)]
    pub digest: Option<String>,
}

/// The file names (after `cosmic-mail-<version>`) that update this copy, best first. Empty
/// means no file is needed (the package manager installs it).
fn suffixes(method: UpdateMethod, os: &str, arch: &str) -> Vec<&'static str> {
    let mac: &[&'static str] = if arch == "aarch64" {
        &["-macos-arm64.dmg", "-macos-universal.dmg"]
    } else {
        &["-macos-universal.dmg", "-macos-x86_64.dmg"]
    };
    match method {
        UpdateMethod::Installer => vec!["-windows-x64-setup.exe"],
        UpdateMethod::Portable => vec!["-windows-x64-portable.zip"],
        UpdateMethod::MacApp => mac.to_vec(),
        UpdateMethod::Download => match os {
            "windows" => vec!["-windows-x64-setup.exe", "-windows-x64-portable.zip"],
            "macos" => mac.to_vec(),
            _ => Vec::new(),
        },
        UpdateMethod::PackageManager => Vec::new(),
    }
}

/// `sha256:<64 hex>` as bytes.
fn parse_digest(digest: &str) -> Option<[u8; 32]> {
    let hex = digest.strip_prefix("sha256:")?;
    if hex.len() != 64 {
        return None;
    }
    let mut out = [0u8; 32];
    for (i, byte) in out.iter_mut().enumerate() {
        *byte = u8::from_str_radix(hex.get(i * 2..i * 2 + 2)?, 16).ok()?;
    }
    Some(out)
}

/// The newest published release above `current` that has what `method` needs.
pub fn pick(
    releases: &[ApiRelease],
    current: Version,
    method: UpdateMethod,
    os: &str,
    arch: &str,
) -> Option<Found> {
    let wanted = suffixes(method, os, arch);
    let mut candidates: Vec<(Version, &ApiRelease)> = releases
        .iter()
        .filter(|r| !r.draft && !r.prerelease)
        .filter_map(|r| Some((Version::parse(&r.tag_name)?, r)))
        .filter(|(v, _)| *v > current)
        .collect();
    candidates.sort_by_key(|(v, _)| std::cmp::Reverse(*v));
    candidates.into_iter().find_map(|(version, release)| {
        let found = |asset| Found {
            version,
            current,
            release_url: release.html_url.clone(),
            method,
            asset,
        };
        if wanted.is_empty() {
            return Some(found(None));
        }
        let asset = wanted.iter().find_map(|suffix| {
            let name = format!("cosmic-mail-{version}{suffix}");
            let a = release.assets.iter().find(|a| a.name == name)?;
            Some(Asset {
                sha256: parse_digest(a.digest.as_deref()?)?,
                name,
                url: a.browser_download_url.clone(),
                size: a.size,
            })
        })?;
        Some(found(Some(asset)))
    })
}

// ---- Network ---------------------------------------------------------------------------

fn client() -> Result<reqwest::Client> {
    reqwest::Client::builder()
        // GitHub requires a User-Agent; it names the app and nothing about the user.
        .user_agent(format!("cosmic-mail/{}", crate::VERSION))
        .https_only(true)
        .connect_timeout(Duration::from_secs(15))
        .read_timeout(Duration::from_secs(60))
        .redirect(reqwest::redirect::Policy::limited(5))
        .build()
        .context("building the HTTP client")
}

/// Ask GitHub for a newer release than `current` that `method` can use.
pub async fn check(current: Version, method: UpdateMethod) -> Result<Option<Found>> {
    let url = format!("https://api.github.com/repos/{REPO}/releases?per_page=20");
    let releases: Vec<ApiRelease> = client()?
        .get(&url)
        .header("Accept", "application/vnd.github+json")
        .header("X-GitHub-Api-Version", "2022-11-28")
        .timeout(Duration::from_secs(30))
        .send()
        .await
        .context("could not reach GitHub")?
        .error_for_status()
        .context("GitHub refused the release list")?
        .json()
        .await
        .context("could not read GitHub's release list")?;
    Ok(pick(
        &releases,
        current,
        method,
        std::env::consts::OS,
        std::env::consts::ARCH,
    ))
}

/// Download `asset` into `dir`, checking its size and SHA-256 as it streams. A file already
/// there with the right digest is reused.
pub async fn download(asset: &Asset, dir: &Path) -> Result<PathBuf> {
    std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
    // The name is `cosmic-mail-<digits.digits.digits><fixed suffix>`: no separators.
    let path = dir.join(&asset.name);
    if sha256_file(&path).is_ok_and(|d| d == asset.sha256) {
        return Ok(path);
    }
    let part = dir.join(format!("{}.part", asset.name));
    let result = download_to(asset, &part).await;
    if let Err(err) = result {
        let _ = std::fs::remove_file(&part);
        return Err(err);
    }
    std::fs::rename(&part, &path).with_context(|| format!("moving {}", path.display()))?;
    Ok(path)
}

async fn download_to(asset: &Asset, part: &Path) -> Result<()> {
    let mut response = client()?
        .get(&asset.url)
        .send()
        .await
        .context("could not start the download")?
        .error_for_status()
        .context("the download was refused")?;
    let mut file =
        std::fs::File::create(part).with_context(|| format!("creating {}", part.display()))?;
    let mut hasher = Sha256::new();
    let mut total: u64 = 0;
    while let Some(chunk) = response.chunk().await.context("the download broke off")? {
        total += chunk.len() as u64;
        ensure!(
            total <= asset.size,
            "the download is larger than the release says"
        );
        hasher.update(&chunk);
        file.write_all(&chunk).context("writing the download")?;
    }
    file.sync_all().context("writing the download")?;
    ensure!(
        total == asset.size,
        "the download ended early ({total} of {} bytes)",
        asset.size
    );
    let digest: [u8; 32] = hasher.finalize().into();
    ensure!(
        digest == asset.sha256,
        "the download doesn't match the release's SHA-256 digest"
    );
    Ok(())
}

fn sha256_file(path: &Path) -> Result<[u8; 32]> {
    let mut file = std::fs::File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; 64 * 1024];
    loop {
        let n = file.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(hasher.finalize().into())
}

// ---- Prepare -------------------------------------------------------------------------

/// Make a downloaded file ready to install at exit. Blocking: runs off the async workers.
pub fn prepare(method: UpdateMethod, file: &Path, version: Version) -> Result<Prepared> {
    match method {
        UpdateMethod::Installer => Ok(Prepared::Installer {
            setup: file.to_path_buf(),
        }),
        UpdateMethod::Portable => prepare_portable(file, version),
        UpdateMethod::MacApp => prepare_mac(file),
        UpdateMethod::Download | UpdateMethod::PackageManager => {
            bail!("this copy of Cosmic Mail can't install updates itself")
        }
    }
}

fn prepare_portable(zip: &Path, version: Version) -> Result<Prepared> {
    let exe = std::env::current_exe().context("locating the running executable")?;
    let out = zip
        .parent()
        .context("the download has no folder")?
        .join(format!("portable-{version}"));
    if out.exists() {
        std::fs::remove_dir_all(&out).with_context(|| format!("clearing {}", out.display()))?;
    }
    std::fs::create_dir_all(&out).with_context(|| format!("creating {}", out.display()))?;
    // Windows 10 and later ship bsdtar, which reads zip archives.
    let tar = std::env::var_os("SystemRoot")
        .map(|root| PathBuf::from(root).join("System32").join("tar.exe"))
        .unwrap_or_else(|| PathBuf::from("tar.exe"));
    run(Command::new(tar).arg("-xf").arg(zip).arg("-C").arg(&out))
        .context("unpacking the update")?;
    ensure!(
        out.join("cosmic-mail.exe").is_file(),
        "the update has no cosmic-mail.exe"
    );
    Ok(Prepared::Portable { files: out, exe })
}

fn prepare_mac(dmg: &Path) -> Result<Prepared> {
    let exe = std::env::current_exe().context("locating the running executable")?;
    let bundle = bundle_of(&exe).context("Cosmic Mail isn't running from its app bundle")?;
    let parent = bundle.parent().context("the app bundle has no folder")?;
    let staged = parent.join(MAC_STAGED);
    if staged.exists() {
        std::fs::remove_dir_all(&staged)
            .with_context(|| format!("clearing {}", staged.display()))?;
    }

    let mount = dmg
        .parent()
        .context("the download has no folder")?
        .join("mnt");
    std::fs::create_dir_all(&mount).with_context(|| format!("creating {}", mount.display()))?;
    run(Command::new("/usr/bin/hdiutil")
        .args([
            "attach",
            "-nobrowse",
            "-readonly",
            "-noautoopen",
            "-quiet",
            "-mountpoint",
        ])
        .arg(&mount)
        .arg(dmg))
    .context("opening the disk image")?;
    let copied = (|| {
        let app = mount.join("Cosmic Mail.app");
        ensure!(app.is_dir(), "the disk image has no Cosmic Mail.app");
        run(Command::new("/usr/bin/ditto").arg(&app).arg(&staged)).context("copying the new app")
    })();
    if run(Command::new("/usr/bin/hdiutil")
        .args(["detach", "-quiet"])
        .arg(&mount))
    .is_err()
    {
        let _ = run(Command::new("/usr/bin/hdiutil")
            .args(["detach", "-force", "-quiet"])
            .arg(&mount));
    }
    if let Err(err) = copied {
        let _ = std::fs::remove_dir_all(&staged);
        return Err(err);
    }

    let verified = run(Command::new("/usr/bin/codesign")
        .args(["--verify", "--strict"])
        .arg(&staged))
    .context("the new app's signature doesn't verify");
    let complete = staged.join("Contents/MacOS/cosmic-mail").is_file();
    if verified.is_err() || !complete {
        let _ = std::fs::remove_dir_all(&staged);
        verified?;
        bail!("the new app has no executable");
    }
    Ok(Prepared::MacApp { staged, bundle })
}

/// Run a helper to completion (no shell, no console window), failing with its stderr.
fn run(command: &mut Command) -> Result<()> {
    command.stdin(Stdio::null()).stdout(Stdio::null());
    no_window(command);
    let output = command
        .output()
        .with_context(|| format!("starting {:?}", command.get_program()))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        bail!(
            "{:?} failed ({}): {}",
            command.get_program(),
            output.status,
            stderr.trim()
        );
    }
    Ok(())
}

/// Windows: no console window for a helper started by the GUI app.
fn no_window(command: &mut Command) {
    #[cfg(target_os = "windows")]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        command.creation_flags(CREATE_NO_WINDOW);
    }
    #[cfg(not(target_os = "windows"))]
    let _ = command;
}

// ---- Install at exit -----------------------------------------------------------------

/// Install a prepared update. Runs as the process ends, after the app's loop has returned.
pub fn apply(prepared: Prepared, relaunch: Option<Relaunch>) -> Result<()> {
    match prepared {
        Prepared::Installer { setup } => {
            let mut command = Command::new(&setup);
            command.args([
                "/VERYSILENT",
                "/SUPPRESSMSGBOXES",
                "/NORESTART",
                "/CLOSEAPPLICATIONS",
                "/CURRENTUSER",
            ]);
            match relaunch {
                Some(Relaunch::Foreground) => command.arg("/RELAUNCH=foreground"),
                Some(Relaunch::Background) => command.arg("/RELAUNCH=background"),
                None => &mut command,
            };
            command
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
                .with_context(|| format!("starting {}", setup.display()))?;
            Ok(())
        }
        Prepared::Portable { files, exe } => {
            swap_portable(&files, &exe)?;
            if let Some(mode) = relaunch {
                start(&exe, mode)?;
            }
            Ok(())
        }
        Prepared::MacApp { staged, bundle } => {
            swap_bundle(&staged, &bundle)?;
            if let Some(mode) = relaunch {
                start(&bundle.join("Contents/MacOS/cosmic-mail"), mode)?;
            }
            Ok(())
        }
    }
}

/// Move the running exe aside (Windows allows renaming it) and copy the new files in.
fn swap_portable(files: &Path, exe: &Path) -> Result<()> {
    let dir = exe.parent().context("the executable has no folder")?;
    let old = old_exe(exe);
    let _ = std::fs::remove_file(&old);
    std::fs::rename(exe, &old).with_context(|| format!("moving {} aside", exe.display()))?;
    if let Err(err) = std::fs::copy(files.join("cosmic-mail.exe"), exe) {
        let _ = std::fs::rename(&old, exe);
        return Err(err).context("copying the new cosmic-mail.exe");
    }
    // The rest (licence, notification icon) is best effort: the app runs without them.
    if let Ok(entries) = std::fs::read_dir(files) {
        for entry in entries.flatten() {
            let name = entry.file_name();
            if name != "cosmic-mail.exe" && entry.path().is_file() {
                if let Err(err) = std::fs::copy(entry.path(), dir.join(&name)) {
                    tracing::warn!(error = %err, file = ?name, "could not update a file");
                }
            }
        }
    }
    Ok(())
}

fn old_exe(exe: &Path) -> PathBuf {
    let mut name = exe.as_os_str().to_owned();
    name.push(".old");
    PathBuf::from(name)
}

/// Swap the prepared bundle in, undoing the first rename if the second fails.
fn swap_bundle(staged: &Path, bundle: &Path) -> Result<()> {
    let parent = bundle.parent().context("the app bundle has no folder")?;
    let old = parent.join(MAC_OLD);
    if old.exists() {
        std::fs::remove_dir_all(&old).with_context(|| format!("clearing {}", old.display()))?;
    }
    std::fs::rename(bundle, &old).with_context(|| format!("moving {} aside", bundle.display()))?;
    if let Err(err) = std::fs::rename(staged, bundle) {
        let _ = std::fs::rename(&old, bundle);
        return Err(err).context("moving the new app into place");
    }
    if let Err(err) = std::fs::remove_dir_all(&old) {
        tracing::warn!(error = %err, "could not delete the previous app; the next start will");
    }
    Ok(())
}

/// Start the new version. It waits, reading the stdin pipe this process holds, until this
/// process has exited, so it doesn't hand itself to us over the single-instance socket.
fn start(exe: &Path, mode: Relaunch) -> Result<()> {
    let mut command = Command::new(exe);
    if mode == Relaunch::Background {
        command.arg("--background");
    }
    let mut child = command
        .env(AFTER_UPDATE_ENV, "1")
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .with_context(|| format!("starting {}", exe.display()))?;
    // Keep our end open until the process exits; the OS closes it then.
    std::mem::forget(child.stdin.take());
    Ok(())
}

/// In a process an update relaunched: wait (at most 30 s) for the previous one to exit,
/// which closes our stdin. Call before claiming the single instance.
pub fn wait_for_previous_instance() {
    if std::env::var_os(AFTER_UPDATE_ENV).is_none() {
        return;
    }
    let (tx, rx) = std::sync::mpsc::channel();
    let spawned = std::thread::Builder::new()
        .name("update-wait".into())
        .spawn(move || {
            let mut stdin = std::io::stdin();
            let mut buf = [0u8; 64];
            while matches!(stdin.read(&mut buf), Ok(n) if n > 0) {}
            let _ = tx.send(());
        });
    if spawned.is_ok() && rx.recv_timeout(WAIT_FOR_PREVIOUS).is_err() {
        tracing::warn!("the previous instance didn't exit in time; starting anyway");
    }
}

/// Delete what an earlier update left behind. Call once this process owns the instance.
pub fn clean_leftovers() {
    if let Ok(dir) = updates_dir() {
        if dir.exists() {
            // The Windows installer may still be running from here: best effort.
            let _ = std::fs::remove_dir_all(&dir);
        }
    }
    let Ok(exe) = std::env::current_exe() else {
        return;
    };
    if cfg!(target_os = "windows") {
        let old = old_exe(&exe);
        if old.exists() {
            let _ = std::fs::remove_file(old);
        }
    }
    if cfg!(target_os = "macos") {
        if let Some(parent) = bundle_of(&exe).as_deref().and_then(Path::parent) {
            for name in [MAC_STAGED, MAC_OLD] {
                let path = parent.join(name);
                if path.exists() {
                    let _ = std::fs::remove_dir_all(path);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(s: &str) -> Version {
        Version::parse(s).expect("version")
    }

    const DIGEST: &str = "sha256:8929159924a23b5b7119a5f71045378f5c7d45b75ad01d796c72f3d62a158fc6";

    fn asset(name: &str, digest: Option<&str>) -> ApiAsset {
        ApiAsset {
            name: name.into(),
            size: 10,
            browser_download_url: format!("https://github.com/x/{name}"),
            digest: digest.map(Into::into),
        }
    }

    fn release(tag: &str, assets: Vec<ApiAsset>) -> ApiRelease {
        ApiRelease {
            tag_name: tag.into(),
            html_url: format!("https://github.com/PlusCosmic/cosmic-mail/releases/tag/{tag}"),
            draft: false,
            prerelease: false,
            assets,
        }
    }

    #[test]
    fn versions_parse_and_order_numerically() {
        assert_eq!(v("v0.1.149"), v("0.1.149"));
        assert!(v("0.1.150") > v("0.1.149"));
        assert!(v("0.1.100") > v("0.1.99"));
        assert!(v("0.2.0") > v("0.1.999"));
        assert!(v("1.0.0") > v("0.9.9"));
        assert_eq!(v("0.1.7").to_string(), "0.1.7");
        for bad in [
            "",
            "v",
            "1.2",
            "1.2.3.4",
            "1.2.x",
            "1.2.3-rc1",
            "1..3",
            "+1.2.3",
        ] {
            assert_eq!(Version::parse(bad), None, "{bad}");
        }
    }

    #[test]
    fn digests_parse_only_as_sha256_hex() {
        let d = parse_digest(DIGEST).expect("digest");
        assert_eq!(d[0], 0x89);
        assert_eq!(d[31], 0xc6);
        assert_eq!(parse_digest("md5:abcd"), None);
        assert_eq!(parse_digest("sha256:abcd"), None);
        assert_eq!(
            parse_digest(&format!("{}zz", &DIGEST[..DIGEST.len() - 2])),
            None
        );
    }

    #[test]
    fn picks_the_newest_release_with_this_platforms_file() {
        let releases = vec![
            // Newest, but its Windows build failed: no file.
            release(
                "v0.1.152",
                vec![asset("cosmic-mail-0.1.152-macos-arm64.dmg", Some(DIGEST))],
            ),
            release(
                "v0.1.151",
                vec![
                    asset("cosmic-mail-0.1.151-windows-x64-setup.exe", Some(DIGEST)),
                    asset("cosmic-mail-0.1.151-windows-x64-portable.zip", Some(DIGEST)),
                ],
            ),
            release(
                "v0.1.150",
                vec![asset(
                    "cosmic-mail-0.1.150-windows-x64-setup.exe",
                    Some(DIGEST),
                )],
            ),
        ];
        let found = pick(
            &releases,
            v("0.1.149"),
            UpdateMethod::Installer,
            "windows",
            "x86_64",
        )
        .expect("found");
        assert_eq!(found.version, v("0.1.151"));
        assert_eq!(found.info(false).current_version, "0.1.149");
        let a = found.asset.expect("asset");
        assert_eq!(a.name, "cosmic-mail-0.1.151-windows-x64-setup.exe");
        assert_eq!(a.size, 10);

        let mac = pick(
            &releases,
            v("0.1.149"),
            UpdateMethod::MacApp,
            "macos",
            "aarch64",
        )
        .expect("found");
        assert_eq!(mac.version, v("0.1.152"));
        // An Intel Mac can't run the arm64 image.
        assert_eq!(
            pick(
                &releases,
                v("0.1.149"),
                UpdateMethod::MacApp,
                "macos",
                "x86_64"
            ),
            None
        );
    }

    #[test]
    fn nothing_newer_means_none() {
        let releases = vec![
            release(
                "v0.1.149",
                vec![asset(
                    "cosmic-mail-0.1.149-windows-x64-setup.exe",
                    Some(DIGEST),
                )],
            ),
            release(
                "v0.1.148",
                vec![asset(
                    "cosmic-mail-0.1.148-windows-x64-setup.exe",
                    Some(DIGEST),
                )],
            ),
        ];
        assert_eq!(
            pick(
                &releases,
                v("0.1.149"),
                UpdateMethod::Installer,
                "windows",
                "x86_64"
            ),
            None
        );
    }

    #[test]
    fn skips_drafts_prereleases_odd_tags_and_files_without_digests() {
        let mut draft = release(
            "v0.1.160",
            vec![asset(
                "cosmic-mail-0.1.160-windows-x64-setup.exe",
                Some(DIGEST),
            )],
        );
        draft.draft = true;
        let mut pre = release(
            "v0.1.159",
            vec![asset(
                "cosmic-mail-0.1.159-windows-x64-setup.exe",
                Some(DIGEST),
            )],
        );
        pre.prerelease = true;
        let releases = vec![
            draft,
            pre,
            release(
                "nightly",
                vec![asset(
                    "cosmic-mail-nightly-windows-x64-setup.exe",
                    Some(DIGEST),
                )],
            ),
            release(
                "v0.1.158",
                vec![asset("cosmic-mail-0.1.158-windows-x64-setup.exe", None)],
            ),
            // A file for another version inside this release doesn't count.
            release(
                "v0.1.157",
                vec![asset(
                    "cosmic-mail-0.1.156-windows-x64-setup.exe",
                    Some(DIGEST),
                )],
            ),
            release(
                "v0.1.155",
                vec![asset(
                    "cosmic-mail-0.1.155-windows-x64-setup.exe",
                    Some(DIGEST),
                )],
            ),
        ];
        let found = pick(
            &releases,
            v("0.1.150"),
            UpdateMethod::Installer,
            "windows",
            "x86_64",
        )
        .expect("found");
        assert_eq!(found.version, v("0.1.155"));
    }

    #[test]
    fn the_package_manager_needs_no_file() {
        let releases = vec![
            release("v0.1.151", Vec::new()),
            release("v0.1.150", Vec::new()),
        ];
        let found = pick(
            &releases,
            v("0.1.149"),
            UpdateMethod::PackageManager,
            "linux",
            "x86_64",
        )
        .expect("found");
        assert_eq!(found.version, v("0.1.151"));
        assert_eq!(found.asset, None);
        assert!(!found.method.installs());
    }

    #[test]
    fn download_takes_any_file_for_this_os() {
        let releases = vec![release(
            "v0.1.151",
            vec![asset(
                "cosmic-mail-0.1.151-windows-x64-portable.zip",
                Some(DIGEST),
            )],
        )];
        assert!(pick(
            &releases,
            v("0.1.149"),
            UpdateMethod::Download,
            "windows",
            "x86_64"
        )
        .is_some());
        assert!(pick(
            &releases,
            v("0.1.149"),
            UpdateMethod::Download,
            "macos",
            "aarch64"
        )
        .is_none());
    }

    #[test]
    fn finds_the_bundle_around_an_executable() {
        let exe = Path::new("/Applications/Cosmic Mail.app/Contents/MacOS/cosmic-mail");
        assert_eq!(
            bundle_of(exe),
            Some(PathBuf::from("/Applications/Cosmic Mail.app"))
        );
        assert_eq!(bundle_of(Path::new("/usr/bin/cosmic-mail")), None);
        assert_eq!(bundle_of(Path::new("/x/target/release/cosmic-mail")), None);
    }

    #[test]
    fn modes_and_methods_serialize_as_the_contract_says() {
        assert_eq!(
            serde_json::to_string(&UpdateMode::Auto).unwrap(),
            "\"auto\""
        );
        assert_eq!(
            serde_json::to_string(&UpdateMode::default()).unwrap(),
            "\"notify\""
        );
        assert_eq!(
            serde_json::to_string(&UpdateMethod::PackageManager).unwrap(),
            "\"packageManager\""
        );
        assert_eq!(
            serde_json::to_string(&UpdateMethod::MacApp).unwrap(),
            "\"macApp\""
        );
    }

    #[test]
    fn swaps_a_bundle_and_rolls_back_when_the_new_one_is_missing() {
        let dir = tempfile::tempdir().unwrap();
        let bundle = dir.path().join("Cosmic Mail.app");
        let staged = dir.path().join(MAC_STAGED);
        std::fs::create_dir_all(bundle.join("Contents")).unwrap();
        std::fs::write(bundle.join("Contents/v"), "old").unwrap();

        // No staged bundle: the installed one must come back.
        assert!(swap_bundle(&staged, &bundle).is_err());
        assert_eq!(
            std::fs::read_to_string(bundle.join("Contents/v")).unwrap(),
            "old"
        );

        std::fs::create_dir_all(staged.join("Contents")).unwrap();
        std::fs::write(staged.join("Contents/v"), "new").unwrap();
        swap_bundle(&staged, &bundle).unwrap();
        assert_eq!(
            std::fs::read_to_string(bundle.join("Contents/v")).unwrap(),
            "new"
        );
        assert!(!staged.exists());
        assert!(!dir.path().join(MAC_OLD).exists());
    }

    #[test]
    fn swaps_portable_files_beside_the_exe() {
        let dir = tempfile::tempdir().unwrap();
        let app = dir.path().join("app");
        let files = dir.path().join("files");
        std::fs::create_dir_all(&app).unwrap();
        std::fs::create_dir_all(&files).unwrap();
        let exe = app.join("cosmic-mail.exe");
        std::fs::write(&exe, "old").unwrap();
        std::fs::write(files.join("cosmic-mail.exe"), "new").unwrap();
        std::fs::write(files.join("icon.png"), "icon").unwrap();

        swap_portable(&files, &exe).unwrap();
        assert_eq!(std::fs::read_to_string(&exe).unwrap(), "new");
        assert_eq!(std::fs::read_to_string(old_exe(&exe)).unwrap(), "old");
        assert_eq!(
            std::fs::read_to_string(app.join("icon.png")).unwrap(),
            "icon"
        );
    }

    #[test]
    fn a_portable_swap_without_a_new_exe_restores_the_old_one() {
        let dir = tempfile::tempdir().unwrap();
        let exe = dir.path().join("cosmic-mail.exe");
        std::fs::write(&exe, "old").unwrap();
        let empty = dir.path().join("empty");
        std::fs::create_dir_all(&empty).unwrap();
        assert!(swap_portable(&empty, &exe).is_err());
        assert_eq!(std::fs::read_to_string(&exe).unwrap(), "old");
    }

    /// Live: asks GitHub (`cargo test --lib -- --ignored update`).
    #[tokio::test]
    #[ignore]
    async fn live_check_finds_a_release_for_an_old_version() {
        let _ = rustls::crypto::ring::default_provider().install_default();
        let method = if cfg!(target_os = "windows") {
            UpdateMethod::Installer
        } else if cfg!(target_os = "macos") {
            UpdateMethod::MacApp
        } else {
            UpdateMethod::PackageManager
        };
        let found = check(v("0.1.0"), method)
            .await
            .expect("check")
            .expect("found");
        assert!(found.version > v("0.1.0"));
        assert_eq!(found.asset.is_some(), method.installs());
    }

    #[tokio::test]
    async fn a_cached_download_with_the_right_digest_is_reused() {
        let dir = tempfile::tempdir().unwrap();
        let name = "cosmic-mail-0.1.151-windows-x64-setup.exe";
        std::fs::write(dir.path().join(name), b"hello").unwrap();
        let digest: [u8; 32] = Sha256::digest(b"hello").into();
        let a = Asset {
            name: name.into(),
            // Never fetched: the cached file already matches.
            url: "https://invalid.example/never".into(),
            size: 5,
            sha256: digest,
        };
        assert_eq!(
            download(&a, dir.path()).await.unwrap(),
            dir.path().join(name)
        );
    }
}
