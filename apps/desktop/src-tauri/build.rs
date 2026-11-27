fn main() {
    // When this build was made: the updater refuses an update signed before it (see `privcord_desktop_host::update_guard`).
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    println!("cargo:rustc-env=PRIVCORD_BUILT_AT={now}");
    tauri_build::build()
}
