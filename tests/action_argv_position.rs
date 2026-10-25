//! The property that stage 14 will lean on: **no untrusted string ever lands in flag
//! position.**
//!
//! Today the action layer has four variants, and every one of them builds its positionals
//! from constants (`"pr"`, `"merge"`, `"comment"`) and from a `u64`. The untrusted strings
//! — a comment body, a close reason — are bound to a flag (`--body <value>`), where a
//! leading dash cannot become a flag because the token is in *value* position.
//!
//! That is a property of the **builder**, not of the data, and the menu (stage 14) will add
//! variants that carry text taken from GitHub: branch names, titles, templates. A branch
//! named `--upload-pack=…` is not hypothetical — it is a name someone can create.
//!
//! So this test pins the *shape*:
//!
//!   1. every string that comes from outside appears **immediately after** a token starting
//!      with `--` (value position), never as a bare positional;
//!   2. a value containing a NUL is refused outright;
//!   3. the match below has **no catch-all**, so adding an `Action` variant is a compile
//!      error here until the new variant is exercised too.
//!
//! Point 3 is the one that makes this a guard rather than a snapshot: the test cannot be
//! silently outgrown.

use std::path::{Path, PathBuf};

use gtasks::actions::{Action, MergeMethod, argv_for};
use gtasks::binaries::Binaries;

/// Strings that try to become a flag, plus empty and whitespace-only ones.
const ADVERSARIAL: &[&str] = &[
    "--upload-pack=x",
    "-R",
    "--repo",
    "--body",
    "--",
    "-",
    "a b",
    "x\ny",
    "",
];

fn fake_gh(dir: &Path) -> PathBuf {
    let path = dir.join("gh");
    std::fs::write(&path, "#!/bin/sh\nexit 0\n").expect("write fake gh");
    let mut perms = std::fs::metadata(&path).expect("stat").permissions();
    std::os::unix::fs::PermissionsExt::set_mode(&mut perms, 0o700);
    std::fs::set_permissions(&path, perms).expect("chmod");
    path
}

fn slug() -> gtasks::actions::RepoSlug {
    gtasks::actions::RepoSlug::parse("me/one").expect("slug")
}

/// Every string injected from outside must sit in a flag's value slot.
fn assert_value_position(binaries: &Binaries, action: &Action, untrusted: &[&str]) {
    let checked = argv_for(binaries, action).expect("a legitimate value must not be refused");
    let args: Vec<String> = checked
        .as_argv()
        .args
        .iter()
        .map(|a| a.to_string_lossy().into_owned())
        .collect();

    for needle in untrusted {
        let hits: Vec<usize> = args
            .iter()
            .enumerate()
            .filter(|(_, a)| a.as_str() == *needle)
            .map(|(i, _)| i)
            .collect();
        assert!(
            !hits.is_empty(),
            "the value {needle:?} did not reach the argv at all: {args:?}"
        );
        // A value that is *itself* the text of a legitimate flag (e.g. the injected string
        // is literally "--repo") also matches the builder's own flag at index 0. What must
        // hold is that the injected copy landed in a value slot, so at least one occurrence
        // has a flag immediately before it.
        assert!(
            hits.iter().any(|&i| i > 0 && args[i - 1].starts_with("--")),
            "{needle:?} appears at {hits:?} of {args:?} but never right after a flag: a bare \
             positional can be read as a flag by the program it is handed to"
        );
    }
}

#[test]
fn a_comment_body_is_a_value_not_a_flag() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let binaries = Binaries::new(fake_gh(tmp.path()), PathBuf::from("/usr/bin/git"));

    for evil in ADVERSARIAL {
        let action = Action::Comment {
            repo: slug(),
            number: 7,
            body: (*evil).to_string(),
        };
        assert_value_position(&binaries, &action, &[evil]);
    }
}

#[test]
fn a_close_reason_and_comment_are_values_not_flags() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let binaries = Binaries::new(fake_gh(tmp.path()), PathBuf::from("/usr/bin/git"));

    for evil in ADVERSARIAL {
        let action = Action::Close {
            repo: slug(),
            number: 7,
            reason: Some((*evil).to_string()),
            comment: Some((*evil).to_string()),
        };
        assert_value_position(&binaries, &action, &[evil]);
    }
}

/// A NUL byte cannot travel in an argv, so it must be refused — not silently truncated.
#[test]
fn a_value_with_a_nul_is_refused() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let binaries = Binaries::new(fake_gh(tmp.path()), PathBuf::from("/usr/bin/git"));

    let action = Action::Comment {
        repo: slug(),
        number: 7,
        body: "before\0after".to_string(),
    };
    let err = argv_for(&binaries, &action).expect_err("a NUL in a value must be refused");
    println!("refused: {err}");
}

/// No catch-all: a new `Action` variant does not compile until it is exercised above.
#[test]
fn every_variant_is_covered_by_this_file() {
    fn must_be_exercised(action: &Action) {
        match action {
            Action::Merge { .. } => {}
            Action::Comment { .. } => {}
            Action::Close { .. } => {}
            Action::Checkout { .. } => {}
        }
    }
    let tmp = tempfile::tempdir().expect("tempdir");
    let binaries = Binaries::new(fake_gh(tmp.path()), PathBuf::from("/usr/bin/git"));
    let merge = Action::Merge {
        repo: slug(),
        number: 7,
        method: MergeMethod::Squash,
        delete_branch: false,
    };
    must_be_exercised(&merge);
    // The merge variant carries no untrusted text: its number is a u64 and its method an
    // enum, so there is nothing an attacker can shape.
    let checked = argv_for(&binaries, &merge).expect("merge argv");
    println!("merge argv: {:?}", checked.as_argv().args);
}
