//! Task 12 repo menu: `R` multi-select, `Space` toggles, `Enter` applies to
//! the live list and reports the file write, `Esc` cancels. The file itself
//! (`state.json`, 0600, atomic) is `State::save`; here we pin the UI half.

use gtasks::app::{App, Key, Modal, Msg};

fn app_with_three_repos() -> App {
    let mut app = App {
        all_repos: ["me/a", "me/b", "me/c"]
            .iter()
            .map(|s| gtasks::actions::RepoSlug::parse(s).expect("valid slug"))
            .collect(),
        ..App::default()
    };
    app.set_repos(app.all_repos.clone());
    app
}

fn open_menu(app: &mut App) {
    app.update(Msg::Key(Key::Char('R')));
    assert!(
        matches!(app.modal, Some(Modal::RepoMenu(_))),
        "R opens the menu"
    );
}

fn marked(app: &App) -> Vec<usize> {
    match &app.modal {
        Some(Modal::RepoMenu(menu)) => menu.marked.iter().copied().collect(),
        _ => panic!("menu is not open"),
    }
}

#[test]
fn the_menu_opens_with_the_live_selection_marked() {
    let mut app = app_with_three_repos();
    open_menu(&mut app);
    assert_eq!(
        marked(&app),
        vec![0, 1, 2],
        "everything is marked by default"
    );
}

#[test]
fn space_toggles_and_the_cursor_moves() {
    let mut app = app_with_three_repos();
    open_menu(&mut app);
    app.update(Msg::Key(Key::Down)); // row 1
    app.update(Msg::Key(Key::Char(' '))); // unmark me/b
    assert_eq!(marked(&app), vec![0, 2]);
    app.update(Msg::Key(Key::Char(' '))); // mark again
    assert_eq!(marked(&app), vec![0, 1, 2]);
}

#[test]
fn enter_applies_to_the_live_list_and_reports_for_save() {
    let mut app = app_with_three_repos();
    open_menu(&mut app);
    app.update(Msg::Key(Key::Down)); // row 1: me/b
    app.update(Msg::Key(Key::Char(' '))); // unmark
    app.update(Msg::Key(Key::Enter));
    assert!(app.modal.is_none(), "Enter closes the menu");
    assert_eq!(
        app.selection_applied,
        Some(vec!["me/a".to_string(), "me/c".to_string()]),
        "the loop must save exactly this"
    );
    assert_eq!(
        app.repos.iter().map(|r| r.text()).collect::<Vec<_>>(),
        vec!["— all repos —", "me/a", "me/c"],
        "the live list narrows at once"
    );
}

#[test]
fn applying_a_new_selection_drops_the_stale_detail() {
    // The body belonged to the previous list: showing it here would say
    // "this is the item you are looking at" about an item that may be gone.
    let mut app = app_with_three_repos();
    app.detail = "old body".to_string();
    app.detail_for = Some(("me/a".to_string(), 1));
    open_menu(&mut app);
    app.update(Msg::Key(Key::Down)); // row 1: me/b
    app.update(Msg::Key(Key::Char(' '))); // unmark
    app.update(Msg::Key(Key::Enter)); // apply
    assert!(app.detail.is_empty());
    assert!(app.detail_for.is_none());
    assert!(app.detail_requested.is_none());
}

#[test]
fn esc_cancels_without_side_effects() {
    let mut app = app_with_three_repos();
    open_menu(&mut app);
    app.update(Msg::Key(Key::Char(' '))); // unmark row 0
    app.update(Msg::Key(Key::Esc));
    assert!(app.modal.is_none());
    assert!(app.selection_applied.is_none(), "nothing to save");
    assert_eq!(app.repos.len(), 4, "the live list is untouched");
}

#[test]
fn unmarking_everything_applies_everything() {
    // An empty check means "everything", like an empty state file: there is
    // no representation for "no repos", so the UI cannot ask for it.
    let mut app = app_with_three_repos();
    open_menu(&mut app);
    for _ in 0..3 {
        app.update(Msg::Key(Key::Char(' ')));
        app.update(Msg::Key(Key::Down));
    }
    assert!(marked(&app).is_empty());
    app.update(Msg::Key(Key::Enter));
    assert_eq!(app.selection_applied, Some(Vec::new()));
    assert_eq!(app.repos.len(), 4, "empty check shows everything");
}

#[test]
fn keys_are_swallowed_while_the_menu_is_open() {
    let mut app = app_with_three_repos();
    open_menu(&mut app);
    app.update(Msg::Key(Key::Char('q')));
    assert!(!app.quit, "q must not quit through the menu");
    assert!(
        matches!(app.modal, Some(Modal::RepoMenu(_))),
        "the menu stays open"
    );
}

#[test]
fn r_without_repos_says_so() {
    let mut app = App::default();
    app.update(Msg::Key(Key::Char('R')));
    assert!(app.modal.is_none());
    assert_eq!(app.status, "no repos configured");
}

#[test]
fn legend_names_the_menu_keys_while_open() {
    let mut app = app_with_three_repos();
    open_menu(&mut app);
    let legend = app.legend();
    assert!(legend.contains("Space mark"), "{legend}");
    assert!(legend.contains("Enter apply"), "{legend}");
}

#[test]
fn the_selection_survives_a_restart_on_disk() {
    // Save → load round trip, with the 0600 the brief demands: this is the
    // half the UI cannot see, so the test does the loop's job by hand.
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("state.json");
    let state = gtasks::state::State {
        selected_repos: vec!["me/a".to_string(), "me/c".to_string()],
        ..gtasks::state::State::default()
    };
    state.save(&path).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600, "state file must be private");
    }
    let back = gtasks::state::State::load(&path).unwrap();
    assert_eq!(back.selected_repos, state.selected_repos);
}
