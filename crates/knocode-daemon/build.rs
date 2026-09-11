//! Windows version resource for `knocode-daemon.exe`.
//!
//! A PE with no VERSIONINFO (blank ProductName/CompanyName/FileVersion in
//! Explorer / Defender / sigcheck) is a strong heuristic & ML-classifier
//! false-positive trigger. This embeds real metadata, synced from the
//! workspace version at compile time.
//!
//! Guarded to the `windows` TARGET so the resource compiles into the exe only
//! for Windows builds, regardless of the host OS building it.
//!
//! NOTE: keep resource strings ASCII — the .rc toolchain is codepage-sensitive
//! and non-ASCII characters get mangled.

fn main() {
    println!("cargo:rerun-if-changed=../../Cargo.toml");

    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }

    let version = std::env::var("CARGO_PKG_VERSION").unwrap_or_else(|_| "0.0.0".into());
    let mut res = winresource::WindowsResource::new();
    res.set("FileDescription", "Knocode daemon - local context service for coding agents");
    res.set("FileVersion", &version);
    res.set("ProductName", "Knocode");
    res.set("ProductVersion", &version);
    res.set("LegalCopyright", "Copyright (C) 2026 Knocode contributors");
    res.set("OriginalFilename", "knocode-daemon.exe");
    res.set("CompanyName", "Knocode contributors");

    if let Err(e) = res.compile() {
        eprintln!("cargo:warning=knocode-daemon: failed to embed Windows version resource: {e}");
    }
}
