//! Calibration-only metadata guard, checked immediately before each spawn callback.
//! It reads no instruction contents, credentials or settings. It is not an OS
//! isolation boundary or a native SDK inventory proof. The caller owns cleanup.

use std::ffi::{OsStr, OsString};
use std::fs;
use std::path::{Path, PathBuf};

pub const ISOLATION_ROOT: &str = "C:/CNA-Seat-Isolation";
pub const CALIBRATION_CWD: &str = "C:/CNA-Seat-Isolation/calibration-one-20261008-seat-cwd";
pub const CALIBRATION_CONFIG: &str = "C:/CNA-Seat-Isolation/calibration-one-20261008-claude-config";

// Installed CLI 2.1.293 engine Fwt and agents-md plugin J/D. Rules are a
// directory source with recursive imports, so ANY presence refuses.
const DIRECT_INSTRUCTIONS: &[&str] = &["AGENTS.md", "CLAUDE.md", "CLAUDE.local.md"];
const MAX_ROOT_ENTRIES: usize = 128;
const MAX_ANCESTORS: usize = 128;

/// Fixed categories never expose discovered names, paths or file contents.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum IsolationError {
    #[error("unsupported calibration isolation layout")]
    Layout,
    #[error("isolation metadata unavailable or ambiguous")]
    Metadata,
    #[error("ancestor instruction source exists")]
    Instructions,
    #[error("shared isolation root contains an unauthorized entry")]
    RootEntry,
    #[error("calibration directory is not empty")]
    NotEmpty,
    #[error("managed instruction route is present or unverified")]
    Managed,
}

