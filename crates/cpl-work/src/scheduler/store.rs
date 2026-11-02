//! Coordinator state on disk (`spec/toolchain.md` 10.1, T33): one JSON
//! document per change run at `<project root>/.cpl/work/<change run
//! id>.json`. The state is disposable execution state; durable results are
//! git objects. A restart reloads the documents and voids every lease whose
//! `expires_at` has passed, leaving worktrees in place for inspection or
//! reassignment ([`Coordinator::restart`]).

use super::run::{ChangeRun, ScheduleError};
use crate::model::{ChangeRunId, TaskId, Timestamp};
use std::collections::BTreeMap;
use std::io;
use std::path::{Path, PathBuf};

/// The change runs of one project.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Coordinator {
    /// The project root.
    pub root: PathBuf,
    pub runs: BTreeMap<ChangeRunId, ChangeRun>,
}

/// Why coordinator state could not be read or written.
#[derive(thiserror::Error, Debug)]
pub enum CoordinatorError {
    #[error("{}: {source}", .path.display())]
    Io {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("{}: {source}", .path.display())]
    Json {
        path: PathBuf,
        #[source]
        source: serde_json::Error,
    },
    #[error("{}: the file holds change run {found}", .path.display())]
    Misnamed { path: PathBuf, found: String },
    #[error(transparent)]
    Schedule(#[from] ScheduleError),
}

impl Coordinator {
    /// No change runs.
    pub fn new(root: impl Into<PathBuf>) -> Coordinator {
        Coordinator {
            root: root.into(),
            runs: BTreeMap::new(),
        }
    }

    /// `<root>/.cpl/work`.
    pub fn state_dir(root: &Path) -> PathBuf {
        root.join(".cpl").join("work")
    }

    /// `<root>/.cpl/work/<id>.json`.
    pub fn state_file(&self, id: &ChangeRunId) -> PathBuf {
        Coordinator::state_dir(&self.root).join(format!("{id}.json"))
    }

    /// Read every `<change run id>.json` directly in the state directory
    /// (none when it does not exist). A file whose run id differs from its
    /// name is [`CoordinatorError::Misnamed`].
    pub fn load(root: &Path) -> Result<Coordinator, CoordinatorError> {
        let mut coordinator = Coordinator::new(root);
        let dir = Coordinator::state_dir(root);
        let entries = match std::fs::read_dir(&dir) {
            Ok(entries) => entries,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(coordinator),
            Err(source) => return Err(CoordinatorError::Io { path: dir, source }),
        };
        let mut paths = Vec::new();
        for entry in entries {
            let entry = entry.map_err(|source| CoordinatorError::Io {
                path: dir.clone(),
                source,
            })?;
            let path = entry.path();
            if path.extension().is_some_and(|ext| ext == "json") && path.is_file() {
                paths.push(path);
            }
        }
        paths.sort();
        for path in paths {
            let bytes = std::fs::read(&path).map_err(|source| CoordinatorError::Io {
                path: path.clone(),
                source,
            })?;
            let run: ChangeRun =
                serde_json::from_slice(&bytes).map_err(|source| CoordinatorError::Json {
                    path: path.clone(),
                    source,
                })?;
            let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("");
            if stem != run.change.id.as_str() {
                return Err(CoordinatorError::Misnamed {
                    path,
                    found: run.change.id.to_string(),
                });
            }
            coordinator.runs.insert(run.change.id.clone(), run);
        }
        Ok(coordinator)
    }

    /// A coordinator restart (Section 10.1): [`Coordinator::load`], then
    /// [`Coordinator::void_expired_leases`] at `now`. Returns the tasks whose
    /// leases were voided; the caller saves the state.
    pub fn restart(
        root: &Path,
        now: &Timestamp,
    ) -> Result<(Coordinator, Vec<TaskId>), CoordinatorError> {
        let mut coordinator = Coordinator::load(root)?;
        let voided = coordinator.void_expired_leases(now)?;
        Ok((coordinator, voided))
    }

    /// [`ChangeRun::expire_leases`] for every run that is not closed.
    pub fn void_expired_leases(&mut self, now: &Timestamp) -> Result<Vec<TaskId>, ScheduleError> {
        let mut voided = Vec::new();
        for run in self.runs.values_mut() {
            if run.change.state.is_terminal() {
                continue;
            }
            for name in run.expire_leases(now)? {
                voided.push(TaskId {
                    change: run.change.id.clone(),
                    name,
                });
            }
        }
        Ok(voided)
    }

    /// Add or replace a run.
    pub fn insert(&mut self, run: ChangeRun) -> Option<ChangeRun> {
        self.runs.insert(run.change.id.clone(), run)
    }

    /// Write one run's document: pretty-printed JSON with a final LF,
    /// written to `<id>.json.tmp` and renamed over `<id>.json`, so that a
    /// crash leaves the previous state rather than a partial file. Nothing
    /// is written for an id the coordinator does not hold.
    pub fn save_run(&self, id: &ChangeRunId) -> Result<(), CoordinatorError> {
        let Some(run) = self.runs.get(id) else {
            return Ok(());
        };
        let dir = Coordinator::state_dir(&self.root);
        std::fs::create_dir_all(&dir).map_err(|source| CoordinatorError::Io {
            path: dir.clone(),
            source,
        })?;
        let path = self.state_file(id);
        let mut text =
            serde_json::to_string_pretty(run).map_err(|source| CoordinatorError::Json {
                path: path.clone(),
                source,
            })?;
        text.push('\n');
        let temporary = path.with_extension("json.tmp");
        std::fs::write(&temporary, text).map_err(|source| CoordinatorError::Io {
            path: temporary.clone(),
            source,
        })?;
        std::fs::rename(&temporary, &path).map_err(|source| CoordinatorError::Io {
            path: path.clone(),
            source,
        })
    }

    /// [`Coordinator::save_run`] for every run.
    pub fn save(&self) -> Result<(), CoordinatorError> {
        for id in self.runs.keys() {
            self.save_run(id)?;
        }
        Ok(())
    }
}
