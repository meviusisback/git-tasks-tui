//! S2: binaries are absolute and never group/world writable.

use std::os::unix::fs::PermissionsExt;

use gtasks::binaries::resolve_in;

fn fake_bin(dir: &std::path::Path, name: &str, mode: u32) -> std::path::PathBuf {
    let path = dir.join(name);
    std::fs::write(&path, "#!/bin/sh\nexit 0\n").unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(mode)).unwrap();
    path
}

/// Fake 0755 binary resolves to an absolute path.
#[test]
fn binaries_are_absolute_and_not_group_writable() {
    let tmp = tempfile::tempdir().unwrap();
    let path = fake_bin(tmp.path(), "gh", 0o755);

    let resolved = resolve_in("gh", &[tmp.path()]).unwrap();
    assert!(resolved.is_absolute(), "must be absolute");
    assert_eq!(resolved, path.canonicalize().unwrap());

    // 0775 (group writable) is refused...
    let tmp2 = tempfile::tempdir().unwrap();
    fake_bin(tmp2.path(), "gh", 0o775);
    resolve_in("gh", &[tmp2.path()]).expect_err("group-writable binary must be refused");

    // ...as are world-writable, non-executable, missing, and non-bare names.
    let tmp3 = tempfile::tempdir().unwrap();
    fake_bin(tmp3.path(), "gh", 0o757);
    resolve_in("gh", &[tmp3.path()]).expect_err("world-writable binary must be refused");

    let tmp4 = tempfile::tempdir().unwrap();
    fake_bin(tmp4.path(), "gh", 0o644);
    resolve_in("gh", &[tmp4.path()]).expect_err("non-executable binary must be refused");

    resolve_in("nope", &[tmp4.path()]).expect_err("missing binary must be refused");
    resolve_in("", &[tmp4.path()]).expect_err("empty name must be refused");
    resolve_in("a/b", &[tmp4.path()]).expect_err("name with slash must be refused");
}