/// A callback failure stays distinct from a prelaunch refusal.
#[derive(Debug, thiserror::Error)]
pub enum LaunchError<E> {
    #[error("calibration prelaunch refused: {0}")]
    Isolation(#[from] IsolationError),
    #[error("calibration spawn callback failed")]
    Spawn(E),
}

/// Check the fixed, flattened Windows calibration locations, then immediately
/// invoke the caller's spawn callback with those locations. There is no cached
/// receipt or unchecked public layout. Every invocation repeats every check.
/// This does not start a driver by itself, authorize a credential or eliminate
/// same-user mutations after the metadata check. Existing drivers are unchanged.
pub fn with_checked_calibration_spawn<T, E>(
    spawn: impl FnOnce(&Path, &Path) -> Result<T, E>,
) -> Result<T, LaunchError<E>> {
    if !cfg!(windows) {
        return Err(IsolationError::Layout.into());
    }
    let layout = Layout {
        root: PathBuf::from(ISOLATION_ROOT),
        cwd: PathBuf::from(CALIBRATION_CWD),
        config: PathBuf::from(CALIBRATION_CONFIG),
        floor: PathBuf::from("C:/"),
    };
    checked_launch(&StdManaged, &StdMetadata, &layout, spawn)
}

#[derive(Clone, Copy, Debug)]
enum ManagedRoute {
    ProgramFiles,
    MachineClaudeCode,
    MachineAnthropic,
    UserClaudeCode,
    UserAnthropic,
    LegacyProgramData,
}
const MANAGED_ROUTES: [ManagedRoute; 6] = [
    ManagedRoute::ProgramFiles,
    ManagedRoute::MachineClaudeCode,
    ManagedRoute::MachineAnthropic,
    ManagedRoute::UserClaudeCode,
    ManagedRoute::UserAnthropic,
    ManagedRoute::LegacyProgramData,
];
trait ManagedMetadata {
    // Only a positively established absence permits launch. Any denied,
    // inaccessible, present or otherwise unknown route must refuse.
    fn absent(&self, route: ManagedRoute) -> Result<bool, IsolationError>;
}
fn check_managed_routes(metadata: &impl ManagedMetadata) -> Result<(), IsolationError> {
    for route in MANAGED_ROUTES {
        let outcome = metadata.absent(route);
        #[cfg(test)]
        receipt_route(route, &outcome);
        if !outcome? {
            return Err(IsolationError::Managed);
        }
    }
    Ok(())
}
struct StdManaged;
impl ManagedMetadata for StdManaged {
    fn absent(&self, route: ManagedRoute) -> Result<bool, IsolationError> {
        match route {
            ManagedRoute::ProgramFiles => {
                Ok(StdMetadata.kind(Path::new("C:/Program Files/ClaudeCode"))? == Kind::Absent)
            }
            ManagedRoute::LegacyProgramData => {
                Ok(StdMetadata.kind(Path::new("C:/ProgramData/ClaudeCode"))? == Kind::Absent)
            }
            ManagedRoute::MachineClaudeCode => registry_absent("LocalMachine", "ClaudeCode"),
            ManagedRoute::MachineAnthropic => registry_absent("LocalMachine", "Anthropic"),
            ManagedRoute::UserClaudeCode => registry_absent("CurrentUser", "ClaudeCode"),
            ManagedRoute::UserAnthropic => registry_absent("CurrentUser", "Anthropic"),
        }
    }
}

// Generic shell success/failure does not establish a metadata result. Only
// our explicit completed-script statuses permit absence/presence conclusions.
#[cfg(any(windows, test))]
fn registry_verdict(code: Option<i32>) -> Result<bool, IsolationError> {
    match code {
        Some(10) => Ok(true),
        Some(11) => Ok(false),
        _ => Err(IsolationError::Managed), // COULD-NOT-DETERMINE; always refuses.
    }
}

#[cfg(not(windows))]
fn registry_absent(_: &str, _: &str) -> Result<bool, IsolationError> {
    Err(IsolationError::Managed)
}

#[cfg(windows)]
fn registry_absent(hive: &str, key: &str) -> Result<bool, IsolationError> {
    use std::os::windows::process::CommandExt;
    use std::process::{Command, Stdio};
    use std::time::{Duration, Instant};
    // No query of policy values or contents. Open read-only by documented name
    // in BOTH views; a present key refuses irrespective of whether it has values.
    // The helper emits nothing and inherits no credentials/profile/operator env.
    let hive = match hive {
        "LocalMachine" => "LocalMachine",
        "CurrentUser" => "CurrentUser",
        _ => return Err(IsolationError::Managed),
    };
    let key = match key {
        "ClaudeCode" => "ClaudeCode",
        // Additional conservative refusal, NOT a demonstrated loader route.
        "Anthropic" => "Anthropic",
        _ => return Err(IsolationError::Managed),
    };
    let script = format!(
        r#"$ErrorActionPreference='Stop'; try {{
      foreach ($view in @([Microsoft.Win32.RegistryView]::Registry64,[Microsoft.Win32.RegistryView]::Registry32)) {{
        $base=$null; $key=$null;
        try {{
          $base=[Microsoft.Win32.RegistryKey]::OpenBaseKey([Microsoft.Win32.RegistryHive]::{hive},$view);
          $key=$base.OpenSubKey('SOFTWARE\Policies\{key}',$false);
          if ($null -ne $key) {{ exit 11 }}
        }} finally {{ if ($null -ne $key) {{ $key.Close() }}; if ($null -ne $base) {{ $base.Close() }} }}
      }}; exit 10
    }} catch {{ exit 12 }}"#
    );
    let mut command = Command::new("C:/Windows/System32/WindowsPowerShell/v1.0/powershell.exe");
    command
        .args([
            "-NoProfile",
            "-NoLogo",
            "-NonInteractive",
            "-Command",
            &script,
        ])
        .env_clear()
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .creation_flags(0x0800_0000);
    for name in ["SystemRoot", "WINDIR", "SystemDrive", "TEMP", "TMP"] {
        if let Some(value) = std::env::var_os(name) {
            command.env(name, value);
        }
    }
    // Only the explicitly selected inert witness captures complete helper pipes.
    // Default tests never launch this helper; production suppresses its output.
    #[cfg(test)]
    capture_helper_output(&mut command, hive, key)?;
    let mut child = command.spawn().map_err(|_| IsolationError::Managed)?;
    #[cfg(test)]
    let pid = child.id();
    let started = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                #[cfg(test)]
                receipt_helper(hive, key, pid, status.code(), false, true);
                return registry_verdict(status.code());
            }
            Ok(None) if started.elapsed() < Duration::from_secs(5) => {
                std::thread::sleep(Duration::from_millis(10))
            }
            _ => {
                // A timeout/failed status never becomes an absence receipt.
                // Always request termination AND join before returning refusal.
                let killed = child.kill();
                let joined = child.wait();
                #[cfg(test)]
                receipt_helper(
                    hive,
                    key,
                    pid,
                    joined.as_ref().ok().and_then(|s| s.code()),
                    true,
                    joined.is_ok(),
                );
                // A failed termination request never upgrades the outcome.
                let _confirmed_cleanup = killed.is_ok() && joined.is_ok();
                return Err(IsolationError::Managed);
            }
        }
    }
}

