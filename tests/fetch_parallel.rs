//! Task 7b: parallel fetch, one failure does not sink the rest.

use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};

use gtasks::fetch::{FetchError, FetchOutcome, fetch_many_with};
use gtasks::model::{Item, ItemKind};

fn item(number: u64, repo: &str) -> Item {
    Item {
        number,
        title: format!("item {number}"),
        state: "OPEN".into(),
        is_draft: None,
        author: None,
        labels: Vec::new(),
        updated_at: "2026-10-20T10:00:00Z".into(),
        created_at: String::new(),
        url: String::new(),
        head_ref_name: None,
        review_decision: None,
        mergeable: None,
        status_check_rollup: Vec::new(),
        repo: repo.into(),
        kind: ItemKind::Pr,
    }
}

fn repos(names: &[&str]) -> Vec<String> {
    names.iter().map(|s| s.to_string()).collect()
}

#[test]
fn one_broken_repo_does_not_lose_the_others() {
    let repos = repos(&["a/one", "b/bad", "c/three"]);
    let outcome = fetch_many_with(&repos, 2, |repo| {
        if repo == "b/bad" {
            Err(FetchError::Timeout)
        } else {
            Ok(vec![item(1, repo)])
        }
    });

    assert_eq!(outcome.items.len(), 2);
    assert_eq!(outcome.failed(), 1);
    assert_eq!(outcome.failures[0].0, "b/bad");
    assert!(outcome.failures[0].1.contains("timed out"));
}

#[test]
fn results_keep_repo_order_whatever_finishes_first() {
    let repos = repos(&["a/1", "b/2", "c/3", "d/4"]);
    let outcome = fetch_many_with(&repos, 4, |repo| {
        // Make the later repos finish first.
        if repo.starts_with("a/") {
            std::thread::sleep(std::time::Duration::from_millis(40));
        }
        Ok(vec![item(1, repo)])
    });

    let order: Vec<&str> = outcome.items.iter().map(|i| i.repo.as_str()).collect();
    assert_eq!(order, vec!["a/1", "b/2", "c/3", "d/4"]);
}

#[test]
fn concurrency_is_bounded_and_every_repo_is_fetched_once() {
    let names: Vec<String> = (0..12).map(|i| format!("org/r{i}")).collect();
    let in_flight = AtomicUsize::new(0);
    let peak = AtomicUsize::new(0);
    let calls = Mutex::new(Vec::new());

    let outcome = fetch_many_with(&names, 3, |repo| {
        let now = in_flight.fetch_add(1, Ordering::SeqCst) + 1;
        peak.fetch_max(now, Ordering::SeqCst);
        std::thread::sleep(std::time::Duration::from_millis(15));
        in_flight.fetch_sub(1, Ordering::SeqCst);
        match calls.lock() {
            Ok(mut guard) => guard.push(repo.to_string()),
            Err(poisoned) => poisoned.into_inner().push(repo.to_string()),
        }
        Ok(vec![item(1, repo)])
    });

    assert_eq!(outcome.items.len(), 12);
    assert_eq!(outcome.failed(), 0);
    assert!(
        peak.load(Ordering::SeqCst) <= 3,
        "peak was {}",
        peak.load(Ordering::SeqCst)
    );

    let mut seen = match calls.into_inner() {
        Ok(value) => value,
        Err(poisoned) => poisoned.into_inner(),
    };
    seen.sort();
    seen.dedup();
    assert_eq!(seen.len(), 12, "each repo fetched exactly once");
}

#[test]
fn parallel_is_clamped_to_four() {
    let names: Vec<String> = (0..8).map(|i| format!("org/r{i}")).collect();
    let in_flight = AtomicUsize::new(0);
    let peak = AtomicUsize::new(0);

    fetch_many_with(&names, 99, |repo| {
        let now = in_flight.fetch_add(1, Ordering::SeqCst) + 1;
        peak.fetch_max(now, Ordering::SeqCst);
        std::thread::sleep(std::time::Duration::from_millis(10));
        in_flight.fetch_sub(1, Ordering::SeqCst);
        Ok(vec![item(1, repo)])
    });

    assert!(
        peak.load(Ordering::SeqCst) <= 4,
        "cap is 4, peak was {}",
        peak.load(Ordering::SeqCst)
    );
}

#[test]
fn an_empty_repo_list_is_not_an_error() {
    let outcome = fetch_many_with(&[], 2, |repo| Ok(vec![item(1, repo)]));
    assert_eq!(outcome.items.len(), 0);
    assert_eq!(outcome.failed(), 0);
    assert_eq!(FetchOutcome::default().failed(), 0);
}
