//! File-level retained session checks. No conversation, compaction, permission
//! or cost schema is validated. Only operative path references and a bounded
//! observed-window persistence witness are examined. Native admission is separate.
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{
    fs,
    io::Read,
    os::windows::{
        fs::{MetadataExt, OpenOptionsExt},
        process::CommandExt,
    },
    path::{Component, Path, PathBuf, Prefix},
    process::{Command, Stdio},
    time::{Duration, Instant},
};
use uuid::Uuid;

const TRANSCRIPT_BYTES: u64 = 32 * 1024 * 1024;
const TOTAL_BYTES: u64 = 128 * 1024 * 1024;
const FILES: usize = 4096;

#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum StateRefusal {
    #[error("retained session file metadata unavailable or unsafe")]
    Metadata,
    #[error("unapproved retained session path or file")]
    Inventory,
    #[error("retained session size/time bound exceeded")]
    Limit,
    #[error("operative reference leaves this seat's run directories")]
    Reference,
    #[error("current-window persistence witness unavailable or ambiguous")]
    Witness,
    #[error("retained session file identity or bytes changed")]
    Changed,
}

/// Never Debug/serialize observed content. The driver must reset this per turn
/// and only capture actively consumed non-replay main assistant frames.
#[derive(Clone)]
pub struct ObservedAssistant {
    session: Uuid,
    id: String,
    time: String,
    uuid: String,
    content: Value,
}
impl ObservedAssistant {
    pub fn from_frame(frame: &Value, session: Uuid) -> Option<Self> {
        if frame["type"] != "assistant"
            || frame["session_id"].as_str() != Some(session.to_string().as_str())
            || frame
                .get("parent_tool_use_id")
                .is_some_and(|v| !v.is_null())
            || frame["isReplay"] == true
            || frame["is_replay"] == true
            || frame.get("error").is_some_and(|v| !v.is_null())
        {
            return None;
        }
        let content = frame["message"]["content"].as_array()?;
        if content.len() != 1 {
            return None;
        }
        Some(Self {
            session,
            id: frame["message"]["id"].as_str()?.into(),
            time: frame["timestamp"].as_str()?.into(),
            uuid: frame["uuid"].as_str()?.into(),
            content: frame["message"]["content"].clone(),
        })
    }
    fn matches(&self, row: &Value) -> bool {
        if row["type"] != "assistant"
            || row["sessionId"].as_str() != Some(self.session.to_string().as_str())
            || row["message"]["id"].as_str() != Some(self.id.as_str())
            || row["timestamp"].as_str() != Some(self.time.as_str())
        {
            return false;
        }
        let Some(raw) = row["uuid"].as_str() else {
            return false;
        };
        if Uuid::parse_str(raw).ok().map(|v| v.to_string()).as_deref() != Some(raw) {
            return false;
        }
        let Some(blocks) = row["message"]["content"].as_array() else {
            return false;
        };
        blocks.iter().enumerate().any(|(index, block)| {
            let projected = if blocks.len() > 1 {
                format!("{}{:012x}", &raw[..24], index)
            } else {
                raw.to_string()
            };
            self.uuid == projected && self.content == Value::Array(vec![block.clone()])
        })
    }
}

#[derive(Clone, PartialEq, Eq)]
struct Stamp {
    path: PathBuf,
    volume: u64,
    index: u64,
    bytes: u64,
    written: Option<u64>,
    digest: [u8; 32],
}
/// This is an in-memory same-run/seat/session file binding, not a receipt.
/// Only a naturally successful, tree-drained park may create one in the driver.
pub struct ParkedFiles {
    config: PathBuf,
    cwd: PathBuf,
    session: Uuid,
    files: Vec<Stamp>,
    original: Vec<u8>,
    observed: ObservedAssistant,
    leaf: Value,
}
impl ParkedFiles {
    pub fn capture(
        config: &Path,
        cwd: &Path,
        session: Uuid,
        observed: ObservedAssistant,
    ) -> Result<Self, StateRefusal> {
        if observed.session != session {
            return Err(StateRefusal::Witness);
        }
        let (files, original) = inventory(config, cwd, session)?;
        let leaf = references_and_witness(&original, config, cwd, session, &observed)?;
        Ok(Self {
            config: config.into(),
            cwd: cwd.into(),
            session,
            files,
            original,
            observed,
            leaf,
        })
    }
    pub fn verify(&self, config: &Path, cwd: &Path, session: Uuid) -> Result<(), StateRefusal> {
        if self.config != config || self.cwd != cwd || self.session != session {
            return Err(StateRefusal::Reference);
        }
        let (files, original) = inventory(config, cwd, session)?;
        let leaf = references_and_witness(&original, config, cwd, session, &self.observed)?;
        if files != self.files || original != self.original || leaf != self.leaf {
            return Err(StateRefusal::Changed);
        }
        Ok(())
    }
}

