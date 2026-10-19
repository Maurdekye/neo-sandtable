//! Watched-run first-start guard. The calibration guard remains unchanged.
//! Resume is deliberately refused until its bounded persistence allowlist exists.

use super::{
    DIRECT_INSTRUCTIONS, ISOLATION_ROOT, IsolationError, Kind, LaunchError, MAX_ANCESTORS,
    ManagedMetadata, Metadata, StdManaged, StdMetadata, check_managed_routes,
};
use cna_core::ids::SeatId;
use std::{
    ffi::OsString,
    path::{Path, PathBuf},
};

/// The launcher chooses one fresh UUID. Seat names come only from canonical IDs.
pub struct WatchedNamespace {
    root: PathBuf,
    floor: PathBuf,
    run: uuid::Uuid,
}

impl WatchedNamespace {
    pub fn new(run: uuid::Uuid) -> Self {
        Self {
            root: PathBuf::from(ISOLATION_ROOT),
            floor: PathBuf::from("C:/"),
            run,
        }
    }

    pub fn run_dir(&self) -> PathBuf {
        self.root.join(self.run.to_string())
    }

    pub fn cwd(&self, seat: SeatId) -> PathBuf {
        self.run_dir().join(seat.to_string()).join("seat-cwd")
    }

    pub fn config(&self, seat: SeatId) -> PathBuf {
        self.run_dir().join(seat.to_string()).join("claude-config")
    }

    /// No directory creation or credential access. Checks run again immediately
    /// before the callback. This metadata guard does not prevent same-user races.
    pub fn with_checked_initial_spawn<T, E>(
        &self,
        seat: SeatId,
        resume: bool,
        spawn: impl FnOnce(&Path, &Path) -> Result<T, E>,
    ) -> Result<T, LaunchError<E>> {
        if !cfg!(windows) || resume {
            return Err(IsolationError::Layout.into());
        }
        self.checked_launch(&StdManaged, &StdMetadata, seat, spawn)
    }

    fn checked_launch<T, E>(
        &self,
        managed: &impl ManagedMetadata,
        metadata: &impl Metadata,
        seat: SeatId,
        spawn: impl FnOnce(&Path, &Path) -> Result<T, E>,
    ) -> Result<T, LaunchError<E>> {
        check_managed_routes(managed)?;
        self.checked_spawn(metadata, seat, spawn)
    }

    fn checked_spawn<T, E>(
        &self,
        metadata: &impl Metadata,
        seat: SeatId,
        spawn: impl FnOnce(&Path, &Path) -> Result<T, E>,
    ) -> Result<T, LaunchError<E>> {
        self.check(metadata, seat)?;
        spawn(&self.cwd(seat), &self.config(seat)).map_err(LaunchError::Spawn)
    }

    fn check(&self, metadata: &impl Metadata, seat: SeatId) -> Result<(), IsolationError> {
        let cwd = self.cwd(seat);
        if !self.root.is_absolute()
            || !self.floor.is_absolute()
            || !self.root.starts_with(&self.floor)
        {
            return Err(IsolationError::Layout);
        }
        let mut reached_floor = false;
        for (depth, ancestor) in cwd.ancestors().enumerate() {
            if depth == MAX_ANCESTORS || !ancestor.starts_with(&self.floor) {
                return Err(IsolationError::Layout);
            }
            if metadata.kind(ancestor)? != Kind::Directory {
                return Err(IsolationError::Metadata);
            }
            for name in DIRECT_INSTRUCTIONS.iter().copied().chain([".claude"]) {
                if metadata.kind(&ancestor.join(name))? != Kind::Absent {
                    return Err(IsolationError::Instructions);
                }
            }
            if ancestor == self.floor {
                reached_floor = true;
                break;
            }
        }
        if !reached_floor {
            return Err(IsolationError::Layout);
        }
        exact_directories(metadata, &self.root, &[self.run.to_string().into()])?;
        let seats: Vec<_> = SeatId::all()
            .map(|s| OsString::from(s.to_string()))
            .collect();
        exact_directories(metadata, &self.run_dir(), &seats)?;
        for other in SeatId::all() {
            let directory = self.run_dir().join(other.to_string());
            exact_directories(
                metadata,
                &directory,
                &["seat-cwd".into(), "claude-config".into()],
            )?;
            if !metadata.entries(&self.cwd(other))?.is_empty() {
                return Err(IsolationError::NotEmpty);
            }
        }
        // Other seats may already be running. Their config contents are not an
        // input to this seat's CLI. This first start accepts only an empty own
        // config; every same-seat restart is refused by the public entry above.
        if !metadata.entries(&self.config(seat))?.is_empty() {
            return Err(IsolationError::NotEmpty);
        }
        Ok(())
    }
}