struct Layout {
    root: PathBuf,
    cwd: PathBuf,
    config: PathBuf,
    // Only tests may replace the actual drive root with an owned fixture root.
    floor: PathBuf,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    Absent,
    Directory,
    Other,
    Reparse,
}

trait Metadata {
    fn kind(&self, path: &Path) -> Result<Kind, IsolationError>;
    fn entries(&self, path: &Path) -> Result<Vec<(OsString, Kind)>, IsolationError>;
}

struct StdMetadata;
impl Metadata for StdMetadata {
    fn kind(&self, path: &Path) -> Result<Kind, IsolationError> {
        let metadata = match fs::symlink_metadata(path) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Kind::Absent),
            Err(_) => return Err(IsolationError::Metadata),
        };
        #[cfg(windows)]
        {
            use std::os::windows::fs::MetadataExt;
            // Includes junctions and other reparse points, not only symlinks.
            if metadata.file_attributes() & 0x400 != 0 {
                return Ok(Kind::Reparse);
            }
        }
        Ok(if metadata.file_type().is_symlink() {
            Kind::Reparse
        } else if metadata.is_dir() {
            Kind::Directory
        } else {
            Kind::Other
        })
    }

    fn entries(&self, path: &Path) -> Result<Vec<(OsString, Kind)>, IsolationError> {
        let mut entries = Vec::new();
        for entry in fs::read_dir(path).map_err(|_| IsolationError::Metadata)? {
            if entries.len() == MAX_ROOT_ENTRIES {
                return Err(IsolationError::Metadata);
            }
            let entry = entry.map_err(|_| IsolationError::Metadata)?;
            entries.push((entry.file_name(), self.kind(&entry.path())?));
        }
        Ok(entries)
    }
}

// The public entry and the authorized inert witness share this exact pipeline.
// Test substitution changes only the owned filesystem floor and callback.
fn checked_launch<T, E>(
    managed: &impl ManagedMetadata,
    metadata: &impl Metadata,
    layout: &Layout,
    spawn: impl FnOnce(&Path, &Path) -> Result<T, E>,
) -> Result<T, LaunchError<E>> {
    check_managed_routes(managed)?;
    checked_spawn(metadata, layout, spawn)
}

#[cfg(test)]
thread_local! {
    static MANAGED_RECEIPT: std::cell::RefCell<Option<(PathBuf, Vec<serde_json::Value>)>> = const {
        std::cell::RefCell::new(None)
    };
}
#[cfg(test)]
fn receipt_route(route: ManagedRoute, outcome: &Result<bool, IsolationError>) {
    MANAGED_RECEIPT.with(|receipt| {
        if let Some((_, entries)) = receipt.borrow_mut().as_mut() {
            let state = match outcome {
                Ok(true) => "ABSENT",
                Ok(false) => "PRESENT",
                Err(_) => "COULD-NOT-DETERMINE",
            };
            entries.push(serde_json::json!({"route":format!("{route:?}"),"outcome":state}));
        }
    });
}
#[cfg(all(test, windows))]
fn capture_helper_output(
    command: &mut std::process::Command,
    hive: &str,
    key: &str,
) -> Result<(), IsolationError> {
    MANAGED_RECEIPT.with(|receipt| {
        if let Some((directory, _)) = receipt.borrow().as_ref() {
            for (channel, stdout) in [("stdout", true), ("stderr", false)] {
                let file = fs::OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .open(directory.join(format!("{hive}-{key}-{channel}.raw")))
                    .map_err(|_| IsolationError::Managed)?;
                if stdout {
                    command.stdout(file);
                } else {
                    command.stderr(file);
                }
            }
        }
        Ok(())
    })
}
#[cfg(all(test, windows))]
fn receipt_helper(
    hive: &str,
    key: &str,
    pid: u32,
    exit_code: Option<i32>,
    timed_out: bool,
    joined: bool,
) {
    MANAGED_RECEIPT.with(|receipt| {
        if let Some((_, entries)) = receipt.borrow_mut().as_mut() {
            entries.push(serde_json::json!({"helper_hive":hive,"key":key,"pid":pid,
                "exit_code":exit_code,"timeout_or_status_error":timed_out,"joined":joined}));
        }
    });
}