fn plain(path: &Path, directory: bool) -> Result<(), StateRefusal> {
    let m = fs::symlink_metadata(path).map_err(|_| StateRefusal::Metadata)?;
    if m.file_attributes() & 0x400 != 0 || if directory { !m.is_dir() } else { !m.is_file() } {
        return Err(StateRefusal::Metadata);
    }
    Ok(())
}
fn ancestors(path: &Path) -> Result<(), StateRefusal> {
    for ancestor in path.ancestors() {
        plain(ancestor, true)?;
    }
    Ok(())
}
fn ordinary(path: &Path) -> bool {
    path.is_absolute()
        && matches!(path.components().next(),Some(Component::Prefix(p)) if matches!(p.kind(),Prefix::Disk(_)))
        && path.components().all(|p| match p {
            Component::Normal(s) => !s.to_string_lossy().contains(':'),
            Component::ParentDir | Component::CurDir => false,
            _ => true,
        })
}
fn contained(path: &str, config: &Path, cwd: &Path) -> Result<PathBuf, StateRefusal> {
    let path = Path::new(path);
    if !ordinary(path) {
        return Err(StateRefusal::Reference);
    }
    let metadata = fs::symlink_metadata(path).map_err(|_| StateRefusal::Reference)?;
    if metadata.file_attributes() & 0x400 != 0 {
        return Err(StateRefusal::Reference);
    }
    if metadata.is_dir() {
        ancestors(path)?;
    } else {
        ancestors(path.parent().ok_or(StateRefusal::Reference)?)?;
        plain(path, false)?;
    }
    let resolved = path.canonicalize().map_err(|_| StateRefusal::Reference)?;
    let c = config.canonicalize().map_err(|_| StateRefusal::Reference)?;
    let w = cwd.canonicalize().map_err(|_| StateRefusal::Reference)?;
    if !resolved.starts_with(c) && !resolved.starts_with(w) {
        return Err(StateRefusal::Reference);
    }
    Ok(resolved)
}
fn entries(path: &Path) -> Result<Vec<PathBuf>, StateRefusal> {
    plain(path, true)?;
    let mut paths = Vec::new();
    for entry in fs::read_dir(path).map_err(|_| StateRefusal::Metadata)? {
        if paths.len() == FILES {
            return Err(StateRefusal::Limit);
        }
        paths.push(entry.map_err(|_| StateRefusal::Metadata)?.path());
    }
    paths.sort();
    Ok(paths)
}
fn exact_directory(path: &Path, child: &str) -> Result<(), StateRefusal> {
    if entries(path)? != vec![path.join(child)] {
        return Err(StateRefusal::Inventory);
    }
    Ok(())
}
fn inventory(
    config: &Path,
    cwd: &Path,
    session: Uuid,
) -> Result<(Vec<Stamp>, Vec<u8>), StateRefusal> {
    if !ordinary(config) || !ordinary(cwd) {
        return Err(StateRefusal::Reference);
    }
    ancestors(config)?;
    ancestors(cwd)?;
    if !entries(cwd)?.is_empty() {
        return Err(StateRefusal::Inventory);
    }
    exact_directory(config, "projects")?;
    let projects = config.join("projects");
    exact_directory(&projects, "watched-seat")?;
    let project = projects.join("watched-seat");
    let transcript = project.join(format!("{session}.jsonl"));
    let sidecar = project.join(session.to_string());
    let mut paths = vec![transcript.clone()];
    let mut directories = vec![
        cwd.to_path_buf(),
        config.to_path_buf(),
        projects.clone(),
        project.clone(),
    ];
    let mut found = false;
    for entry in entries(&project)? {
        if entry == transcript {
            found = true;
        } else if entry == sidecar {
            exact_directory(&sidecar, "tool-results")?;
            directories.push(sidecar.clone());
            directories.push(sidecar.join("tool-results"));
            for output in entries(&sidecar.join("tool-results"))? {
                if !matches!(
                    output.extension().and_then(|v| v.to_str()),
                    Some("txt" | "json")
                ) || output.file_stem().is_none_or(|v| v.is_empty())
                {
                    return Err(StateRefusal::Inventory);
                }
                paths.push(output);
            }
        } else {
            return Err(StateRefusal::Inventory);
        }
    }
    if !found || paths.len() > FILES {
        return Err(StateRefusal::Inventory);
    }
    paths.sort();
    let deadline = Instant::now() + Duration::from_secs(10);
    let mut stamps = Vec::new();
    let mut original = Vec::new();
    let mut total = 0u64;
    for path in &paths {
        if Instant::now() >= deadline {
            return Err(StateRefusal::Limit);
        }
        plain(path, false)?;
        let mut file = fs::OpenOptions::new()
            .read(true)
            .share_mode(1)
            .custom_flags(0x0020_0000)
            .open(path)
            .map_err(|_| StateRefusal::Metadata)?;
        let info = winapi_util::file::information(&file).map_err(|_| StateRefusal::Metadata)?;
        if info.file_attributes() & 0x400 != 0 || info.number_of_links() != 1 {
            return Err(StateRefusal::Metadata);
        }
        total = total
            .checked_add(info.file_size())
            .ok_or(StateRefusal::Limit)?;
        if total > TOTAL_BYTES
            || (path == &transcript
                && (info.file_size() == 0 || info.file_size() > TRANSCRIPT_BYTES))
        {
            return Err(StateRefusal::Limit);
        }
        let mut digest = Sha256::new();
        let mut read = 0u64;
        let mut buffer = [0u8; 64 * 1024];
        loop {
            if Instant::now() >= deadline {
                return Err(StateRefusal::Limit);
            }
            let length = file.read(&mut buffer).map_err(|_| StateRefusal::Metadata)?;
            if length == 0 {
                break;
            }
            read = read.checked_add(length as u64).ok_or(StateRefusal::Limit)?;
            if read > info.file_size() {
                return Err(StateRefusal::Changed);
            }
            digest.update(&buffer[..length]);
            if path == &transcript {
                original.extend_from_slice(&buffer[..length]);
            }
        }
        if read != info.file_size() {
            return Err(StateRefusal::Changed);
        }
        stamps.push(Stamp {
            path: path
                .strip_prefix(config)
                .map_err(|_| StateRefusal::Reference)?
                .into(),
            volume: info.volume_serial_number(),
            index: info.file_index(),
            bytes: info.file_size(),
            written: info.last_write_time(),
            digest: digest.finalize().into(),
        });
    }
    // Only the unnamed file stream is adopted. Bounded trusted metadata helper,
    // no CLI/account/environment authority and no source/artifact manifest.
    for batch in paths.chunks(16) {
        stream_check(batch, deadline)?;
    }
    for directory in directories {
        directory_stream_check(&directory, deadline)?;
    }
    Ok((stamps, original))
}
fn stream_check(paths: &[PathBuf], deadline: Instant) -> Result<(), StateRefusal> {
    let literals = paths
        .iter()
        .map(|p| format!("'{}'", p.to_string_lossy().replace('\'', "''")))
        .collect::<Vec<_>>()
        .join(",");
    let script = format!(
        "$ErrorActionPreference='Stop';foreach($p in @({literals})){{$s=@(Get-Item -LiteralPath $p -Stream * -ErrorAction Stop);if($s.Count -ne 1 -or $s[0].Stream -ne ':$DATA'){{exit 3}}}};exit 0"
    );
    if script.len() > 24_000 {
        return Err(StateRefusal::Limit);
    }
    let mut command = Command::new("C:/Windows/System32/WindowsPowerShell/v1.0/powershell.exe");
    command
        .args(["-NoProfile", "-NonInteractive", "-Command", &script])
        .env_clear()
        .env("SYSTEMROOT", "C:/Windows")
        .env("WINDIR", "C:/Windows")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .creation_flags(0x0800_0000);
    let mut child = command.spawn().map_err(|_| StateRefusal::Metadata)?;
    let bound = deadline.min(Instant::now() + Duration::from_secs(2));
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                return if status.success() {
                    Ok(())
                } else {
                    Err(StateRefusal::Metadata)
                };
            }
            Ok(None) if Instant::now() < bound => std::thread::sleep(Duration::from_millis(5)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(StateRefusal::Metadata);
            }
        }
    }
}

