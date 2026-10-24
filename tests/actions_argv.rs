//! Task 9: the action argv, checked against the per-action allowlist.

use gtasks::actions::{Action, MergeMethod, RepoSlug, allowed_flags, argv_for, emitted_flags};
use gtasks::binaries::Binaries;
use gtasks::config::validate_repo_slug;

fn slug(value: &str) -> RepoSlug {
    RepoSlug::parse(value).expect("the test slug is valid")
}

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
            repo: slug("acme/widget"),
            number: 12,
            method: MergeMethod::Squash,
            delete_branch: false,
        },
        Action::Merge {
            repo: slug("acme/widget"),
            number: 12,
            method: MergeMethod::Rebase,
            delete_branch: true,
        },
        Action::Comment {
            repo: slug("acme/widget"),
            number: 7,
            body: "looks good".into(),
        },
        Action::Close {
            repo: slug("acme/widget"),
            number: 9,
            reason: Some("not planned".into()),
            comment: Some("closing".into()),
        },
        Action::Checkout {
            repo: slug("acme/widget"),
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
        repo: slug("acme/widget"),
        number: 1,
        body: "-admin --force".into(),
    };
    let out = texts(&action);

    let body_at = out
        .iter()
        .position(|t| t == "--body")
        .expect("--body is emitted");
    assert_eq!(out[body_at + 1], "-admin --force", "the body is one value");
    // Only `--body`: `--repo` now comes from the type, not from the action.
    assert_eq!(flags(&action), vec!["--body"], "the body is not a flag");
}

#[test]
fn a_merge_emits_exactly_one_method_flag() {
    for method in [MergeMethod::Merge, MergeMethod::Squash, MergeMethod::Rebase] {
        let action = Action::Merge {
            repo: slug("acme/widget"),
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
        repo: slug("acme/widget"),
        number: 5,
        method: MergeMethod::Squash,
        delete_branch: false,
    };
    let with = Action::Merge {
        repo: slug("acme/widget"),
        number: 5,
        method: MergeMethod::Squash,
        delete_branch: true,
    };
    assert!(!flags(&base).contains(&"--delete-branch".to_string()));
    assert!(flags(&with).contains(&"--delete-branch".to_string()));
}

#[test]
fn an_invalid_repo_cannot_even_be_constructed() {
    // The type is the door: an invalid slug never becomes an `Action`.
    for bad in ["", "../x", "a b/c", "a/b/c", "-x/y"] {
        assert!(
            RepoSlug::parse(bad).is_err(),
            "{bad} must be refused by the type"
        );
        assert!(validate_repo_slug(bad).is_err(), "{bad} must be invalid");
    }
}

#[test]
fn a_nul_in_a_body_is_refused() {
    let action = Action::Comment {
        repo: slug("acme/widget"),
        number: 1,
        body: "ok\0rm -rf".into(),
    };
    let err = argv_for(&binaries(), &action).unwrap_err();
    assert!(err.to_string().contains("NUL"), "got {err}");
}

#[test]
fn the_number_is_a_positional_value_not_a_flag() {
    let action = Action::Checkout {
        repo: slug("acme/widget"),
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

/// A-1: the action lists and the transport's deny-list disagreed about
/// `--delete-branch`, so a merge that deletes the branch panicked in `gh()`.
/// The allowlist is the authority: no emitted flag may be forbidden.
#[test]
fn no_action_emits_a_flag_the_transport_forbids() {
    for action in every_action() {
        for flag in emitted_flags(&action).unwrap() {
            assert!(
                !gtasks::gh::is_forbidden_flag(flag),
                "{:?} emits {flag}, which the transport refuses",
                action
            );
        }
    }
}

/// A-1, the call the test above replaces: building the argv must not panic.
#[test]
fn a_merge_that_deletes_the_branch_builds_without_panicking() {
    let action = Action::Merge {
        repo: slug("acme/widget"),
        number: 1,
        method: MergeMethod::Squash,
        delete_branch: true,
    };
    let argv = argv_for(&binaries(), &action).expect("no panic, no error");
    let rendered: Vec<String> = argv
        .args
        .iter()
        .map(|a| a.to_string_lossy().to_string())
        .collect();
    assert!(
        rendered.contains(&"--delete-branch".to_string()),
        "got {rendered:?}"
    );
}

/// A-2: a value that starts with a dash must survive the transport.
#[test]
fn a_comment_body_starting_with_a_dash_survives_the_transport() {
    let action = Action::Comment {
        repo: slug("acme/widget"),
        number: 1,
        body: "-done: looks good".into(),
    };
    let argv = argv_for(&binaries(), &action).expect("builds");
    // The transport check is the one that used to reject it.
    gtasks::gh::check_argv(&argv).expect("a value is not a flag");
}

/// The decision made with the auditor: `--repo` is not an action flag, it is
/// produced by the constructor from the type.
#[test]
fn a_repo_flag_cannot_appear_in_an_action_list() {
    for action in every_action() {
        assert!(
            !allowed_flags(&action).contains(&"--repo"),
            "--repo must not be allowlisted for {:?}",
            action
        );
    }
}

#[test]
fn the_action_prefix_is_built_from_the_type() {
    let action = Action::Checkout {
        repo: slug("acme/widget"),
        number: 7,
    };
    let argv = argv_for(&binaries(), &action).unwrap();
    let rendered: Vec<String> = argv
        .args
        .iter()
        .map(|a| a.to_string_lossy().to_string())
        .collect();

    assert_eq!(
        &rendered[..2],
        &["--repo".to_string(), "acme/widget".to_string()]
    );
    assert!(
        !emitted_flags(&action).unwrap().contains(&"--repo"),
        "the prefix is not an action token"
    );
}
