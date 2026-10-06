// Pin the engine compiled into this binary, not runtime sources.
mod build_fingerprint;
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let root = std::path::PathBuf::from(std::env::var("CARGO_MANIFEST_DIR")?).join("../..");
    let (hash, watched) = build_fingerprint::fingerprint(&root)?;
    for path in watched {
        println!("cargo:rerun-if-changed={}", path.display());
    }
    println!("cargo:rustc-env=CNA_ENGINE_SOURCE_HASH={hash}");
    Ok(())
}
