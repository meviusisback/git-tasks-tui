//! Shared scratch dir: canonicalised, so the state dir's symlink refusal does
//! not trip over the scratch root itself. macOS `$TMPDIR` lives under `/var`,
//! which is a symlink to `/private/var` — without this, every private write
//! under a plain `tempdir()` fails there before the test even starts.
//!
//! The `TempDir` guard is returned alongside the path: dropping it deletes the
//! directory, so callers must hold it (`let (_tmp, dir) = ...`).
use std::path::PathBuf;

pub fn scratch() -> (tempfile::TempDir, PathBuf) {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().canonicalize().unwrap();
    (tmp, dir)
}
