//! Reproduce physical file sharing; pass a new empty directory outside user data.
use std::path::PathBuf;
use vivian_storage_contracts::assets;

fn main() {
    let root = PathBuf::from(
        std::env::args_os()
            .nth(1)
            .expect("new measurement directory"),
    );
    std::fs::create_dir(&root).expect("measurement directory must be new");
    let bytes: Vec<u8> = (0..3_234_352).map(|i| (i % 251) as u8).collect();
    // Optional second argument puts the content pool on another filesystem to
    // exercise the ordinary-copy fallback instead of hard links.
    let objects = std::env::args_os().nth(2).map(PathBuf::from)
        .unwrap_or_else(|| root.join("after/objects"));
    for i in 0..44 {
        let before = root.join(format!("before/note-{i}/fonts/font.woff2"));
        std::fs::create_dir_all(before.parent().unwrap()).unwrap();
        std::fs::write(&before, &bytes).unwrap();
        assets::materialize(
            &objects,
            &root.join(format!("after/note-{i}/fonts/font.woff2")),
            &bytes,
        )
        .unwrap();
    }
    println!("{}", root.display());
}