fn checked_spawn<T, E>(
    metadata: &impl Metadata,
    layout: &Layout,
    spawn: impl FnOnce(&Path, &Path) -> Result<T, E>,
) -> Result<T, LaunchError<E>> {
    check(metadata, layout)?;
    spawn(&layout.cwd, &layout.config).map_err(LaunchError::Spawn)
}

fn check(metadata: &impl Metadata, layout: &Layout) -> Result<(), IsolationError> {
    if !layout.cwd.is_absolute()
        || !layout.config.is_absolute()
        || layout.cwd == layout.config
        || layout.cwd.parent() != Some(layout.root.as_path())
        || layout.config.parent() != Some(layout.root.as_path())
        || !layout.root.starts_with(&layout.floor)
        || !directory_name(layout.cwd.file_name().ok_or(IsolationError::Layout)?)
        || !directory_name(layout.config.file_name().ok_or(IsolationError::Layout)?)
    {
        return Err(IsolationError::Layout);
    }
    // Walk the physical spelling without canonicalizing/following links. Each
    // ancestor and intermediate .claude directory must be an ordinary directory.
    let mut reached_floor = false;
    for (depth, ancestor) in layout.cwd.ancestors().enumerate() {
        if depth == MAX_ANCESTORS || !ancestor.starts_with(&layout.floor) {
            return Err(IsolationError::Layout);
        }
        if metadata.kind(ancestor)? != Kind::Directory {
            return Err(IsolationError::Metadata);
        }
        for name in DIRECT_INSTRUCTIONS {
            if metadata.kind(&ancestor.join(name))? != Kind::Absent {
                return Err(IsolationError::Instructions);
            }
        }
        // ANY .claude presence refuses, even an empty directory or harmless
        // settings/skills. Never inspect nested instruction/config contents.
        if metadata.kind(&ancestor.join(".claude"))? != Kind::Absent {
            return Err(IsolationError::Instructions);
        }
        if ancestor == layout.floor {
            reached_floor = true;
            break;
        }
    }
    if !reached_floor {
        return Err(IsolationError::Layout);
    }
    let root_entries = metadata.entries(&layout.root)?;
    let expected = [
        Path::new(CALIBRATION_CWD)
            .file_name()
            .ok_or(IsolationError::Layout)?,
        Path::new(CALIBRATION_CONFIG)
            .file_name()
            .ok_or(IsolationError::Layout)?,
    ];
    if root_entries.len() != expected.len() {
        return Err(IsolationError::RootEntry);
    }
    let mut seen = [false; 2];
    for (name, kind) in root_entries {
        // EQUAL exactly the two named plain directories, not a generic suffix
        // whitelist. No extra valid-name directory or files of ANY name.
        let Some(index) = expected.iter().position(|expected| name == *expected) else {
            return Err(IsolationError::RootEntry);
        };
        if kind != Kind::Directory || seen[index] {
            return Err(IsolationError::RootEntry);
        }
        seen[index] = true;
    }
    if !seen.into_iter().all(|seen| seen) {
        return Err(IsolationError::RootEntry);
    }
    for directory in [&layout.cwd, &layout.config] {
        if metadata.kind(directory)? != Kind::Directory {
            return Err(IsolationError::Metadata);
        }
        if !metadata.entries(directory)?.is_empty() {
            return Err(IsolationError::NotEmpty);
        }
    }
    Ok(())
}

