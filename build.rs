fn main() {
    // FORCE_REBUILD: kein reales File → build.rs wird bei jedem "cargo build" ausgeführt.
    // Das stellt sicher dass die Slint-UI immer neu kompiliert wird ohne alle
    // Abhängigkeiten neu zu bauen (wie bei "cargo clean && cargo build").
    println!("cargo:rerun-if-changed=FORCE_REBUILD");

    slint_build::compile("ui/appwindow.slint").expect("Slint build failed");
    embed_windows_resources();
}

#[cfg(windows)]
fn embed_windows_resources() {
    // Use an absolute path so winresource always finds the file,
    // regardless of the working directory during the build.
    let manifest_dir = std::env::var("CARGO_MANIFEST_DIR").unwrap();
    let icon_path = format!("{}/assets/icon.ico", manifest_dir);

    let mut res = winresource::WindowsResource::new();
    res.set_icon(&icon_path);
    res.compile().expect("Failed to embed Windows icon resource");
}

#[cfg(not(windows))]
fn embed_windows_resources() {}
