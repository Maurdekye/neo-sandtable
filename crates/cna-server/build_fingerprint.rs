// Shared build-time fingerprint for every campaign ruleset.
use sha2::{Digest, Sha256};
use std::{
    fs,
    path::{Path, PathBuf},
};
pub const CRATES: &[&str] = &[
    "cna-core",
    "cna-protocol",
    "cna-content",
    "cna-tables",
    "cna-rules",
    "cna-sandbox",
];
fn sources(
    directory: &Path,
    files: &mut Vec<PathBuf>,
    watched: &mut Vec<PathBuf>,
) -> std::io::Result<()> {
    watched.push(directory.to_owned());
    for entry in fs::read_dir(directory)? {
        let path = entry?.path();
        if path.is_dir() {
            sources(&path, files, watched)?;
        } else if path.extension().is_some_and(|e| e == "rs") {
            files.push(path);
        }
    }
    Ok(())
}
pub fn fingerprint(root: &Path) -> Result<(String, Vec<PathBuf>), Box<dyn std::error::Error>> {
    let mut files = vec![root.join("Cargo.lock"), root.join("Cargo.toml")];
    let mut watched = Vec::new();
    for name in CRATES {
        let package = root.join("crates").join(name);
        files.push(package.join("Cargo.toml"));
        sources(&package.join("src"), &mut files, &mut watched)?;
    }
    files.sort();
    let mut digest = Sha256::new();
    digest.update(b"cna-engine-sources-v1");
    for file in &files {
        let name = file
            .strip_prefix(root)?
            .to_string_lossy()
            .replace('\\', "/");
        let bytes = fs::read(file)?;
        digest.update((name.len() as u64).to_le_bytes());
        digest.update(name.as_bytes());
        digest.update((bytes.len() as u64).to_le_bytes());
        digest.update(bytes);
    }
    watched.extend(files);
    Ok((format!("{:x}", digest.finalize()), watched))
}
