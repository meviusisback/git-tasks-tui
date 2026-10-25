//! The agent runs embedded: a pty child parsed live, keys routed as bytes.
//!
//! The hinge is the environment: the pty child must see the allowlist and
//! nothing else — the same property `agent_env_interactive` proves for the
//! old blocking path, proved here for the pty spawn.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use gtasks::agent::{AgentOutcome, AgentStatus};
use gtasks::app::Key;
use gtasks::embedded::{EmbeddedSession, key_to_pty};

fn dirty_parent() -> HashMap<String, String> {
    let mut parent = HashMap::new();
    parent.insert("PATH".into(), "/usr/bin:/bin".into());
    parent.insert("HOME".into(), "/home/user".into());
    parent.insert("GH_TOKEN".into(), "ghp_leak_me".into());
    parent.insert("GIT_DIR".into(), "/somewhere/else".into());
    parent
}

fn fake_hermes(dir: &Path, script: &str, dump: Option<&Path>) -> PathBuf {
    let path = dir.join("hermes");
    let mut body = String::from("#!/bin/sh\n");
    if let Some(dump) = dump {
        body.push_str(&format!("env > {}\n", dump.display()));
    }
    body.push_str(script);
    body.push('\n');
    std::fs::write(&path, body).expect("write fake hermes");
    let mut perms = std::fs::metadata(&path).expect("stat").permissions();
    std::os::unix::fs::PermissionsExt::set_mode(&mut perms, 0o700);
    std::fs::set_permissions(&path, perms).expect("chmod");
    path
}

fn seed_in(dir: &Path) -> gtasks::agent::SeedFile {
    gtasks::agent::SeedFile::create_in(dir, "prompt body").expect("seed")
}

fn spawn(program: &Path, seed_dir: &Path, worktree: &Path) -> EmbeddedSession {
    // The argv seed only needs to exist as a path: the fake hermes never
    // reads it. The session owns a real SeedFile, unlinked on exit.
    let argv_seed = seed_dir.join("argv-seed");
    std::fs::write(&argv_seed, "seed\n").expect("argv seed");
    let argv = gtasks::agent::agent_argv(program, "default", &argv_seed, worktree);
    let seed = seed_in(seed_dir);
    EmbeddedSession::spawn(
        &argv,
        seed,
        1,
        "o/r".to_string(),
        7,
        "default".to_string(),
        "hermes".to_string(),
        worktree.to_path_buf(),
        40,
        10,
        &dirty_parent(),
    )
    .expect("spawn in pty")
}

fn child_env(path: &Path) -> Vec<(String, String)> {
    std::fs::read_to_string(path)
        .expect("the child must have written its environment")
        .lines()
        .filter_map(|line| {
            line.split_once('=')
                .map(|(k, v)| (k.to_string(), v.to_string()))
        })
        .collect()
}

