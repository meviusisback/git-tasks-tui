//! The launcher receives exactly one argument: the validated URL.

use std::os::unix::fs::PermissionsExt;
use std::time::{Duration, Instant};

use gtasks::actions::open_in_browser;

#[test]
fn the_launcher_gets_one_argument_and_nothing_else() {
    let dir = tempfile::tempdir().unwrap();
    let script = dir.path().join("xdg-open");
    let out = dir.path().join("args.txt");
    std::fs::write(
        &script,
        // One write, not two: a reader must never see half the answer.
        format!(
            "#!/bin/sh\nprintf '%s%s' \"$#\" \"$1\" > {}\n",
            out.display()
        ),
    )
    .unwrap();
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();

    let url = "https://github.com/o/r/pull/1";
    open_in_browser(&script, url).expect("launcher starts");

    // The child is detached: wait for it to write, briefly.
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut seen = String::new();
    while Instant::now() < deadline {
        seen = std::fs::read_to_string(&out).unwrap_or_default();
        if !seen.is_empty() {
            break;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    assert_eq!(seen, format!("1{url}"), "argc and the single argument");
}

#[test]
fn a_relative_launcher_is_refused() {
    let err = open_in_browser(
        std::path::Path::new("xdg-open"),
        "https://github.com/o/r/pull/1",
    )
    .unwrap_err();
    assert!(err.to_string().contains("absolute"), "got {err}");
}

#[test]
fn an_unsafe_url_never_reaches_the_launcher() {
    let dir = tempfile::tempdir().unwrap();
    let script = dir.path().join("xdg-open");
    std::fs::write(&script, "#!/bin/sh\ntouch \"$1.marker\"\n").unwrap();
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();

    let err = open_in_browser(&script, "file:///etc/passwd").unwrap_err();
    assert!(err.to_string().contains("https"), "got {err}");
}

/// B-2: the launcher is detached but must still be reaped, otherwise every
/// press of `o` leaves a zombie until the app exits.
#[test]
fn the_launcher_is_reaped() {
    let dir = tempfile::tempdir().unwrap();
    let script = dir.path().join("xdg-open");
    std::fs::write(&script, "#!/bin/sh\nexit 0\n").unwrap();
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();

    let pid = open_in_browser(&script, "https://github.com/o/r/pull/1").expect("launcher starts");

    // A zombie keeps /proc/<pid> alive; a reaped child disappears.
    let deadline = Instant::now() + Duration::from_secs(5);
    let proc_entry = format!("/proc/{pid}");
    while Instant::now() < deadline {
        if !std::path::Path::new(&proc_entry).exists() {
            return;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    panic!("child {pid} was never reaped: {proc_entry} still exists");
}