fn directory_stream_check(directory: &Path, deadline: Instant) -> Result<(), StateRefusal> {
    // WindowsPowerShell5.1 hides directory ADS even with -Stream *. Native
    // DIR /R enumerates streams on the current directory's "." entry. Only
    // that entry is relevant; parent-directory metadata is not adopted.
    // The path is environment data, not interpolated command source. CMD's
    // expansion is single-pass, quoted, with delayed expansion disabled.
    let mut child = Command::new(r"C:\Windows\System32\cmd.exe")
        .args(["/d", "/v:off", "/u", "/c"])
        // CMD does not use the normal argv quote/backslash convention.
        // This raw fragment is constant; directory data comes only from env.
        .raw_arg("dir /r /a /-c \"%CNA_DIRECTORY%\"")
        .env_clear()
        .env("SYSTEMROOT", "C:/Windows")
        .env("WINDIR", "C:/Windows")
        .env("CNA_DIRECTORY", directory)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .creation_flags(0x0800_0000)
        .spawn()
        .map_err(|_| StateRefusal::Metadata)?;
    const OUTPUT: u64 = 4 * 1024 * 1024;
    let Some(stdout) = child.stdout.take() else {
        let _ = child.kill();
        let _ = child.wait();
        return Err(StateRefusal::Metadata);
    };
    // Drain concurrently so a full metadata pipe cannot deadlock wait. DIR is
    // a builtin with no child process; both its root and reader are joined.
    let reader = match std::thread::Builder::new().spawn(move || {
        let mut bytes = Vec::new();
        stdout
            .take(OUTPUT + 1)
            .read_to_end(&mut bytes)
            .map(|_| bytes)
    }) {
        Ok(reader) => reader,
        Err(_) => {
            let _ = child.kill();
            let _ = child.wait();
            return Err(StateRefusal::Metadata);
        }
    };
    let bound = deadline.min(Instant::now() + Duration::from_secs(2));
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Some(status),
            Ok(None) if Instant::now() < bound => std::thread::sleep(Duration::from_millis(5)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                break None;
            }
        }
    };
    let bytes = reader
        .join()
        .map_err(|_| StateRefusal::Metadata)?
        .map_err(|_| StateRefusal::Metadata)?;
    if status.is_none_or(|s| !s.success()) || bytes.len() as u64 > OUTPUT || bytes.len() % 2 != 0 {
        return Err(StateRefusal::Metadata);
    }
    let units: Vec<_> = bytes
        .as_chunks::<2>()
        .0
        .iter()
        .map(|p| u16::from_le_bytes(*p))
        .collect();
    let text = String::from_utf16(&units).map_err(|_| StateRefusal::Metadata)?;
    for line in text.lines() {
        let line = line.trim_start();
        if let Some((size, rest)) = line.split_once(char::is_whitespace)
            && !size.is_empty()
            && size.bytes().all(|v| v.is_ascii_digit())
            && rest.trim_start().starts_with(".:")
        {
            return Err(StateRefusal::Metadata);
        }
    }
    Ok(())
}