fn exact_directories(
    metadata: &impl Metadata,
    directory: &Path,
    expected: &[OsString],
) -> Result<(), IsolationError> {
    if metadata.kind(directory)? != Kind::Directory {
        return Err(IsolationError::Metadata);
    }
    let entries = metadata.entries(directory)?;
    if entries.len() != expected.len() {
        return Err(IsolationError::RootEntry);
    }
    let mut seen = vec![false; expected.len()];
    for (name, kind) in entries {
        let Some(index) = expected.iter().position(|value| *value == name) else {
            return Err(IsolationError::RootEntry);
        };
        if kind != Kind::Directory
            || seen[index]
            || metadata.kind(&directory.join(&name))? != Kind::Directory
        {
            return Err(IsolationError::RootEntry);
        }
        seen[index] = true;
    }
    if !seen.into_iter().all(|value| value) {
        return Err(IsolationError::RootEntry);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{cell::Cell, collections::BTreeMap};

    struct Synthetic {
        kinds: BTreeMap<PathBuf, Kind>,
        entries: BTreeMap<PathBuf, Vec<(OsString, Kind)>>,
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
            Ok(self.entries.get(path).cloned().unwrap_or_default())
        }
    }
    fn fixture() -> (WatchedNamespace, Synthetic, SeatId) {
        let floor = std::env::current_dir().unwrap().join("notional-drive");
        let layout = WatchedNamespace {
            root: floor.join("CNA-Seat-Isolation"),
            floor,
            run: uuid::Uuid::from_u128(1),
        };
        let seat = SeatId::all().next().unwrap();
        let mut metadata = Synthetic {
            kinds: BTreeMap::new(),
            entries: BTreeMap::new(),
            unavailable: None,
        };
        for ancestor in layout.cwd(seat).ancestors() {
            metadata.kinds.insert(ancestor.into(), Kind::Directory);
            if ancestor == layout.floor {
                break;
            }
        }
        metadata.entries.insert(
            layout.root.clone(),
            vec![(layout.run.to_string().into(), Kind::Directory)],
        );
        metadata.entries.insert(
            layout.run_dir(),
            SeatId::all()
                .map(|s| (s.to_string().into(), Kind::Directory))
                .collect(),
        );
        for other in SeatId::all() {
            let directory = layout.run_dir().join(other.to_string());
            metadata.kinds.insert(directory.clone(), Kind::Directory);
            metadata.entries.insert(
                directory,
                vec![
                    ("seat-cwd".into(), Kind::Directory),
                    ("claude-config".into(), Kind::Directory),
                ],
            );
            metadata.kinds.insert(layout.cwd(other), Kind::Directory);
            metadata.kinds.insert(layout.config(other), Kind::Directory);
        }
        (layout, metadata, seat)
    }
    fn refused(layout: &WatchedNamespace, metadata: &Synthetic, seat: SeatId) {
        let calls = Cell::new(0);
        assert!(
            layout
                .checked_spawn(metadata, seat, |_, _| {
                    calls.set(calls.get() + 1);
                    Ok::<_, ()>(())
                })
                .is_err()
        );
        assert_eq!(calls.get(), 0);
    }

    #[test]
    fn exact_namespace_allows_first_start_and_rechecks_every_callback() {
        let (layout, mut metadata, seat) = fixture();
        layout
            .checked_spawn(&metadata, seat, |cwd, config| {
                assert_eq!(cwd, layout.cwd(seat));
                assert_eq!(config, layout.config(seat));
                Ok::<_, ()>(())
            })
            .unwrap();
        metadata
            .kinds
            .insert(layout.floor.join("CLAUDE.md"), Kind::Other);
        refused(&layout, &metadata, seat);
    }

    #[test]
    fn managed_presence_and_unknown_routes_refuse_in_same_watched_pipeline() {
        struct Managed(Result<bool, IsolationError>);
        impl ManagedMetadata for Managed {
            fn absent(&self, _: super::super::ManagedRoute) -> Result<bool, IsolationError> {
                self.0
            }
        }
        let (layout, metadata, seat) = fixture();
        for state in [Ok(false), Err(IsolationError::Managed)] {
            let calls = Cell::new(0);
            assert!(
                layout
                    .checked_launch(&Managed(state), &metadata, seat, |_, _| {
                        calls.set(calls.get() + 1);
                        Ok::<_, ()>(())
                    })
                    .is_err()
            );
            assert_eq!(calls.get(), 0);
        }
        layout
            .checked_launch(&Managed(Ok(true)), &metadata, seat, |_, _| Ok::<_, ()>(()))
            .unwrap();
    }

    #[test]
    fn sibling_runs_seats_and_arbitrary_pair_entries_refuse() {
        for level in 0..3 {
            for name in ["arbitrary", "extra-seat-cwd", ".hidden"] {
                let (layout, mut metadata, seat) = fixture();
                let directory = match level {
                    0 => layout.root.clone(),
                    1 => layout.run_dir(),
                    _ => layout.run_dir().join(seat.to_string()),
                };
                metadata
                    .entries
                    .get_mut(&directory)
                    .unwrap()
                    .push((name.into(), Kind::Directory));
                refused(&layout, &metadata, seat);
            }
        }
    }

    #[test]
    fn all_ancestors_instruction_sources_reparse_and_unknown_metadata_refuse() {
        let (layout, _, seat) = fixture();
        for ancestor in layout.cwd(seat).ancestors() {
            for name in DIRECT_INSTRUCTIONS.iter().copied().chain([".claude"]) {
                let (_, mut metadata, _) = fixture();
                metadata.kinds.insert(ancestor.join(name), Kind::Other);
                refused(&layout, &metadata, seat);
            }
            let (_, mut metadata, _) = fixture();
            metadata.kinds.insert(ancestor.into(), Kind::Reparse);
            refused(&layout, &metadata, seat);
            metadata.kinds.insert(ancestor.into(), Kind::Directory);
            metadata.unavailable = Some(ancestor.into());
            refused(&layout, &metadata, seat);
            if ancestor == layout.floor {
                break;
            }
        }
    }

    #[test]
    fn config_links_and_dirty_cwd_refuse_and_resume_has_no_callback() {
        let (layout, mut metadata, seat) = fixture();
        metadata.kinds.insert(layout.config(seat), Kind::Reparse);
        refused(&layout, &metadata, seat);
        let (layout, mut metadata, seat) = fixture();
        metadata
            .entries
            .insert(layout.cwd(seat), vec![("file".into(), Kind::Other)]);
        refused(&layout, &metadata, seat);
        let (layout, mut metadata, seat) = fixture();
        metadata.entries.insert(
            layout.config(seat),
            vec![("session.jsonl".into(), Kind::Other)],
        );
        refused(&layout, &metadata, seat);
        assert!(
            layout
                .with_checked_initial_spawn(seat, true, |_, _| -> Result<(), ()> {
                    panic!("resume must refuse before callback or managed helper")
                })
                .is_err()
        );
    }
}
