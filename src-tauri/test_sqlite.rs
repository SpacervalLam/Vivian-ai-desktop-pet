use std::path::PathBuf;

fn main() {
    let appdata = std::env::var("APPDATA").unwrap();
    let path = PathBuf::from(&appdata).join("vivian").join("characters").join("nana").join("memory").join("vectors_test.db");
    println!("Path: {:?}", path);
    println!("Parent exists: {:?}", path.parent().map(|p| p.exists()));
    println!("Parent: {:?}", path.parent());
    
    // Create parent dirs
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).unwrap();
    }
    
    let conn = rusqlite::Connection::open(&path);
    match conn {
        Ok(c) => {
            c.execute_batch("CREATE TABLE IF NOT EXISTS test (id INTEGER);").unwrap();
            println!("SUCCESS: opened and created table");
            drop(c);
            std::fs::remove_file(&path).unwrap();
        }
        Err(e) => {
            println!("FAILED: {:?}", e);
        }
    }
}