fn references_and_witness(
    bytes: &[u8],
    config: &Path,
    cwd: &Path,
    session: Uuid,
    observed: &ObservedAssistant,
) -> Result<Value, StateRefusal> {
    if bytes.len() as u64 > TRANSCRIPT_BYTES || !bytes.ends_with(b"\n") {
        return Err(StateRefusal::Witness);
    }
    let text = std::str::from_utf8(bytes).map_err(|_| StateRefusal::Witness)?;
    if text
        .lines()
        .last()
        .is_none_or(|line| serde_json::from_str::<Value>(line).is_err())
    {
        return Err(StateRefusal::Witness);
    }
    let mut matches = 0;
    let mut leaf = None;
    let mut rows = Vec::new();
    for line in text.lines() {
        // No generic row schema: unrecognized metadata/cost/conversation is not
        // a gate. Parse only to locate actual operative paths and park markers.
        let Ok(row) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        check_references(&row, config, cwd, session)?;
        if observed.matches(&row) {
            matches += 1;
        }
        if row["type"] == "last-prompt"
            && row["sessionId"].as_str() == Some(session.to_string().as_str())
        {
            leaf = row.get("leafUuid").cloned();
        }
        rows.push(row);
    }
    if matches != 1 {
        return Err(StateRefusal::Witness);
    }
    let leaf = leaf
        .and_then(|v| v.as_str().map(str::to_owned))
        .ok_or(StateRefusal::Witness)?;
    let mut selected = rows.into_iter().filter(|row| {
        row["uuid"].as_str() == Some(leaf.as_str())
            && row["sessionId"].as_str() == Some(session.to_string().as_str())
    });
    let row = selected.next().ok_or(StateRefusal::Witness)?;
    if selected.next().is_some() {
        return Err(StateRefusal::Witness);
    }
    Ok(row)
}
fn check_references(
    row: &Value,
    config: &Path,
    cwd: &Path,
    session: Uuid,
) -> Result<(), StateRefusal> {
    if let Some(value) = row.get("cwd") {
        let p = contained(value.as_str().ok_or(StateRefusal::Reference)?, config, cwd)?;
        if p != cwd.canonicalize().map_err(|_| StateRefusal::Reference)? {
            return Err(StateRefusal::Reference);
        }
    }
    for key in ["relocatedCwd", "devModsFolder"] {
        if let Some(value) = row.get(key) {
            contained(value.as_str().ok_or(StateRefusal::Reference)?, config, cwd)?;
        }
    }
    if row["type"] == "dev-mods" {
        contained(
            row["folder"].as_str().ok_or(StateRefusal::Reference)?,
            config,
            cwd,
        )?;
    }
    if let Some(worktree) = row.get("worktreeSession")
        && !worktree.is_null()
    {
        let object = worktree.as_object().ok_or(StateRefusal::Reference)?;
        for key in ["worktreePath", "originalCwd", "liveLaunchAnchor"] {
            if let Some(value) = object.get(key) {
                contained(value.as_str().ok_or(StateRefusal::Reference)?, config, cwd)?;
            }
        }
    }
    if let Some(blocks) = row["message"]["content"].as_array() {
        for block in blocks {
            if block["type"] == "tool_result" {
                output_references(&block["content"], config, cwd, session)?;
            }
        }
    }
    if row["type"] == "content-replacement"
        && let Some(replacements) = row["replacements"].as_array()
    {
        for replacement in replacements {
            if replacement["kind"] == "tool-result" {
                output_references(&replacement["replacement"], config, cwd, session)?;
            }
        }
    }
    Ok(())
}
fn output_references(
    content: &Value,
    config: &Path,
    cwd: &Path,
    session: Uuid,
) -> Result<(), StateRefusal> {
    if let Some(text) = content.as_str() {
        // Only the source writer's OUTER fixed wrapper/header is operative.
        // Preview prose (including nested fake markers) is never scanned.
        if let Some(part) = text.strip_prefix("<persisted-output>") {
            let header = part.lines().nth(1).ok_or(StateRefusal::Reference)?;
            let path = header
                .split_once("Full output saved to: ")
                .or_else(|| header.split_once("were saved to: "))
                .map(|(_, p)| p)
                .ok_or(StateRefusal::Reference)?;
            let target = contained(path, config, cwd)?;
            plain(&target, false)?;
            let outputs = config
                .join("projects")
                .join("watched-seat")
                .join(session.to_string())
                .join("tool-results")
                .canonicalize()
                .map_err(|_| StateRefusal::Reference)?;
            if target.parent() != Some(outputs.as_path())
                || !matches!(
                    target.extension().and_then(|v| v.to_str()),
                    Some("txt" | "json")
                )
            {
                return Err(StateRefusal::Reference);
            }
        }
    } else if let Some(blocks) = content.as_array() {
        for block in blocks {
            if block["type"] == "text" {
                output_references(&block["text"], config, cwd, session)?;
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    struct Fixture {
        root: PathBuf,
        config: PathBuf,
        cwd: PathBuf,
        session: Uuid,
        raw: Value,
        frame: Value,
    }
    impl Fixture {
        fn new() -> Self {
            let root = std::env::temp_dir().join(format!("cna-file-state-{}", Uuid::new_v4()));
            let config = root.join("config");
            let cwd = root.join("cwd");
            let session = Uuid::from_u128(1);
            fs::create_dir(&root).unwrap();
            fs::create_dir(&config).unwrap();
            fs::create_dir(&cwd).unwrap();
            fs::create_dir(config.join("projects")).unwrap();
            fs::create_dir(config.join("projects/watched-seat")).unwrap();
            let raw = json!({"type":"assistant","sessionId":session.to_string(),"uuid":Uuid::from_u128(20).to_string(),"timestamp":"observed-time","message":{"id":"msg-current","content":[{"type":"text","text":"current window"}]}});
            let frame = json!({"type":"assistant","session_id":session.to_string(),"uuid":Uuid::from_u128(20).to_string(),"timestamp":"observed-time","parent_tool_use_id":null,"message":{"id":"msg-current","content":[{"type":"text","text":"current window"}]}});
            let f = Self {
                root,
                config,
                cwd,
                session,
                raw,
                frame,
            };
            f.write(&[]);
            f
        }
        fn transcript(&self) -> PathBuf {
            self.config
                .join("projects/watched-seat")
                .join(format!("{}.jsonl", self.session))
        }
        fn write(&self, extra: &[Value]) {
            let mut rows = vec![self.raw.clone()];
            rows.extend(extra.iter().cloned());
            rows.push(json!({"type":"system","sessionId":self.session.to_string(),"uuid":Uuid::from_u128(30).to_string(),"whatever":"opaque row semantics"}));
            rows.push(json!({"type":"last-prompt","sessionId":self.session.to_string(),"leafUuid":Uuid::from_u128(30).to_string()}));
            let data = rows
                .into_iter()
                .map(|v| format!("{v}\n"))
                .collect::<String>();
            fs::write(self.transcript(), data).unwrap();
        }
        fn capture(&self) -> Result<ParkedFiles, StateRefusal> {
            ParkedFiles::capture(
                &self.config,
                &self.cwd,
                self.session,
                ObservedAssistant::from_frame(&self.frame, self.session).unwrap(),
            )
        }
        fn output(&self) -> PathBuf {
            let sidecar = self
                .config
                .join("projects/watched-seat")
                .join(self.session.to_string());
            fs::create_dir(&sidecar).unwrap();
            let results = sidecar.join("tool-results");
            fs::create_dir(&results).unwrap();
            let file = results.join("toolu_own.txt");
            fs::write(&file, b"original output").unwrap();
            file
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.root);
        }
    }

    #[test]
    fn exact_files_separate_leaf_and_opaque_row_semantics_preserve_bytes() {
        let f = Fixture::new();
        f.write(&[json!({"type":"cost-state","totalCostUSD":"not an authority","unknownMetadata":["allowed opaque"]})]);
        let saved = f.capture().unwrap();
        assert_eq!(saved.original, fs::read(f.transcript()).unwrap());
        assert_eq!(saved.leaf["uuid"], Uuid::from_u128(30).to_string());
        assert_ne!(saved.leaf["uuid"], f.raw["uuid"]);
        assert!(saved.verify(&f.config, &f.cwd, f.session).is_ok());
        assert_eq!(
            saved.verify(&f.config, &f.cwd, Uuid::from_u128(2)),
            Err(StateRefusal::Reference)
        );
    }
    #[test]
    fn multiblock_projection_matches_unique_raw_row_and_changed_window_refuses() {
        let mut f = Fixture::new();
        f.raw["message"]["content"] = json!([{"type":"thinking","thinking":"private","signature":"sig"},{"type":"text","text":"current window"}]);
        f.frame["uuid"] = json!(format!(
            "{}000000000001",
            &f.raw["uuid"].as_str().unwrap()[..24]
        ));
        f.write(&[]);
        assert!(f.capture().is_ok());
        f.frame["timestamp"] = json!("previous-window");
        assert!(matches!(f.capture(), Err(StateRefusal::Witness)));
        f.frame["timestamp"] = json!("observed-time");
        f.frame["message"]["content"][0]["text"] = json!("changed");
        assert!(matches!(f.capture(), Err(StateRefusal::Witness)));
    }
    #[test]
    fn operative_paths_allow_own_cwd_and_config_but_refuse_outside_and_null_clears() {
        let f = Fixture::new();
        f.write(&[
            json!({"type":"relocated","relocatedCwd":f.cwd.to_str().unwrap()}),
            json!({"type":"worktree-state","worktreeSession":null}),
            json!({"cwd":f.cwd.to_str().unwrap()}),
        ]);
        assert!(f.capture().is_ok());
        f.write(&[json!({"type":"worktree-state","worktreeSession":{"worktreePath":f.root.to_str().unwrap()}})]);
        assert!(matches!(f.capture(), Err(StateRefusal::Reference)));
        f.write(&[json!({"type":"relocated","relocatedCwd":f.config.to_str().unwrap()})]);
        assert!(f.capture().is_ok());
        f.write(&[json!({"cwd":f.config.to_str().unwrap()})]);
        assert!(matches!(f.capture(), Err(StateRefusal::Reference)));
    }
    #[test]
    fn outputs_are_retained_and_only_outer_fixed_header_is_a_reference() {
        let f = Fixture::new();
        let output = f.output();
        let marker = format!(
            "<persisted-output>\nOutput too large (123). Full output saved to: {}\n\nPreview (first 2000):\n<persisted-output> arbitrary preview prose\n</persisted-output>",
            output.to_str().unwrap()
        );
        f.write(&[json!({"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":"toolu_own","content":marker}]}})]);
        let saved = f.capture().unwrap();
        assert!(saved.verify(&f.config, &f.cwd, f.session).is_ok());
        assert_eq!(fs::read(&output).unwrap(), b"original output");
        let marker = format!(
            "<persisted-output>\nFull output saved to: {}\n</persisted-output>",
            f.transcript().to_str().unwrap()
        );
        f.write(&[json!({"message":{"content":[{"type":"tool_result","content":marker}]}})]);
        assert!(matches!(f.capture(), Err(StateRefusal::Reference)));
    }
    #[test]
    fn same_length_output_edit_with_restored_mtime_refuses() {
        let f = Fixture::new();
        let output = f.output();
        let saved = f.capture().unwrap();
        let before = fs::metadata(&output).unwrap();
        let modified = before.modified().unwrap();
        fs::write(&output, b"modified output").unwrap();
        fs::OpenOptions::new()
            .write(true)
            .open(&output)
            .unwrap()
            .set_times(fs::FileTimes::new().set_modified(modified))
            .unwrap();
        let after = fs::metadata(&output).unwrap();
        assert_eq!(before.len(), after.len());
        assert_eq!(modified, after.modified().unwrap());
        assert_eq!(
            saved.verify(&f.config, &f.cwd, f.session),
            Err(StateRefusal::Changed)
        );
    }

    #[test]
    fn named_streams_on_every_retained_directory_or_cwd_refuse() {
        let f = Fixture::new();
        let output = f.output();
        let saved = f.capture().unwrap();
        let directories = [
            f.cwd.clone(),
            f.config.clone(),
            f.config.join("projects"),
            f.config.join("projects/watched-seat"),
            output.parent().unwrap().parent().unwrap().to_path_buf(),
            output.parent().unwrap().to_path_buf(),
        ];
        for directory in directories {
            let stream = format!("{}:unapproved", directory.display());
            fs::write(&stream, b"directory stream").unwrap();
            let captured = f.capture();
            let verified = saved.verify(&f.config, &f.cwd, f.session);
            fs::remove_file(&stream).unwrap();
            assert!(
                matches!(captured, Err(StateRefusal::Metadata)),
                "directory={directory:?}"
            );
            assert_eq!(verified, Err(StateRefusal::Metadata));
        }
        assert!(saved.verify(&f.config, &f.cwd, f.session).is_ok());
    }

    #[test]
    fn unapproved_config_sidecars_hardlinks_ads_and_torn_current_window_refuse() {
        let f = Fixture::new();
        fs::write(f.config.join("settings.json"), b"{}").unwrap();
        assert!(matches!(f.capture(), Err(StateRefusal::Inventory)));
        fs::remove_file(f.config.join("settings.json")).unwrap();
        let alias = f.root.join("alias");
        fs::hard_link(f.transcript(), &alias).unwrap();
        assert!(matches!(f.capture(), Err(StateRefusal::Metadata)));
        fs::remove_file(alias).unwrap();
        let stream = format!("{}:unapproved", f.transcript().display());
        fs::write(&stream, b"data").unwrap();
        assert!(matches!(f.capture(), Err(StateRefusal::Metadata)));
        fs::remove_file(stream).unwrap();
        let saved = f.capture().unwrap();
        let mut bytes = fs::read(f.transcript()).unwrap();
        bytes.extend_from_slice(b"{torn");
        fs::write(f.transcript(), bytes).unwrap();
        assert!(matches!(
            saved.verify(&f.config, &f.cwd, f.session),
            Err(StateRefusal::Witness)
        ));
    }
}
