//! Task 4: config/state round-trips, `0600` files, symlink refusal, validators.

use std::os::unix::fs::{PermissionsExt, symlink};
use std::path::Path;

use gtasks::config::{Config, from_plugin_json, validate_profile, validate_repo_slug};
use gtasks::state::State;

fn mode_of(path: &Path) -> u32 {
    std::fs::symlink_metadata(path)
        .unwrap()
        .permissions()
        .mode()
        & 0o7777
}

#[test]
fn defaults_are_sane() {
    let config = Config::default();
    assert_eq!(config.parallel, 4);
    assert_eq!(config.timeout_s, 30);
    assert_eq!(config.write_timeout_s, 60);
    assert_eq!(config.refresh_s, 300);
    assert_eq!(config.limit, 30);
    assert!(!config.trust_worktree_rules);
    assert!(!config.sandbox_agents);
    assert!(config.repos.is_empty());
    assert_eq!(config.agent_profile, "default");
    assert!(config.clone_root.is_none());
}

#[test]
fn config_roundtrip_is_private() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("gtasks/config.toml");

    let config = Config {
        repos: vec!["owner/repo".into()],
        parallel: 2,
        ..Config::default()
    };
    config.save(&path).unwrap();

    assert_eq!(mode_of(&path), 0o600, "config must be 0600");
    assert_eq!(mode_of(&root.path().join("gtasks")), 0o700, "dir 0700");
    assert_eq!(Config::load(&path).unwrap(), config);
}

#[test]
fn missing_file_yields_defaults() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("nope/config.toml");
    assert_eq!(Config::load(&path).unwrap(), Config::default());
}

#[test]
fn invalid_config_is_an_error_not_a_fallback() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("config.toml");

    std::fs::write(&path, "parallel = 99\n").unwrap();
    assert!(Config::load(&path).is_err(), "out-of-range value must fail");

    std::fs::write(&path, "this is not toml").unwrap();
    assert!(Config::load(&path).is_err(), "garbage must fail");

    std::fs::write(&path, "unknown_key = 1\n").unwrap();
    assert!(
        Config::load(&path).is_err(),
        "unknown keys must fail closed"
    );
}

type Mutation = Box<dyn Fn(&mut Config)>;

#[test]
fn validation_rejects_out_of_range_values() {
    let cases: Vec<Mutation> = vec![
        Box::new(|c| c.parallel = 0),
        Box::new(|c| c.parallel = 9),
        Box::new(|c| c.timeout_s = 0),
        Box::new(|c| c.write_timeout_s = 601),
        Box::new(|c| c.refresh_s = 5),
        Box::new(|c| c.limit = 0),
        Box::new(|c| c.limit = 31),
        Box::new(|c| c.repos = vec!["../etc".into()]),
        Box::new(|c| c.default_profile = Some("Bad Profile".into())),
        Box::new(|c| c.agent_profile = "Bad Profile".into()),
        Box::new(|c| c.agent_profile = String::new()),
    ];
    for mutate in cases {
        let mut config = Config::default();
        mutate(&mut config);
        assert!(
            config.validate().is_err(),
            "config must be rejected: {config:?}"
        );
    }
}

#[test]
fn save_refuses_symlinked_target() {
    let root = tempfile::tempdir().unwrap();
    let victim = root.path().join("victim.toml");
    std::fs::write(&victim, "ORIGINAL").unwrap();
    let link = root.path().join("config.toml");
    symlink(&victim, &link).unwrap();

    let err = Config::default()
        .save(&link)
        .expect_err("symlinked target must be refused");
    assert!(format!("{err:#}").contains("symlink"), "got: {err:#}");
    assert_eq!(std::fs::read_to_string(&victim).unwrap(), "ORIGINAL");
}

#[test]
fn state_roundtrip_and_privacy() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("gtasks/state.json");

    let state = State {
        selected_repos: vec!["owner/repo".into()],
        sort_key: "created".into(),
        ascending: true,
        ..State::default()
    };
    state.save(&path).unwrap();

    assert_eq!(mode_of(&path), 0o600, "state must be 0600");
    assert_eq!(State::load(&path).unwrap(), state);
    assert_eq!(
        State::load(&root.path().join("missing.json")).unwrap(),
        State::default()
    );
}

#[test]
fn state_refuses_invalid_repo() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("state.json");
    let state = State {
        selected_repos: vec!["../etc".into()],
        ..State::default()
    };
    assert!(state.save(&path).is_err());
}

#[test]
fn plugin_import_maps_only_selected_repos() {
    let json = r#"{"selectedRepos":["owner/repo","other/name"],"windowWidth":900}"#;
    assert_eq!(
        from_plugin_json(json).unwrap(),
        vec!["owner/repo".to_string(), "other/name".to_string()]
    );

    assert_eq!(
        from_plugin_json(r#"{"foo":1}"#).unwrap(),
        Vec::<String>::new()
    );
    assert!(from_plugin_json(r#"{"selectedRepos":["../bad"]}"#).is_err());
    assert!(from_plugin_json("not json").is_err());
}

#[test]
fn validators_reject_traversal_and_bad_profiles() {
    for bad in [
        "", "/", "owner", "owner/", "/repo", "a/b/c", "a b/c", "-a/b", "a/../b",
    ] {
        assert!(
            validate_repo_slug(bad).is_err(),
            "slug {bad:?} must be refused"
        );
    }
    for good in ["owner/repo", "Owner.Name/repo_1", "a/b"] {
        validate_repo_slug(good).expect("valid slug");
    }

    for bad in ["", "Coder", "a b", "-x", "a/b", "ä"] {
        assert!(
            validate_profile(bad).is_err(),
            "profile {bad:?} must be refused"
        );
    }
    validate_profile("coder").unwrap();
    validate_profile("9lives").unwrap();
}
