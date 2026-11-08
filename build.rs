//! Embeds the app icon (and version info) in the Windows executable, so
//! Explorer, the taskbar and the installer show it. Nothing on other targets.

fn main() {
    println!("cargo:rerun-if-changed=assets/icon.ico");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    let mut res = winresource::WindowsResource::new();
    res.set_icon("assets/icon.ico")
        .set("ProductName", "Cosmic Mail")
        .set("FileDescription", "Cosmic Mail")
        .set("CompanyName", "PlusCosmic");
    if let Err(err) = res.compile() {
        // A missing resource compiler must not break the build; the exe just
        // has no embedded icon.
        println!("cargo:warning=could not embed the Windows icon: {err}");
    }
}
