//! A restarted gtasks finds its direct agents by pid, not by parenthood.
//!
//! The pid record is saved at launch; on restore a live pid re-shows the
//! tab's agent, a dead or recycled one is dropped. Reopening the tree
//! stops the detached process before resuming, so two agents never work
//! the same tree.

use gtasks::agent::{LiveDirect, owns_tree};

#[test]
fn a_live_pid_working_in_the_tree_counts() {
    let mut child = std::process::Command::new("sleep")
        .arg("30")
        .spawn()
        .expect("sleep on PATH");
    let pid = child.id();
    // The child execs asynchronously: right after the fork its cmdline is
    // still empty, so poll until it owns the word (or it never does).
    let mut owned = false;
    for _ in 0..100 {
        if owns_tree(pid, "sleep") {
            owned = true;
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    assert!(owned, "the sleep child owns the word sleep");
    child.kill().ok();
    let _ = child.wait();
    // Give the kernel a beat: a zombie still answers kill(pid, 0).
    for _ in 0..50 {
        if !owns_tree(pid, "sleep") {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    panic!("the reaped sleep still counts as ours");
}

#[test]
fn a_dead_pid_and_pid_zero_never_count() {
    assert!(!owns_tree(0, "anything"));
    // A pid that cannot exist: far past any real one.
    assert!(!owns_tree(1 << 30, "anything"));
}

#[test]
fn a_recycled_pid_working_elsewhere_does_not_count() {
    // Init owns no tree of ours: aliveness alone must not match.
    assert!(!owns_tree(1, "/nonexistent-tree-gtasks-detached-probe"));
}

#[test]
fn restore_re_shows_the_tab_whose_pid_is_alive() {
    use gtasks::app::{App, Tab};
    let exe = std::env::current_exe().expect("test binary path");
    let tree = exe.parent().expect("binary dir").display().to_string();
    let live = LiveDirect {
        pid: std::process::id(),
        repo: "me/a".to_string(),
        number: 1,
        agent: "hermes".to_string(),
        profile: "coder".to_string(),
    };
    let mut state = gtasks::state::State::default();
    state.tabs.push(gtasks::state::TabState {
        repo: "me/a".to_string(),
        number: 1,
    });
    state.active_tab = Some(0);
    state.live_direct.insert(tree.clone(), live);
    // Our own cmdline holds the test binary path, inside `tree`.
    assert!(
        owns_tree(std::process::id(), &tree),
        "precondition: we own our binary dir"
    );
    let mut app = App::default();
    app.restore_session(&state);
    let key = ("me/a".to_string(), 1);
    let entry = app.tab_agents.get(&key).expect("the shadow is back");
    assert!(entry.running, "the tab shows its agent as running");
    let record = entry.record.as_ref().expect("a record to name it");
    assert_eq!(record.worktree.display().to_string(), tree);
    assert!(matches!(record.status, gtasks::agent::AgentStatus::Running));
    // A dead pid leaves no shadow behind.
    let mut state2 = gtasks::state::State::default();
    state2.tabs.push(gtasks::state::TabState {
        repo: "me/a".to_string(),
        number: 1,
    });
    state2.live_direct.insert(
        "/nonexistent-tree-gtasks-detached-probe".to_string(),
        LiveDirect {
            pid: 1 << 30,
            repo: "me/a".to_string(),
            number: 1,
            agent: "hermes".to_string(),
            profile: "coder".to_string(),
        },
    );
    let mut app2 = App::default();
    app2.restore_session(&state2);
    assert!(
        !app2.tab_agents.contains_key(&key),
        "no shadow for a dead pid"
    );
    assert!(
        app2.live_direct.is_empty(),
        "the dead record is pruned, not kept"
    );
    let _ = Tab {
        repo: String::new(),
        number: 0,
    };
}

#[test]
fn agent_program_names() {
    use gtasks::agent::agent_program_name;
    assert_eq!(agent_program_name("omp"), Some("omp"));
    assert_eq!(agent_program_name("omp-sandboxed"), Some("omp"));
    assert_eq!(agent_program_name("/usr/bin/hermes"), None);
    assert_eq!(agent_program_name("hermes"), Some("hermes"));
    assert_eq!(agent_program_name("grep"), None);
    assert_eq!(agent_program_name("ompfoo"), None);
    assert_eq!(agent_program_name("myomp"), None);
}

#[test]
fn orphan_cmdline_matching() {
    use gtasks::agent::match_orphan_cmdline;
    // NUL-separated, as the kernel serves it.
    let tree = "/wt/o/r/issue--7";
    assert_eq!(
        match_orphan_cmdline("omp\0--cwd\0/wt/o/r/issue--7\0", tree),
        Some("omp")
    );
    assert_eq!(
        match_orphan_cmdline(
            "/home/a/.local/bin/hermes-sandboxed\0-p\0coder\0chat\0--in\0/wt/o/r/issue--7\0",
            tree
        ),
        Some("hermes")
    );
    // Right binary, wrong tree.
    assert_eq!(
        match_orphan_cmdline("omp\0--cwd\0/other/tree\0", tree),
        None
    );
    // Wrong binary, right tree (a grep through the tree is not an agent).
    assert_eq!(
        match_orphan_cmdline("grep\0omp\0/wt/o/r/issue--7/file\0", tree),
        None
    );
    assert_eq!(match_orphan_cmdline("", tree), None);
}

#[test]
fn stat_ppid_parsing() {
    use gtasks::agent::parse_stat_ppid;
    assert_eq!(parse_stat_ppid("123 (sleep) S 1 0 0"), Some(1));
    // comm with spaces and parens: the LAST `)` ends the name.
    assert_eq!(parse_stat_ppid("9 (my (odd) proc) S 7 0 0"), Some(7));
    assert_eq!(parse_stat_ppid("garbage"), None);
    assert_eq!(parse_stat_ppid("1 (x)"), None);
}

/// A normal child is never reported as an orphan, on any platform.
#[test]
fn parented_children_are_not_orphans() {
    let mut child = std::process::Command::new("sleep")
        .arg("30")
        .spawn()
        .expect("sleep on PATH");
    let plain = child.id();
    assert!(
        !gtasks::agent::orphaned_agents()
            .iter()
            .any(|(pid, _, _)| *pid == plain),
        "a parented child is not an orphan"
    );
    child.kill().ok();
    let _ = child.wait();
}
