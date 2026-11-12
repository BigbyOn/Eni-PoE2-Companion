fn walk(dir: &std::path::Path) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for entry in entries.flatten() {
        let path = entry.path();
        println!("cargo:rerun-if-changed={}", path.display());
        if path.is_dir() {
            walk(&path);
        }
    }
}

fn main() {
    // `ui/` and `assets/` are embedded through include_str!/generate_context!,
    // so rustc has to know when one of those files changes.
    walk(std::path::Path::new("ui"));
    walk(std::path::Path::new("assets"));
    tauri_build::build()
}
