//! First-launch drafts: the agent opens on an empty prompt and the seed is
//! typed, not sent. These pin the argv half of that promise — no seed file,
//! no initial message, in any position, for any agent. (The typing half
//! lives in the loop; it chunks out of `App::draft_text`.)

use std::path::PathBuf;

use gtasks::agent::{AgentId, agent_argv_for_draft};

fn args(agent: AgentId) -> Vec<String> {
    let program = PathBuf::from("/usr/bin/agent");
    let worktree = PathBuf::from("/home/user/worktrees/acme-widget");
    agent_argv_for_draft(agent, &program, "coder", &worktree, false)
        .as_argv()
        .args
        .iter()
        .map(|a| a.to_string_lossy().into_owned())
        .collect()
}

fn flat_contains(args: &[String], needle: &str) -> bool {
    args.iter().any(|a| a == needle)
}

#[test]
fn draft_argv_carries_no_seed_anywhere() {
    for agent in [
        AgentId::Hermes,
        AgentId::Pi,
        AgentId::Omp,
        AgentId::OpenCode,
        AgentId::Agy,
    ] {
        let args = args(agent);
        assert!(
            !args.iter().any(|a| a.contains("seed")),
            "{agent:?} draft must not name a seed file: {args:?}"
        );
        assert!(
            !args.iter().any(|a| a.starts_with('@')),
            "{agent:?} draft must not use the @file form: {args:?}"
        );
    }
}

#[test]
fn hermes_draft_is_chat_without_a_query() {
    let args = args(AgentId::Hermes);
    for want in ["-p", "coder", "chat", "--in", "--ignore-rules"] {
        assert!(
            flat_contains(&args, want),
            "hermes draft keeps {want}: {args:?}"
        );
    }
    assert!(
        !flat_contains(&args, "--query-file"),
        "no initial message: {args:?}"
    );
}

#[test]
fn pi_and_agy_drafts_are_bare() {
    assert_eq!(args(AgentId::Pi), Vec::<String>::new(), "pi opens empty");
    assert_eq!(args(AgentId::Agy), Vec::<String>::new(), "agy opens empty");
}

#[test]
fn omp_draft_keeps_cwd_but_no_prompt() {
    let args = args(AgentId::Omp);
    assert!(
        flat_contains(&args, "--cwd"),
        "still starts in the tree: {args:?}"
    );
}

#[test]
fn opencode_draft_is_interactive_not_run() {
    let args = args(AgentId::OpenCode);
    assert!(
        !flat_contains(&args, "run") && !flat_contains(&args, "--file"),
        "`run --file` streams once and exits — a draft needs the chat: {args:?}"
    );
}

#[test]
fn omp_draft_keeps_the_sandbox_yolo() {
    let program = PathBuf::from("/usr/bin/omp");
    let worktree = PathBuf::from("/home/user/worktrees/acme-widget");
    let args = agent_argv_for_draft(AgentId::Omp, &program, "coder", &worktree, true)
        .as_argv()
        .args
        .iter()
        .map(|a| a.to_string_lossy().into_owned())
        .collect::<Vec<_>>();
    assert!(
        flat_contains(&args, "--approval-mode"),
        "sandbox semantics unchanged in draft: {args:?}"
    );
}
