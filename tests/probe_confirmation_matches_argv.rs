//! Probe 1/4 for the execution block — **the confirmation must show the argv it
//! will run, and the sentence must name every effect the argv carries**.
//!
//! Runs against what is already on the tree: `argv_for` (the source of truth) and
//! `confirmation_for` (what the user reads). No network: the program is `/bin/true`.
//!
//! A failure means one of two things, both bugs:
//!   * the shown argv differs from the argv that will run — the user approves
//!     something other than what happens;
//!   * the sentence hides an effect the argv has (`--comment`, `--delete-branch`)
//!     — the user reads "close PR 12" while a comment is posted.

use std::path::PathBuf;

use gtasks::actions::{argv_for, Action, MergeMethod, RepoSlug};
use gtasks::actions_ui::confirmation_for;
use gtasks::binaries::Binaries;

fn binaries() -> Binaries {
    Binaries::new(PathBuf::from("/usr/bin/gh"), PathBuf::from("/bin/true"))
}

fn slug() -> RepoSlug {
    RepoSlug::parse("acme/widget").expect("valid slug")
}

/// What the sentence may say to name a given effect, lower-cased.
fn spellings(flag: &str) -> &'static [&'static str] {
    match flag {
        "--merge" => &["merge"],
        "--squash" => &["squash"],
        "--rebase" => &["rebase"],
        "--delete-branch" => &["delete"],
        "--body" => &["comment"],
        "--reason" => &["reason"],
        "--comment" => &["comment"],
        _ => &[],
    }
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
fn every_effect_of_the_argv_is_named_by_the_sentence() {
    let cases: Vec<Action> = vec![
        Action::Merge {
            repo: slug(),
            number: 7,
            method: MergeMethod::Squash,
            delete_branch: true,
        },
        Action::Merge {
            repo: slug(),
            number: 11,
            method: MergeMethod::Rebase,
            delete_branch: false,
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
            comment: Some("closing; do not reopen".to_string()),
        },
        Action::Checkout { repo: slug(), number: 10 },
    ];

    for action in cases {
        let argv = argv_for(&binaries(), &action).expect("the action builds");
        let shown = confirmation_for(&action, argv.as_argv());

        // 1) the shown argv is the argv that will run, program included
        let real: Vec<String> = std::iter::once(argv.as_argv().program.display().to_string())
            .chain(
                argv.as_argv()
                    .args
                    .iter()
                    .map(|a| a.to_string_lossy().into_owned()),
            )
            .collect();
        assert_eq!(
            shown.argv, real,
            "the confirmation shows a different argv from the one that will run: {action:?}"
        );

        // 2) every effect the argv carries is named by the sentence
        for flag in &real {
            let words = spellings(flag);
            if words.is_empty() {
                continue; // not an effect, or not one this probe knows
            }
            assert!(
                words
                    .iter()
                    .any(|w| shown.summary.to_lowercase().contains(w)),
                "the sentence hides an effect of the argv ({flag}): summary={:?} argv={:?}",
                shown.summary,
                real
            );
        }

        // 3) the sentence says which action: number and slug
        assert!(
            shown.summary.contains(&format!("#{}", number_of(&action))),
            "the sentence does not name the PR number: {:?}",
            shown.summary
        );
        assert!(
            shown.summary.contains("acme/widget"),
            "the sentence does not name the repository: {:?}",
            shown.summary
        );
    }
}
