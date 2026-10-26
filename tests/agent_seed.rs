//! Task 1 — seed-file helper + TTY interactivity proof (S8, C1).
//!
//! - `agent_seed_file_0600_and_unlinked`: hermetic, runs in `cargo test`.
//! - `agent_stays_interactive_after_first_turn`: LIVE test (real `hermes` +
//!   LLM) under `script(1)` (spike-proven; no PTY crate needed). Ignored by
//!   default; run explicitly:
//!   `GTASKS_TEST_HERMES=/path/to/hermes GTASKS_TEST_PROFILE=coder cargo test -- --ignored agent_stays_interactive_after_first_turn`

use std::io::Read;

use gtasks::agent::{SeedFile, ensure_runtime_dir};

/// S8, the half that is easy to miss: the seed file must **never follow a symlink**.
///
/// `create_in` uses `create_new` on purpose — `O_CREAT|O_EXCL` fails when the path exists,
/// *including when it exists as a symlink*, even a dangling one. If it were plain `create`,
/// a symlink planted at a predictable name (`seed-<pid>-<n>`, and the pid is not a secret)
/// would make the tool write the seed through it, into a file of someone else's choosing.
///
/// The fixture is deterministic: every candidate name for this pid is a symlink to a victim
/// file, so the creation must fail **and the victim must be untouched**. Then the symlinks
/// are removed and the same call must succeed — the hinge that says the fixture is not
/// simply broken.
#[test]
fn a_symlink_at_every_candidate_name_is_never_followed() {
    use std::os::unix::fs::PermissionsExt;

    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().canonicalize().unwrap().join("gtasks");
    ensure_runtime_dir(&dir).unwrap();
    assert_eq!(
        std::fs::symlink_metadata(&dir)
            .unwrap()
            .permissions()
            .mode()
            & 0o7777,
        0o700
    );

    let victim = tmp.path().join("victim.txt");
    std::fs::write(&victim, "untouched").unwrap();

    let pid = std::process::id();
    // 1000, not 100: the name counter is a process-wide static and other tests in
    // this binary have already consumed values, so a narrow range would leave the first
    // attempted name un-symlinked and the call would simply succeed — a green fixture
    // that proves nothing about following. (Measured: with 100 names the call succeeded.)
    let names: Vec<String> = (0..1000).map(|n| format!("seed-{pid}-{n}")).collect();
    for name in &names {
        std::os::unix::fs::symlink(&victim, dir.join(name)).unwrap();
    }

    // `SeedFile` has no `Debug` (nothing to print about a file we refuse to create), so
    // the error is taken with a match instead of `expect_err`.
    let created = SeedFile::create_in(&dir, "SECRET-SEED");
    // The victim is read **before** the refusal is asserted: if the seed went through a
    // symlink, that is the finding, and the message must say so rather than reporting a
    // fixture that did not apply.
    let victim_after = std::fs::read_to_string(&victim).unwrap();
    assert_eq!(
        victim_after, "untouched",
        "the seed was written through the symlink: the victim file changed"
    );
    let err = match created {
        Ok(seed) => panic!(
            "every candidate name is a symlink, yet a file was created at {} — the loop \
             picked a name outside the fixture, so this run proves nothing",
            seed.path().display()
        ),
        Err(err) => err,
    };
    println!("refused: {err}");

    // Hinge: with the symlinks gone the same call succeeds, so the refusal above was the
    // symlinks and not a broken fixture.
    for name in &names {
        std::fs::remove_file(dir.join(name)).unwrap();
    }
    let seed = SeedFile::create_in(&dir, "SECRET-SEED").expect("a fresh name must be usable");
    assert!(seed.path().exists());
    assert_eq!(
        std::fs::read_to_string(&victim).unwrap(),
        "untouched",
        "the successful path must not touch the victim either"
    );
}

/// S8/S14: the seed lives in a `0600` file inside a `0700` dir and is
/// unlinked when the owner drops (normal exit and failure alike).
#[test]
fn agent_seed_file_0600_and_unlinked() {
    use std::os::unix::fs::PermissionsExt;

    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().canonicalize().unwrap().join("gtasks");
    ensure_runtime_dir(&dir).unwrap();

    let dir_mode = std::fs::symlink_metadata(&dir)
        .unwrap()
        .permissions()
        .mode()
        & 0o7777;
    assert_eq!(dir_mode, 0o700, "runtime dir must be 0700");

    let path;
    {
        let seed = SeedFile::create_in(&dir, "seed-body").unwrap();
        path = seed.path().to_path_buf();

        assert_eq!(std::fs::read_to_string(&path).unwrap(), "seed-body");

        let meta = std::fs::symlink_metadata(&path).unwrap();
        assert!(meta.file_type().is_file(), "seed must be a regular file");
        assert!(!meta.file_type().is_symlink(), "seed must not be a symlink");
        assert_eq!(
            meta.permissions().mode() & 0o7777,
            0o600,
            "seed file must be 0600"
        );
    }
    assert!(!path.exists(), "seed file must be gone after drop");
}

