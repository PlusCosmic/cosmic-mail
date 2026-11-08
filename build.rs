//! Embeds the app icon (and version info) in the Windows executable, so
//! Explorer, the taskbar and the installer show it. Nothing on other targets.

fn main() {
    println!("cargo:rerun-if-changed=assets/icon.ico");
    // CI and the packages set the release version (scripts/version.sh);
    // src/lib.rs reads it as `cosmic_mail::VERSION`.
    println!("cargo:rerun-if-env-changed=COSMIC_MAIL_VERSION");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    let version = std::env::var("COSMIC_MAIL_VERSION")
        .ok()
        .filter(|v| !v.is_empty())
        .unwrap_or_else(|| std::env::var("CARGO_PKG_VERSION").unwrap_or_default());
    let mut res = winresource::WindowsResource::new();
    res.set_icon("assets/icon.ico")
        .set("ProductName", "Cosmic Mail")
        .set("FileDescription", "Cosmic Mail")
        .set("CompanyName", "PlusCosmic")
        .set("ProductVersion", &version)
        .set("FileVersion", &version);
    if let Err(err) = res.compile() {
        // A missing resource compiler must not break the build; the exe just
        // has no embedded icon.
        println!("cargo:warning=could not embed the Windows icon: {err}");
    }
}