fn wait_for_screen(sess: &mut EmbeddedSession, needle: &str) -> bool {
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline {
        if sess.drain() {
            let flat: String = sess
                .snapshot()
                .iter()
                .flat_map(|row| row.iter().map(|cell| cell.text.clone()))
                .collect();
            if flat.contains(needle) {
                return true;
            }
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    false
}

fn wait_for_exit(sess: &mut EmbeddedSession) -> gtasks::agent::AgentRecord {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        sess.drain();
        if let Some(record) = sess.poll_exit() {
            return record;
        }
        assert!(Instant::now() < deadline, "the child did not exit in time");
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[test]
fn the_pty_child_sees_the_allowlist_and_nothing_else() {
    let tmp = tempfile::tempdir().unwrap();
    let dump = tmp.path().join("child.env");
    fake_hermes(tmp.path(), "echo hello-agent\nsleep 30", Some(&dump));
    let mut sess = spawn(&tmp.path().join("hermes"), tmp.path(), tmp.path());
    assert!(
        wait_for_screen(&mut sess, "hello-agent"),
        "live output parses"
    );
    // Kill rather than wait 30s: the record must name the signal.
    sess.kill();
    let record = wait_for_exit(&mut sess);
    assert!(
        matches!(
            record.status,
            AgentStatus::Finished {
                outcome: AgentOutcome::Signal(_),
                ..
            }
        ),
        "a killed child is a signal, not Failed: {:?}",
        record.status
    );

    let env = child_env(&dump);
    let keys: Vec<&str> = env.iter().map(|(k, _)| k.as_str()).collect();
    for expected in ["PATH", "HOME"] {
        assert!(
            keys.contains(&expected),
            "the allowlist must keep {expected}: {keys:?}"
        );
    }
    for banned in ["GH_TOKEN", "GITHUB_TOKEN", "GIT_DIR"] {
        assert!(
            !keys.contains(&banned),
            "the pty child must not see {banned}: {keys:?}"
        );
    }
}

#[test]
fn an_exiting_child_reports_its_code() {
    let tmp = tempfile::tempdir().unwrap();
    fake_hermes(tmp.path(), "exit 3", None);
    let mut sess = spawn(&tmp.path().join("hermes"), tmp.path(), tmp.path());
    let record = wait_for_exit(&mut sess);
    assert_eq!(record.repo, "o/r");
    assert_eq!(record.number, 7);
    assert!(
        matches!(
            record.status,
            AgentStatus::Finished {
                outcome: AgentOutcome::Exit(3),
                ..
            }
        ),
        "exit code survives the pty: {:?}",
        record.status
    );
}

#[test]
fn the_seed_file_is_unlinked_on_exit() {
    let tmp = tempfile::tempdir().unwrap();
    fake_hermes(tmp.path(), "exit 0", None);
    let seed_path = tmp.path().join("seed-1-0");
    let argv = gtasks::agent::agent_argv(
        &tmp.path().join("hermes"),
        "default",
        &seed_path,
        tmp.path(),
    );
    // The session owns its own seed: create it through the constructor path.
    let seed = seed_in(tmp.path());
    let seed_file = seed.path().to_path_buf();
    let mut sess = EmbeddedSession::spawn(
        &argv,
        seed,
        1,
        "o/r".to_string(),
        7,
        "default".to_string(),
        "hermes".to_string(),
        tmp.path().to_path_buf(),
        40,
        10,
        &dirty_parent(),
    )
    .expect("spawn");
    assert!(seed_file.exists(), "the seed exists while the agent runs");
    wait_for_exit(&mut sess);
    drop(sess);
    assert!(!seed_file.exists(), "the seed is gone with the session");
}

#[test]
fn keys_become_terminal_bytes_except_pane_keys() {
    assert_eq!(key_to_pty(&Key::Enter), Some(b"\r".to_vec()));
    assert_eq!(key_to_pty(&Key::Backspace), Some(vec![0x7f]));
    assert_eq!(key_to_pty(&Key::Tab), None, "Tab cycles panes");
    assert_eq!(key_to_pty(&Key::BackTab), None, "BackTab cycles panes");
    assert_eq!(key_to_pty(&Key::Char('a')), Some(b"a".to_vec()));
    assert_eq!(key_to_pty(&Key::Up), Some(b"\x1b[A".to_vec()));
    assert_eq!(key_to_pty(&Key::CtrlC), Some(vec![0x03]));
    assert_eq!(key_to_pty(&Key::Esc), None);
}

#[test]
fn the_wheel_reads_history_not_input() {
    // Fifty lines in a ten-row box: scroll up shows the head, scroll down
    // returns to the live tail. Typing (`write`) jumps back to live, so a
    // reply never lands on a history view.
    let tmp = tempfile::tempdir().unwrap();
    // No trailing newline: the last line stays on the last row instead of
    // scrolling one past it, so the live tail is exactly rows 41-50.
    fake_hermes(tmp.path(), "seq 1 49; printf 50", None);
    let mut sess = spawn(&tmp.path().join("hermes"), tmp.path(), tmp.path());
    assert!(wait_for_screen(&mut sess, "50"), "the whole run parsed");
    sess.scroll(1000); // clamped to the actual scrollback
    let row_text = |sess: &EmbeddedSession, row: usize| -> String {
        sess.snapshot()[row]
            .iter()
            .map(|cell| cell.text.clone())
            .collect::<String>()
            .trim()
            .to_string()
    };
    assert_eq!(row_text(&sess, 0), "1", "scrolled to the head");
    sess.scroll(-1000);
    assert_eq!(row_text(&sess, 9), "50", "back on the live tail");
    // History view, then a keystroke: the screen follows the reply.
    sess.scroll(1000);
    assert_eq!(row_text(&sess, 0), "1");
    sess.write(b"x");
    assert_eq!(
        row_text(&sess, 9),
        "50",
        "typing jumps back to the live tail"
    );
}

#[test]
fn pty_exit_codes_map_like_the_blocking_path() {
    assert!(matches!(
        gtasks::agent::status_for_pty_exit(0, None, 1),
        AgentStatus::Finished {
            outcome: AgentOutcome::Exit(0),
            ..
        }
    ));
    assert!(matches!(
        gtasks::agent::status_for_pty_exit(1, Some("Terminated"), 1),
        AgentStatus::Finished {
            outcome: AgentOutcome::Signal(15),
            ..
        }
    ));
    assert!(matches!(
        gtasks::agent::status_for_pty_exit(1, Some("SIGKILL"), 1),
        AgentStatus::Finished {
            outcome: AgentOutcome::Signal(9),
            ..
        }
    ));
    assert!(matches!(
        gtasks::agent::status_for_pty_exit(1, Some("weird"), 1),
        AgentStatus::Finished {
            outcome: AgentOutcome::Failed,
            ..
        }
    ));
}

#[test]
fn alt_screen_content_folds_into_history() {
    // A fullscreen app draws on the alternate screen: the scrolled lines
    // still reach the wheel, detected from frame to frame.
    let tmp = tempfile::tempdir().unwrap();
    let program = fake_hermes(
        tmp.path(),
        "printf '\\033[?1049h'; seq 1 49; printf 50; printf '\\033[?1049l'; sleep 5",
        None,
    );
    let mut sess = spawn(&program, tmp.path(), tmp.path());
    assert!(
        wait_for_screen(&mut sess, "50"),
        "the folded output reaches the screen"
    );
    sess.scroll(1000);
    let row_text = |sess: &EmbeddedSession, row: usize| -> String {
        sess.snapshot()[row]
            .iter()
            .map(|cell| cell.text.clone())
            .collect::<String>()
            .trim()
            .to_string()
    };
    assert_eq!(row_text(&sess, 0), "1", "scrolled to the head of history");
    sess.scroll(-1000);
    assert_eq!(row_text(&sess, 9), "50", "the live tail is intact");
}

#[test]
fn repaint_frames_are_history_when_nothing_scrolls() {
    // A repaint app (Pi, agy) repositions and overwrites: no scroll ever
    // fires, so the line scrollback stays empty and the wheel walks the
    // journal of past frames instead. The two frames differ well above the
    // spinner threshold, like any real repaint.
    let tmp = tempfile::tempdir().unwrap();
    let program = fake_hermes(
        tmp.path(),
        "printf '\\033[2J\\033[1;1Hfirst-state-AAAAAAAAAAAA'; sleep 1; printf '\\033[2J\\033[1;1Hsecond-state-BBBBBBBBBBBB'; sleep 5",
        None,
    );
    let mut sess = spawn(&program, tmp.path(), tmp.path());
    assert!(
        wait_for_screen(&mut sess, "second-state"),
        "the second frame reaches the screen"
    );
    let row_text = |sess: &EmbeddedSession, row: usize| -> String {
        sess.snapshot()[row]
            .iter()
            .map(|cell| cell.text.clone())
            .collect::<String>()
            .trim()
            .to_string()
    };
    assert_eq!(
        row_text(&sess, 0),
        "second-state-BBBBBBBBBBBB",
        "live first"
    );
    sess.scroll(1);
    assert_eq!(
        row_text(&sess, 0),
        "second-state-BBBBBBBBBBBB",
        "one step back is the newest recorded frame"
    );
    sess.scroll(1);
    assert_eq!(
        row_text(&sess, 0),
        "first-state-AAAAAAAAAAAA",
        "two steps back is the older frame"
    );
    sess.scroll(-100);
    assert_eq!(
        row_text(&sess, 0),
        "second-state-BBBBBBBBBBBB",
        "back on the live tail"
    );
}