fn directory_name(name: &OsStr) -> bool {
    let Some(name) = name.to_str() else {
        return false;
    };
    let stem = name
        .strip_suffix("-seat-cwd")
        .or_else(|| name.strip_suffix("-claude-config"));
    stem.is_some_and(|stem| {
        !stem.is_empty()
            && stem.len() <= 80
            && stem
                .bytes()
                .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;
    use std::collections::BTreeMap;

    struct Synthetic {
        kinds: BTreeMap<PathBuf, Kind>,
        listing: BTreeMap<PathBuf, Vec<(OsString, Kind)>>,
        unavailable: Option<PathBuf>,
    }
    impl Metadata for Synthetic {
        fn kind(&self, path: &Path) -> Result<Kind, IsolationError> {
            if self.unavailable.as_deref() == Some(path) {
                return Err(IsolationError::Metadata);
            }
            Ok(*self.kinds.get(path).unwrap_or(&Kind::Absent))
        }
        fn entries(&self, path: &Path) -> Result<Vec<(OsString, Kind)>, IsolationError> {
            if self.unavailable.as_deref() == Some(path) {
                return Err(IsolationError::Metadata);
            }
            Ok(self.listing.get(path).cloned().unwrap_or_default())
        }
    }
    fn fixture() -> (Synthetic, Layout) {
        // This is only a notional drive root; no actual root access or mutation.
        let floor = std::env::current_dir().unwrap().join("notional-drive");
        let root = floor.join("shared").join("isolation");
        let cwd = root.join(Path::new(CALIBRATION_CWD).file_name().unwrap());
        let config = root.join(Path::new(CALIBRATION_CONFIG).file_name().unwrap());
        let layout = Layout {
            root,
            cwd,
            config,
            floor,
        };
        let mut kinds = BTreeMap::new();
        for path in layout.cwd.ancestors() {
            kinds.insert(path.to_path_buf(), Kind::Directory);
            if path == layout.floor {
                break;
            }
        }
        kinds.insert(layout.config.clone(), Kind::Directory);
        let listing = BTreeMap::from([(
            layout.root.clone(),
            vec![
                (
                    Path::new(CALIBRATION_CWD)
                        .file_name()
                        .unwrap()
                        .to_os_string(),
                    Kind::Directory,
                ),
                (
                    Path::new(CALIBRATION_CONFIG)
                        .file_name()
                        .unwrap()
                        .to_os_string(),
                    Kind::Directory,
                ),
            ],
        )]);
        (
            Synthetic {
                kinds,
                listing,
                unavailable: None,
            },
            layout,
        )
    }
    fn refuses_before_callback(metadata: &impl Metadata, layout: &Layout) {
        let called = Cell::new(false);
        let result = checked_spawn(metadata, layout, |_, _| {
            called.set(true);
            Ok::<_, ()>(())
        });
        assert!(result.is_err());
        assert!(!called.get());
    }

    #[test]
    fn both_instruction_names_at_parent_two_up_and_notional_root_refuse() {
        for name in ["AGENTS.md", "CLAUDE.md"] {
            for depth in [1, 2, 3] {
                let (mut metadata, layout) = fixture();
                let parent = layout.cwd.ancestors().nth(depth).unwrap();
                metadata.kinds.insert(parent.join(name), Kind::Other);
                refuses_before_callback(&metadata, &layout);
            }
        }
    }

    #[test]
    fn every_declared_variant_at_every_depth_refuses_without_contents() {
        for depth in 0..=3 {
            for name in DIRECT_INSTRUCTIONS {
                let (mut metadata, layout) = fixture();
                let parent = layout.cwd.ancestors().nth(depth).unwrap();
                metadata.kinds.insert(parent.join(name), Kind::Other);
                refuses_before_callback(&metadata, &layout);
            }
            for name in ["AGENTS.md", "CLAUDE.md", "rules"] {
                let (mut metadata, layout) = fixture();
                let nested = layout.cwd.ancestors().nth(depth).unwrap().join(".claude");
                metadata.kinds.insert(nested.clone(), Kind::Directory);
                metadata.kinds.insert(nested.join(name), Kind::Other);
                refuses_before_callback(&metadata, &layout);
            }
        }
    }

    #[test]
    fn arbitrary_and_hidden_root_files_material_directory_and_links_refuse() {
        for (name, kind) in [
            ("harmless.txt", Kind::Other),
            (".hidden", Kind::Other),
            ("receipts", Kind::Directory),
            ("another-seat-cwd", Kind::Reparse),
            ("another-claude-config", Kind::Other),
        ] {
            let (mut metadata, layout) = fixture();
            metadata
                .listing
                .get_mut(&layout.root)
                .unwrap()
                .push((name.into(), kind));
            refuses_before_callback(&metadata, &layout);
        }
    }

    #[test]
    fn ancestor_or_nested_link_and_unavailable_metadata_fail_closed() {
        for depth in 0..=3 {
            let (mut metadata, layout) = fixture();
            let parent = layout.cwd.ancestors().nth(depth).unwrap().to_path_buf();
            metadata.kinds.insert(parent.clone(), Kind::Reparse);
            refuses_before_callback(&metadata, &layout);
            metadata.kinds.insert(parent.clone(), Kind::Directory);
            metadata.kinds.insert(parent.join(".claude"), Kind::Reparse);
            refuses_before_callback(&metadata, &layout);
        }
        let (mut metadata, layout) = fixture();
        metadata.unavailable = Some(layout.floor.join("CLAUDE.md"));
        refuses_before_callback(&metadata, &layout);
        metadata.unavailable = Some(layout.root.clone());
        refuses_before_callback(&metadata, &layout);
    }

    #[test]
    fn both_directories_empty_and_distinct_and_every_spawn_rechecks() {
        let (mut metadata, mut layout) = fixture();
        let called = Cell::new(0);
        let spawn = |_: &Path, _: &Path| {
            called.set(called.get() + 1);
            Ok::<_, ()>(())
        };
        checked_spawn(&metadata, &layout, spawn).unwrap();
        assert_eq!(called.get(), 1);
        metadata
            .kinds
            .insert(layout.floor.join("AGENTS.md"), Kind::Other);
        assert!(checked_spawn(&metadata, &layout, spawn).is_err());
        assert_eq!(called.get(), 1);
        metadata.kinds.remove(&layout.floor.join("AGENTS.md"));
        for dir in [&layout.cwd, &layout.config] {
            metadata
                .listing
                .insert(dir.clone(), vec![("arbitrary".into(), Kind::Other)]);
            refuses_before_callback(&metadata, &layout);
            metadata.listing.remove(dir);
        }
        layout.config = layout.cwd.clone();
        refuses_before_callback(&metadata, &layout);
    }

    #[test]
    fn bounded_root_listing_and_misplaced_configuration_refuse() {
        let (mut metadata, mut layout) = fixture();
        metadata.listing.insert(
            layout.root.clone(),
            (0..=MAX_ROOT_ENTRIES)
                .map(|index| (format!("seat-{index}-seat-cwd").into(), Kind::Directory))
                .collect(),
        );
        refuses_before_callback(&metadata, &layout);
        let (metadata, _) = fixture();
        layout.config = layout.cwd.join("nested-claude-config");
        refuses_before_callback(&metadata, &layout);
    }

    #[test]
    fn each_managed_route_present_denied_or_unknown_refuses_before_callback() {
        struct Routes {
            outcomes: [Result<bool, IsolationError>; 6],
            calls: Cell<usize>,
        }
        impl ManagedMetadata for Routes {
            fn absent(&self, route: ManagedRoute) -> Result<bool, IsolationError> {
                self.calls.set(self.calls.get() + 1);
                self.outcomes[route as usize]
            }
        }
        for index in 0..6 {
            for refusal in [
                Ok(false),
                Err(IsolationError::Metadata),
                Err(IsolationError::Managed),
            ] {
                let mut outcomes = [Ok(true); 6];
                outcomes[index] = refusal;
                let metadata = Routes {
                    outcomes,
                    calls: Cell::new(0),
                };
                let called = Cell::new(false);
                let (filesystem, layout) = fixture();
                let result = checked_launch(&metadata, &filesystem, &layout, |_, _| {
                    called.set(true);
                    Ok::<_, ()>(())
                });
                assert!(result.is_err());
                assert!(!called.get());
                assert_eq!(metadata.calls.get(), index + 1);
            }
        }
        let metadata = Routes {
            outcomes: [Ok(true); 6],
            calls: Cell::new(0),
        };
        let (filesystem, layout) = fixture();
        checked_launch(&metadata, &filesystem, &layout, |_, _| Ok::<_, ()>(())).unwrap();
        checked_launch(&metadata, &filesystem, &layout, |_, _| Ok::<_, ()>(())).unwrap();
        assert_eq!(metadata.calls.get(), 12); // No reusable/cached audit receipt.
    }

    #[test]
    fn failed_spawn_is_not_a_cleanup_or_inventory_receipt() {
        let (metadata, layout) = fixture();
        assert!(matches!(
            checked_spawn(&metadata, &layout, |_, _| Err::<(), _>("failure")),
            Err(LaunchError::Spawn(_))
        ));
    }

    struct OwnedFixture(PathBuf);
    impl Drop for OwnedFixture {
        fn drop(&mut self) {
            fs::remove_dir_all(&self.0).unwrap();
        }
    }
    #[test]
    fn actual_owned_metadata_refuses_parent_two_up_and_notional_root_before_spawn() {
        let workspace = std::env::current_dir().unwrap();
        let floor = workspace.join(format!("isolation-fixture-{}", uuid::Uuid::new_v4()));
        assert!(floor.starts_with(&workspace));
        let _cleanup = OwnedFixture(floor.clone());
        let root = floor.join("shared").join("isolation");
        let cwd = root.join(Path::new(CALIBRATION_CWD).file_name().unwrap());
        let config = root.join(Path::new(CALIBRATION_CONFIG).file_name().unwrap());
        fs::create_dir_all(&cwd).unwrap();
        fs::create_dir(&config).unwrap();
        let layout = Layout {
            root,
            cwd,
            config,
            floor,
        };
        checked_spawn(&StdMetadata, &layout, |_, _| Ok::<_, ()>(())).unwrap();
        for depth in [1, 2, 3] {
            for name in ["AGENTS.md", "CLAUDE.md"] {
                let path = layout.cwd.ancestors().nth(depth).unwrap().join(name);
                fs::write(&path, []).unwrap();
                refuses_before_callback(&StdMetadata, &layout);
                fs::remove_file(path).unwrap();
            }
        }
        let unexpected = layout.root.join(".unexpected");
        fs::write(&unexpected, []).unwrap();
        refuses_before_callback(&StdMetadata, &layout);
        fs::remove_file(unexpected).unwrap();
        checked_spawn(&StdMetadata, &layout, |_, _| Ok::<_, ()>(())).unwrap();
    }

    /// No CLI/model/native SDK is started: callback only flips a Cell.
    /// Run ONCE explicitly with an own-scratch receipt directory. Default and
    /// broad ignored suites remain inert unless this independent opt-in is set.
    #[cfg(windows)]
    #[test]
    #[ignore = "opt-in: actual read-only managed metadata path, never CLI/provider"]
    fn actual_managed_pipeline_with_inert_callback() {
        use sha2::{Digest, Sha256};
        let Some(directory) = std::env::var_os("CNA_MANAGED_GUARD_RECEIPT_DIR") else {
            return;
        };
        let directory = PathBuf::from(directory);
        assert!(directory.is_absolute());
        assert!(fs::read_dir(&directory).unwrap().next().is_none());
        let workspace = std::env::current_dir().unwrap();
        // Actual source areas and all real drive ancestors stay untouched.
        assert!(!directory.starts_with(&workspace));
        let floor = workspace.join(format!("managed-fixture-{}", uuid::Uuid::new_v4()));
        assert!(floor.starts_with(&workspace));
        let cleanup = OwnedFixture(floor.clone());
        let root = floor.join("shared").join("isolation");
        let cwd = root.join(Path::new(CALIBRATION_CWD).file_name().unwrap());
        let config = root.join(Path::new(CALIBRATION_CONFIG).file_name().unwrap());
        fs::create_dir_all(&cwd).unwrap();
        fs::create_dir(&config).unwrap();
        let layout = Layout {
            root,
            cwd,
            config,
            floor,
        };
        MANAGED_RECEIPT
            .with(|receipt| *receipt.borrow_mut() = Some((directory.clone(), Vec::new())));
        let called = Cell::new(false);
        let result = checked_launch(&StdManaged, &StdMetadata, &layout, |_, _| {
            called.set(true);
            Ok::<_, ()>(())
        });
        let entries = MANAGED_RECEIPT.with(|receipt| receipt.borrow_mut().take().unwrap().1);
        let result_category = match &result {
            Ok(()) => "CALLBACK_REACHED",
            Err(LaunchError::Isolation(IsolationError::Managed)) => "MANAGED_REFUSED",
            Err(LaunchError::Isolation(_)) => "FILESYSTEM_OR_ACCESS_REFUSED",
            Err(LaunchError::Spawn(())) => "CALLBACK_FAILED",
        };
        // Read every complete helper pipe after native process close; never echo
        // original bytes. These credential-free metadata children are not SDKs.
        let mut raw = Vec::new();
        for entry in fs::read_dir(&directory).unwrap() {
            let path = entry.unwrap().path();
            let bytes = fs::read(&path).unwrap();
            raw.push(
                serde_json::json!({"file":path.file_name().unwrap().to_string_lossy(),
                "bytes":bytes.len(),"sha256":format!("{:X}", Sha256::digest(&bytes))}),
            );
        }
        drop(cleanup);
        let receipt = serde_json::json!({
            "result":result_category,"callback_invoked":called.get(),
            "metadata":entries,"raw":raw,"fixture_removed":!layout.floor.exists(),
            "isolation_root_accessed":false,"cli_sdk_provider_started":false,
            "filesystem_floor":"OWNED_SYNTHETIC_ANCESTORS",
            "output_capture":"FILES_CONNECTED_BEFORE_HELPER_SPAWN_AND_READ_AFTER_CLOSE",
            "limit":"same-user point-in-time metadata, not instruction delivery/inventory proof"
        });
        fs::write(
            directory.join("result.json"),
            serde_json::to_vec_pretty(&receipt).unwrap(),
        )
        .unwrap();
        assert_eq!(called.get(), result.is_ok());
        assert!(!layout.floor.exists());
        assert!(
            entries
                .iter()
                .filter(|entry| entry.get("helper_hive").is_some())
                .all(|entry| entry["joined"] == true)
        );
        if called.get() {
            assert_eq!(
                entries
                    .iter()
                    .filter(|entry| entry.get("route").is_some())
                    .count(),
                6
            );
            assert!(
                entries
                    .iter()
                    .filter(|entry| entry.get("route").is_some())
                    .all(|entry| entry["outcome"] == "ABSENT")
            );
        }
    }

    #[test]
    fn third_valid_leaf_missing_duplicate_or_wrong_kind_root_entry_refuses() {
        let (mut metadata, layout) = fixture();
        let original = metadata.listing.get(&layout.root).unwrap().clone();
        metadata
            .listing
            .get_mut(&layout.root)
            .unwrap()
            .push(("third-seat-cwd".into(), Kind::Directory));
        refuses_before_callback(&metadata, &layout);
        metadata
            .listing
            .insert(layout.root.clone(), vec![original[0].clone()]);
        refuses_before_callback(&metadata, &layout);
        metadata.listing.insert(
            layout.root.clone(),
            vec![original[0].clone(), original[0].clone()],
        );
        refuses_before_callback(&metadata, &layout);
        let mut wrong_kind = original;
        wrong_kind[1].1 = Kind::Reparse;
        metadata.listing.insert(layout.root.clone(), wrong_kind);
        refuses_before_callback(&metadata, &layout);
    }

    #[test]
    fn any_ancestor_dot_claude_including_harmless_settings_and_skills_refuses() {
        for depth in 0..=3 {
            for kind in [Kind::Directory, Kind::Other, Kind::Reparse] {
                let (mut metadata, layout) = fixture();
                let path = layout.cwd.ancestors().nth(depth).unwrap().join(".claude");
                metadata.kinds.insert(path.clone(), kind);
                // Synthetic only: no actual sensitive directory or settings write.
                metadata.listing.insert(
                    path.clone(),
                    vec![
                        ("settings.json".into(), Kind::Other),
                        ("skills".into(), Kind::Directory),
                    ],
                );
                metadata
                    .kinds
                    .insert(path.join("settings.json"), Kind::Other);
                metadata.kinds.insert(path.join("skills"), Kind::Directory);
                refuses_before_callback(&metadata, &layout);
            }
        }
        let (mut metadata, layout) = fixture();
        metadata
            .kinds
            .insert(layout.floor.join(".claude"), Kind::Directory);
        metadata
            .listing
            .insert(layout.floor.join(".claude"), Vec::new());
        refuses_before_callback(&metadata, &layout); // Empty directory also refuses.
    }

    #[test]
    fn only_distinctive_completed_registry_codes_are_metadata_verdicts() {
        assert_eq!(registry_verdict(Some(10)), Ok(true));
        assert_eq!(registry_verdict(Some(11)), Ok(false));
        for code in [
            None,
            Some(0),
            Some(1),
            Some(2),
            Some(12),
            Some(-1),
            Some(42),
            Some(255),
            Some(i32::MIN),
            Some(i32::MAX),
        ] {
            assert_eq!(
                registry_verdict(code),
                Err(IsolationError::Managed),
                "COULD-NOT-DETERMINE must REFUSE, never generic shell success/presence"
            );
        }
    }
}
