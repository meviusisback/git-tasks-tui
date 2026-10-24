//! Probe 1b — **the argv shown in the confirmation is the argv the process gets**.
//!
//! The weaker version compares `confirmation_for()` with `argv_for()`, which only
//! proves that one function copies a vector. This one runs a **fake gh** that
//! records the arguments it actually received, and compares the recording with
//! what the user was shown — program included, order included, nothing normalised.
//!
//! A failure means the user approves one command and a different one runs.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

use gtasks::actions::{argv_for, Action, MergeMethod, RepoSlug};
use gtasks::actions_ui::{confirmation_for, execute};
use gtasks::binaries::Binaries;

/// A fake `gh` for one case: writes the arguments it receives to `record`.
fn fake_gh(dir: &Path, case: u64) -> (PathBuf, PathBuf) {
    let program = dir.join(format!("fake-gh-{case}"));
    let record = dir.join(format!("rec-{case}"));
    fs::write(
        &program,
        format!(
            "#!/bin/sh\nfor a in \"$@\"; do printf '%s\\n' \"$a\"; done > {}\n",
            record.display()
        ),
    )
    .unwrap();
    use std::os::unix::fs::PermissionsExt;
    let mut perms = fs::metadata(&program).unwrap().permissions();
    perms.set_mode(0o755);
    fs::set_permissions(&program, perms).unwrap();
    (program, record)
}

fn slug() -> RepoSlug {
    RepoSlug::parse("acme/widget").expect("valid slug")
}

fn number_of(action: &Action) -> u64 {
    match action {
        Action::Merge { number, .. }
        | Action::Comment { number, .. }
        | Action::Close { number, .. }
        | Action::Checkout { number, .. } => *number,
    }
}

#[test]
fn the_shown_argv_is_the_argv_the_process_receives() {
    let dir = std::env::temp_dir().join(format!("gtt-argv-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();

    let cases = vec![
        Action::Merge {
            repo: slug(),
            number: 7,
            method: MergeMethod::Squash,
            delete_branch: true,
        },
        Action::Comment {
            repo: slug(),
            number: 8,
            body: "-done: looks good".to_string(),
        },
        Action::Close {
            repo: slug(),
            number: 9,
            reason: Some("not planned".to_string()),
            comment: None,
        },
        Action::Checkout {
            repo: slug(),
            number: 10,
        },
    ];

    for action in cases {
        let case = number_of(&action);
        let (program, record) = fake_gh(&dir, case);
        let binaries = Binaries::new(program.clone(), PathBuf::from("/bin/true"));

        let checked = argv_for(&binaries, &action).expect("the action builds");
        let shown = confirmation_for(&action, checked.as_argv());

        // Guard against a test that proves nothing: the program is the fake one.
        assert_eq!(
            shown.argv.first().map(String::as_str),
            Some(program.display().to_string().as_str()),
            "the confirmation does not show the program that will run"
        );

        execute(&checked, Duration::from_secs(10));

        let received: Vec<String> = fs::read_to_string(&record)
            .unwrap_or_default()
            .lines()
            .map(str::to_string)
            .collect();

        assert_eq!(
            received,
            shown.argv[1..].to_vec(),
            "the process received a different argv from the one shown to the user: {action:?}"
        );
        assert!(
            !received.is_empty(),
            "the fake program recorded nothing — the probe would be vacuous"
        );
    }

    let _ = fs::remove_dir_all(&dir);
}
