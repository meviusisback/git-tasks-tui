//! Local clones live in more than one directory.
//!
//! Measured on the machine this program runs on: `~/repo` holds 24 clones and `~/repos` holds 2,
//! and the config said so — `clone_roots = ["/home/alberto/repo", "/home/alberto/repos"]`. The
//! field was removed as "unread", which was true, and the config was migrated by deleting that
//! line, which left `clone_root` unset: every `g` in the interface then refuses with "no
//! clone_root configured", i.e. the agent feature is dead for the only person who uses it.
//!
//! These tests pin the shape the old key already had: a list, searched in order.

use std::path::PathBuf;

use gtasks::agent::{AgentChoice, AgentId, PlanInputs, plan_agent};

fn test_inputs<'a>(profile: &'a str, roots: &'a [std::path::PathBuf]) -> PlanInputs<'a> {
    PlanInputs {
        profile,
        roots,
        default_agent: AgentChoice {
            id: AgentId::Hermes,
            program: std::path::PathBuf::from("/bin/hermes"),
        },
    }
}
use gtasks::config::Config;
use gtasks::model::ItemKind;

fn fake_clone(root: &std::path::Path, repo_name: &str) {
    let git = root.join(repo_name).join(".git");
    std::fs::create_dir_all(&git).expect("create .git");
}

fn config_from(text: &str) -> Config {
    toml::from_str::<Config>(text).expect("the config text must parse")
}

/// The regression: a config that lists the old plural key must still make agents work.
#[test]
fn the_plural_key_survives_the_rename() {
    let config = config_from(
        "repos = [\"meviusisback/git-tasks-tui\"]\n\
         clone_roots = [\"/home/alberto/repo\", \"/home/alberto/repos\"]\n",
    );
    assert_eq!(
        config.agent_roots(),
        vec![
            PathBuf::from("/home/alberto/repo"),
            PathBuf::from("/home/alberto/repos")
        ],
        "the list the old key described is the list the program searches"
    );
}

/// A split layout: the clone is in the second root. One path could not have found it.
#[test]
fn the_clone_is_found_in_the_second_root() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let empty = tmp.path().join("repo");
    let other = tmp.path().join("repos");
    std::fs::create_dir_all(&empty).expect("first root");
    std::fs::create_dir_all(&other).expect("second root");
    fake_clone(&other, "git-tasks-tui");

    let roots = vec![empty.clone(), other.clone()];
    let plan = plan_agent(
        "meviusisback/git-tasks-tui",
        7,
        "https://github.com/meviusisback/git-tasks-tui/pull/7",
        ItemKind::Pr,
        Some("fix-foo"),
        test_inputs("default", &roots),
    )
    .expect("the clone exists in the second root");

    assert_eq!(
        plan.clone,
        other.join("git-tasks-tui"),
        "the plan must name the clone that exists, not the first root tried"
    );
}

/// The refusal names every place that was looked at: a user with one typo must not have to guess
/// which of their roots the program visited.
#[test]
fn a_missing_clone_names_every_root_tried() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let a = tmp.path().join("a");
    let b = tmp.path().join("b");
    std::fs::create_dir_all(&a).expect("a");
    std::fs::create_dir_all(&b).expect("b");

    let err = plan_agent(
        "meviusisback/nope",
        1,
        "https://example.invalid/pull/1",
        ItemKind::Pr,
        Some("x"),
        test_inputs("default", &[a.clone(), b.clone()]),
    )
    .expect_err("no clone anywhere must be refused");

    for root in [&a, &b] {
        assert!(
            err.contains(&root.display().to_string()),
            "the refusal must name {root:?}: {err}"
        );
    }
}

/// The control: no roots at all keeps the old sentence, word for word.
#[test]
fn no_roots_keeps_the_sentence_that_explains_the_config() {
    let err = plan_agent(
        "meviusisback/git-tasks-tui",
        1,
        "https://github.com/meviusisback/git-tasks-tui/pull/1",
        ItemKind::Pr,
        Some("x"),
        test_inputs("default", &[]),
    )
    .expect_err("no roots must be refused");
    assert_eq!(err, "no clone_root configured — agents need local clones");
}

/// `clone_root` (single) is tried first, so a config written for the current key keeps working;
/// duplicates are searched once.
#[test]
fn the_single_root_comes_first_and_is_not_searched_twice() {
    let config = config_from(
        "repos = [\"a/b\"]\n\
         clone_root = \"/one\"\n\
         clone_roots = [\"/two\", \"/one\"]\n",
    );
    assert_eq!(
        config.agent_roots(),
        vec![PathBuf::from("/one"), PathBuf::from("/two")],
        "specific first, and no duplicate"
    );
}
