//! S7/S16 — the agent's argv, as the specification names it.
//!
//! `docs/security.md:53`: *"Never `--yolo`, `--accept-hooks`, `--tui`, `--oneshot`. Default
//! `--ignore-rules` (a repo-committed …)"*, and `docs/PLAN.md:223` lists these two tests by
//! name. They were missing, and the behaviour was missing with them: the agent is launched
//! **inside the worktree of the repository under review**, so without `--ignore-rules` it
//! auto-injects that repository's `AGENTS.md`, `.cursorrules`, memory and preloaded skills —
//! instructions written by whoever opened the pull request, arriving in the agent's prompt.
//!
//! The argv is the whole surface here: nothing else decides which flags the child sees.

use std::path::{Path, PathBuf};

use gtasks::agent::agent_argv;

fn argv_args() -> Vec<String> {
    let hermes = PathBuf::from("/usr/bin/hermes");
    let seed = PathBuf::from("/run/user/1000/gtasks/seed-1-0");
    let worktree = PathBuf::from("/home/user/worktrees/acme-widget");
    let checked = agent_argv(&hermes, "coder", &seed, &worktree);
    checked
        .as_argv()
        .args
        .iter()
        .map(|a| a.to_string_lossy().into_owned())
        .collect()
}

#[test]
fn agent_launch_uses_ignore_rules() {
    let args = argv_args();
    println!("argv: {args:?}");
    assert!(
        args.iter().any(|a| a == "--ignore-rules"),
        "the agent runs inside the worktree of an untrusted repository: without \
         `--ignore-rules` it inherits that repository's AGENTS.md/.cursorrules/memory, \
         i.e. instructions from the change under review. argv was {args:?}"
    );
}

#[test]
fn agent_argv_has_no_yolo_no_accept_hooks() {
    let args = argv_args();
    for forbidden in ["--yolo", "--accept-hooks", "--tui", "--oneshot"] {
        assert!(
            !args.iter().any(|a| a == forbidden),
            "{forbidden} must never be emitted: it stops the agent asking, and this tool \
             must never stop it. argv was {args:?}"
        );
    }
}

/// The hinge: the fixture must be able to produce an argv that *does* contain a flag, or the
/// two tests above would pass on an empty argv for the wrong reason.
#[test]
fn the_fixture_produces_a_real_argv() {
    let args = argv_args();
    assert!(
        args.iter().any(|a| a == "--query-file") && args.iter().any(|a| a == "--in"),
        "the seed and the worktree must travel as paths: argv was {args:?}"
    );
    let seed_at = args.iter().position(|a| a == "--query-file").expect("flag");
    let seed = Path::new("/run/user/1000/gtasks/seed-1-0");
    assert_eq!(
        args.get(seed_at + 1).map(String::as_str),
        seed.to_str(),
        "the seed must sit immediately after its flag"
    );
}
