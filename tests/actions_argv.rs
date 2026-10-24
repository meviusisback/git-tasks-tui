//! Task 9: the action argv, checked against the per-action allowlist.

use gtasks::actions::{Action, MergeMethod, allowed_flags, argv_for, emitted_flags};
use gtasks::binaries::Binaries;
use gtasks::config::validate_repo_slug;

fn binaries() -> Binaries {
    Binaries::new(
        std::path::PathBuf::from("/bin/true"),
        std::path::PathBuf::from("/bin/true"),
    )
}

fn texts(action: &Action) -> Vec<String> {
    argv_for(&binaries(), action)
        .expect("action builds")
        .args
        .iter()
        .map(|a| a.to_string_lossy().to_string())
        .collect()
}

fn flags(action: &Action) -> Vec<String> {
    emitted_flags(action)
        .expect("action builds")
        .into_iter()
        .map(str::to_string)
        .collect()
}

fn every_action() -> Vec<Action> {
    vec![
        Action::Merge {
            repo: "acme/widget".into(),
            number: 12,
            method: MergeMethod::Squash,
            delete_branch: false,
        },
        Action::Merge {
            repo: "acme/widget".into(),
            number: 12,
            method: MergeMethod::Rebase,
            delete_branch: true,
        },
        Action::Comment {
            repo: "acme/widget".into(),
            number: 7,
            body: "looks good".into(),
        },
        Action::Close {
            repo: "acme/widget".into(),
            number: 9,
            reason: Some("not planned".into()),
            comment: Some("closing".into()),
        },
        Action::Checkout {
            repo: "acme/widget".into(),
            number: 3,
        },
    ]
}

#[test]
fn every_action_argv_is_allowlisted() {
    for action in every_action() {
        let allowed = allowed_flags(&action);
        for flag in flags(&action) {
            assert!(
                allowed.contains(&flag.as_str()),
                "{:?} emitted {flag}, allowed: {allowed:?}",
                action
            );
        }
    }
}

#[test]
fn a_comment_body_that_starts_with_a_dash_stays_a_value() {
    let action = Action::Comment {
        repo: "acme/widget".into(),
        number: 1,
        body: "-admin --force".into(),
    };
    let out = texts(&action);

    let body_at = out
        .iter()
        .position(|t| t == "--body")
        .expect("--body is emitted");
    assert_eq!(out[body_at + 1], "-admin --force", "the body is one value");
    // And it is not counted as a flag: only --repo and --body are.
    assert_eq!(
        flags(&action),
        vec!["--repo", "--body"],
        "the body is not a flag"
    );
}

#[test]
fn a_merge_emits_exactly_one_method_flag() {
    for method in [MergeMethod::Merge, MergeMethod::Squash, MergeMethod::Rebase] {
        let action = Action::Merge {
            repo: "acme/widget".into(),
            number: 5,
            method,
            delete_branch: false,
        };
        let methods = flags(&action)
            .into_iter()
            .filter(|f| ["--merge", "--squash", "--rebase"].contains(&f.as_str()))
            .count();
        assert_eq!(methods, 1, "{method:?} must emit exactly one strategy");
    }
}

#[test]
fn delete_branch_is_opt_in() {
    let base = Action::Merge {
        repo: "acme/widget".into(),
        number: 5,
        method: MergeMethod::Squash,
        delete_branch: false,
    };
    let with = Action::Merge {
        repo: "acme/widget".into(),
        number: 5,
        method: MergeMethod::Squash,
        delete_branch: true,
    };
    assert!(!flags(&base).contains(&"--delete-branch".to_string()));
    assert!(flags(&with).contains(&"--delete-branch".to_string()));
}

#[test]
fn an_invalid_repo_is_refused_before_any_argv_exists() {
    for bad in ["", "../x", "a b/c", "a/b/c", "-x/y"] {
        assert!(validate_repo_slug(bad).is_err(), "{bad} must be invalid");
        let action = Action::Checkout {
            repo: bad.to_string(),
            number: 1,
        };
        assert!(
            argv_for(&binaries(), &action).is_err(),
            "{bad} must be refused"
        );
    }
}

#[test]
fn a_nul_in_a_body_is_refused() {
    let action = Action::Comment {
        repo: "acme/widget".into(),
        number: 1,
        body: "ok\0rm -rf".into(),
    };
    let err = argv_for(&binaries(), &action).unwrap_err();
    assert!(err.to_string().contains("NUL"), "got {err}");
}

#[test]
fn the_number_is_a_positional_value_not_a_flag() {
    let action = Action::Checkout {
        repo: "acme/widget".into(),
        number: 42,
    };
    let argv = argv_for(&binaries(), &action).unwrap();
    let rendered: Vec<String> = argv
        .args
        .iter()
        .map(|a| a.to_string_lossy().to_string())
        .collect();
    assert!(rendered.contains(&"checkout".to_string()));
    assert!(rendered.contains(&"42".to_string()));
    assert!(
        !rendered.iter().any(|t| t == "--42"),
        "the number is not a flag"
    );
}
