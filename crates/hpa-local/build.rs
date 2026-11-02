use std::path::Path;

fn main() {
    let migrations = Path::new("../../migrations/postgres");
    println!("cargo:rerun-if-changed={}", migrations.display());
    if let Ok(entries) = std::fs::read_dir(migrations) {
        for entry in entries.flatten() {
            println!("cargo:rerun-if-changed={}", entry.path().display());
        }
    }
}
