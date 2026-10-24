//! The auditor's end-to-end probes, vendored: the guarantee must hold for
//! callers of the public seam (`State::load`), not only for the internal helper.

use gtasks::state::State;

#[test]
fn a_symlinked_state_file_is_refused_through_the_public_seam() {
    let dir = tempfile::tempdir().unwrap();
    let real = dir.path().join("real.json");
    std::fs::write(&real, "{\"selected_repos\":[\"a/b\"]}").unwrap();
    let link = dir.path().join("state.json");
    std::os::unix::fs::symlink(&real, &link).unwrap();

    let err = State::load(&link).unwrap_err();
    let text = format!("{err:#}");
    assert!(
        text.contains("symlink") || text.contains("Too many levels"),
        "the open itself must fail, got: {text}"
    );
}

#[test]
fn a_regular_state_file_loads() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("state.json");
    std::fs::write(&path, "{\"selected_repos\":[\"a/b\"]}").unwrap();

    let state = State::load(&path).expect("a regular file loads");
    assert_eq!(state.selected_repos, vec!["a/b".to_string()]);
    assert_eq!(state.sort_key, "updated", "serde default applies");
    assert!(!state.ascending);
}

#[test]
fn a_missing_file_is_the_default_state() {
    let dir = tempfile::tempdir().unwrap();
    let state = State::load(&dir.path().join("absent.json")).expect("missing file is fine");
    assert!(state.selected_repos.is_empty());
}
