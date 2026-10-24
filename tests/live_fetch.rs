//! Live check against real `gh`; ignored by default, run with `--ignored`.

use std::time::Duration;

use gtasks::fetch::fetch_one;
use gtasks::model::ItemKind;

#[test]
#[ignore = "needs network and an authenticated gh"]
fn a_real_repo_can_be_fetched() {
    match fetch_one("cli/cli", ItemKind::Pr, 5, Duration::from_secs(30)) {
        Ok(items) => {
            println!("fetched {} items", items.len());
            assert!(!items.is_empty(), "cli/cli has open PRs");
            assert!(items.iter().all(|i| i.repo == "cli/cli"));
        }
        Err(err) => panic!("fetch failed: {err:#}"),
    }
}
