//! `N` starts a new GitHub project in one panel: name, visibility and
//! first branch. Confirming validates and queues the create — the loop
//! owns the network. All pinned here, without a terminal.

use gtasks::app::{App, Key, Modal, Msg, NewProjectField};

fn press(app: &mut App, key: Key) {
    app.update(Msg::Key(key));
}

fn project_modal(app: &App) -> gtasks::app::NewProject {
    match app.modal.clone() {
        Some(Modal::NewProject(input)) => input,
        other => panic!("a new-project panel is open, got {other:?}"),
    }
}

fn type_text(app: &mut App, text: &str) {
    for c in text.chars() {
        app.update(Msg::Key(Key::Char(c)));
    }
}

#[test]
fn n_opens_a_blank_panel_private_by_default() {
    let mut app = App::default();
    press(&mut app, Key::Char('N'));
    let input = project_modal(&app);
    assert_eq!(input.field, NewProjectField::Name);
    assert!(input.private, "new projects start private");
    assert!(app.create_requested.is_none(), "nothing queued yet");
}

#[test]
fn the_panel_types_toggles_and_confirms() {
    let mut app = App::default();
    press(&mut app, Key::Char('N'));
    type_text(&mut app, "my-proj");
    press(&mut app, Key::Tab);
    assert_eq!(project_modal(&app).field, NewProjectField::Visibility);
    press(&mut app, Key::Char(' '));
    assert!(
        !project_modal(&app).private,
        "Space flips to public — asked each time, not assumed"
    );
    press(&mut app, Key::Tab);
    type_text(&mut app, "work");
    press(&mut app, Key::Enter);
    let req = app
        .create_requested
        .clone()
        .expect("confirming queues the create");
    assert_eq!(req.name, "my-proj");
    assert!(!req.private);
    assert_eq!(req.branch, "work");
    assert!(
        !matches!(app.modal, Some(Modal::NewProject(_))),
        "the panel closes on confirm"
    );
}

#[test]
fn empty_branch_means_name_dev() {
    let mut app = App::default();
    press(&mut app, Key::Char('N'));
    type_text(&mut app, "my-proj");
    press(&mut app, Key::Enter);
    let req = app.create_requested.clone().expect("queued");
    assert_eq!(req.branch, "my-proj-dev", "like W suggests");
}

#[test]
fn a_bad_name_keeps_the_panel_open() {
    let mut app = App::default();
    press(&mut app, Key::Char('N'));
    type_text(&mut app, "has space");
    press(&mut app, Key::Enter);
    assert!(
        app.create_requested.is_none(),
        "spaces never reach gh repo create"
    );
    assert!(
        matches!(app.modal, Some(Modal::NewProject(_))),
        "the panel stays, with the reason in the status"
    );
    press(&mut app, Key::Esc);
    assert!(
        !matches!(app.modal, Some(Modal::NewProject(_))),
        "Esc still backs out"
    );
}

#[test]
fn a_bad_branch_keeps_the_panel_open() {
    let mut app = App::default();
    press(&mut app, Key::Char('N'));
    type_text(&mut app, "my-proj");
    press(&mut app, Key::Tab);
    press(&mut app, Key::Tab);
    type_text(&mut app, "bad branch");
    press(&mut app, Key::Enter);
    assert!(app.create_requested.is_none());
    assert!(matches!(app.modal, Some(Modal::NewProject(_))));
}
