// Pin the engine compiled into this binary, not whichever sources exist at runtime.
use sha2::{Digest, Sha256};
use std::{
    fs,
    path::{Path, PathBuf},
};

fn sources(directory: &Path, files: &mut Vec<PathBuf>) -> std::io::Result<()> {
    println!("cargo:rerun-if-changed={}", directory.display());
    for entry in fs::read_dir(directory)? {
        let path = entry?.path();
        if path.is_dir() {
            sources(&path, files)?;
        } else if path.extension().is_some_and(|e| e == "rs") {
            files.push(path);
        }
    }
    Ok(())
}
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let root = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR")?).join("../..");
    let mut files = vec![root.join("Cargo.lock")];
    for name in [
        "cna-core",
        "cna-protocol",
        "cna-content",
        "cna-tables",
        "cna-rules",
    ] {
        let package = root.join("crates").join(name);
        files.push(package.join("Cargo.toml"));
        sources(&package.join("src"), &mut files)?;
    }
    files.sort();
    let mut digest = Sha256::new();
    digest.update(b"cna-engine-sources-v1");
    for file in files {
        println!("cargo:rerun-if-changed={}", file.display());
        let name = file
            .strip_prefix(&root)?
            .to_string_lossy()
            .replace('\\', "/");
        let bytes = fs::read(&file)?;
        digest.update((name.len() as u64).to_le_bytes());
        digest.update(name.as_bytes());
        digest.update((bytes.len() as u64).to_le_bytes());
        digest.update(bytes);
    }
    println!(
        "cargo:rustc-env=CNA_ENGINE_SOURCE_HASH={:x}",
        digest.finalize()
    );
    Ok(())
}