/// C1: `--query-file <regular-file>` on a TTY seeds an INTERACTIVE session —
/// the process is still alive when the timeout fires instead of answering
/// and exiting like `--oneshot` would.
///
/// `script(1)` provides the TTY (spike-proven on v0.22.5); `timeout` bounds
/// the run. Proof: the child is killed by the timeout (exit 124) and the
/// output shows the interactive prompt glyph. A one-shot would exit on its
/// own with the agent's status. Afterwards the seed file must be gone.
///
/// Test-only argv construction: S1 governs the shipped app (attacker-
/// influenced inputs); here every path is generated by `tempfile`/us, and
/// each part is still quoted with the app's own `shell-words` because
/// `script -c` takes a single string.
#[test]
#[ignore]
fn agent_stays_interactive_after_first_turn() {
    let hermes = std::env::var("GTASKS_TEST_HERMES").unwrap_or_else(|_| "hermes".to_string());
    let profile = std::env::var("GTASKS_TEST_PROFILE").unwrap_or_else(|_| "coder".to_string());

    let tmp = tempfile::tempdir().unwrap();
    let workdir = tmp.path().join("work");
    std::fs::create_dir(&workdir).unwrap();
    let seed = SeedFile::create_in(
        &tmp.path().canonicalize().unwrap(),
        "Reply with exactly: LIVE-SCRIPT-OK and nothing else. (gtasks Task 1 live TTY test; no tools needed.)",
    )
    .unwrap();
    let seed_path = seed.path().to_path_buf();

    let inner = [
        hermes.as_str(),
        "-p",
        profile.as_str(),
        "chat",
        "--query-file",
        seed_path.to_str().unwrap(),
        "--in",
        workdir.to_str().unwrap(),
        "--ignore-rules",
    ]
    .iter()
    .map(|s| shell_words::quote(s).into_owned())
    .collect::<Vec<_>>()
    .join(" ");
    let mut child = std::process::Command::new("timeout")
        .args(["60", "script", "-qec", &inner, "/dev/null"])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()
        .expect("spawn timeout/script — util-linux script(1) required");

    let mut out = Vec::new();
    child.stdout.take().unwrap().read_to_end(&mut out).unwrap();
    let status = child.wait().unwrap();
    let text = String::from_utf8_lossy(&out);
    // Last ~2000 chars (char-boundary safe: the output is full of 3-byte
    // box-drawing glyphs, so byte-slicing would panic).
    let tail: String = text
        .chars()
        .rev()
        .take(2000)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect();

    assert_eq!(
        status.code(),
        Some(124),
        "hermes must still be alive at the timeout (interactive session), got {status}\n--- output tail ---\n{tail}"
    );
    assert!(
        text.contains("❯"),
        "no interactive prompt glyph seen; did the seed really start a session?\n--- output tail ---\n{tail}"
    );

    // Teardown mirrors the app: the seed must be gone after the child exits.
    drop(seed);
    assert!(!seed_path.exists(), "seed file must be gone after exit");
}

/// C1 negative control: `--query-file -` must fail with exit 2 "is empty",
/// so a regression back to `-` can never pass silently.
///
/// No PTY and no `script` here: stdin is null, so the CLI reads EOF
/// immediately and rejects the empty query BEFORE any oneshot path.
/// A REAL (valid) profile is required: profile validation runs first, so a
/// bogus `-p` would exit 1 and the control would look green without ever
/// touching the query path.
#[test]
#[ignore] // needs a real `hermes`; run with GTASKS_TEST_HERMES + GTASKS_TEST_PROFILE
fn agent_query_file_dash_exits_2_empty() {
    let hermes = std::env::var("GTASKS_TEST_HERMES").unwrap_or_else(|_| "hermes".to_string());
    let profile = std::env::var("GTASKS_TEST_PROFILE").unwrap_or_else(|_| "coder".to_string());

    let tmp = tempfile::tempdir().unwrap();

    let out = std::process::Command::new(&hermes)
        .args([
            "-p",
            profile.as_str(),
            "chat",
            "--query-file",
            "-",
            "--in",
            tmp.path().to_str().unwrap(),
        ])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .output()
        .expect("spawn hermes");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(
        out.status.code(),
        Some(2),
        "--query-file - must exit 2, got {}\nstderr: {stderr}",
        out.status
    );
    assert!(
        stderr.to_lowercase().contains("is empty"),
        "stderr must say the query is empty, got: {stderr}"
    );
}
