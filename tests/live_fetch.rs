//! Live check against real `gh`; ignored by default, run with `--ignored`.

use std::time::Duration;

use gtasks::actions::RepoSlug;
use gtasks::binaries::Binaries;
use gtasks::fetch::fetch_graphql_many;

#[test]
#[ignore = "needs network and an authenticated gh"]
fn a_real_repo_can_be_fetched() {
    let binaries = Binaries::detect().expect("gh and git on PATH");
    let repos = vec![RepoSlug::parse("cli/cli").expect("valid slug")];
    let outcome = fetch_graphql_many(&binaries, &repos, 5, Duration::from_secs(30), 1, false);
    assert!(
        outcome.failures.is_empty(),
        "fetch failed: {:?}",
        outcome.failures
    );
    assert!(!outcome.items.is_empty(), "cli/cli has open PRs");
    assert!(outcome.items.iter().all(|i| i.repo == "cli/cli"));
}
