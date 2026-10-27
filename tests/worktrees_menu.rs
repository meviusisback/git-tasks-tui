//! `g` opens the agent menu, the menu leads to the worktree: pick the agent,
//! then use an existing tree, name a new parallel one, rename or delete.
//! The issues list marks trees that already exist, and a click on a tab
//! opens it — all pinned here, without a terminal.

use std::path::PathBuf;

use gtasks::agent::{AgentId, agent_argv, agent_argv_for};
use gtasks::app::{App, Key, Modal, Msg, Pane};
use gtasks::fetch::FetchOutcome;
use gtasks::model::{Item, ItemKind};

fn item(repo: &str, number: u64, kind: ItemKind, head: Option<&str>) -> Item {
    Item {
        number,
        title: "t".to_string(),
        state: "OPEN".into(),
        is_draft: None,
        author: None,
        labels: Vec::new(),
        assignees: Vec::new(),
        updated_at: "2026-10-20T10:00:00Z".into(),
        created_at: String::new(),
        url: format!("https://github.com/{repo}/pull/{number}"),
        head_ref_name: head.map(str::to_string),
        base_ref_name: None,
        review_decision: None,
        mergeable: None,
        status_check_rollup: Vec::new(),
        closing_issues_references: Vec::new(),
        repo: repo.to_string(),
        kind,
    }
}

/// An app with one PR, a clone, a hermes binary and a worktree root: the
/// whole `g` flow runs without touching the network.
fn pr_app() -> (App, tempfile::TempDir) {
    let tmp = tempfile::tempdir().unwrap();
    let clone = tmp.path().join("r");
    std::fs::create_dir(&clone).unwrap();
    std::fs::create_dir(clone.join(".git")).unwrap();

    let mut app = App::default();
    app.set_repos(vec![
        gtasks::actions::RepoSlug::parse("o/r").expect("valid slug"),
    ]);
    app.apply_outcome(FetchOutcome {
        items: vec![item("o/r", 7, ItemKind::Pr, Some("fix-foo"))],
        failures: Vec::new(),
    });
    app.agent_profile = "default".to_string();
    app.clone_roots = vec![tmp.path().to_path_buf()];
    app.worktree_root = Some(tmp.path().join("wt"));
    let mut binaries =
        gtasks::binaries::Binaries::new(PathBuf::from("/bin/gh"), PathBuf::from("/usr/bin/git"));
    binaries.hermes = Some(PathBuf::from("/bin/hermes"));
    app.binaries = Some(binaries);
    (app, tmp)
}

fn press(app: &mut App, key: Key) {
    app.update(Msg::Key(key));
}

fn open_menu(app: &mut App) {
    press(app, Key::Char('g'));
    assert!(
        matches!(app.modal, Some(Modal::AgentMenu(_))),
        "g opens the agent menu, got {:?}",
        app.modal
    );
}

fn confirm_first_row(app: &mut App) {
    press(app, Key::Enter);
}

/// Plant a worktree exactly where the app looks: the path comes from
/// `resolve_in`, the same function the loop uses, so the test plants what
/// `create` would have made — not a hand-computed guess. The `.git` file
/// points at a gitdir whose HEAD names the branch, like a real tree.
fn plant_worktree(root: &std::path::Path, branch: &str) -> PathBuf {
    use gtasks::worktree::BranchName;
    let slug = gtasks::actions::RepoSlug::parse("o/r").unwrap();
    let branch = BranchName::parse(branch).unwrap();
    let path =
        gtasks::worktree::resolve_in(root, std::path::Path::new(""), &slug, &branch).unwrap();
    std::fs::create_dir_all(&path).unwrap();
    let gitdir = root.join("gitdir-").join(branch.as_str());
    std::fs::create_dir_all(&gitdir).unwrap();
    std::fs::write(gitdir.join("HEAD"), format!("ref: refs/heads/{branch}\n")).unwrap();
    std::fs::write(path.join(".git"), format!("gitdir: {}\n", gitdir.display())).unwrap();
    path
}

fn argv_args(id: AgentId, program: &str) -> Vec<String> {
    let argv = agent_argv_for(
        id,
        PathBuf::from(program).as_path(),
        "default",
        PathBuf::from("/tmp/seed-1").as_path(),
        PathBuf::from("/tmp/wt").as_path(),
        false,
    );
    argv.as_argv()
        .args
        .iter()
        .map(|a| a.to_string_lossy().into_owned())
        .collect()
}

#[test]
fn menu_enter_without_trees_names_the_branch() {
    let (mut app, _tmp) = pr_app();
    open_menu(&mut app);
    confirm_first_row(&mut app);
    let Some(Modal::NameInput(input)) = app.modal.as_ref() else {
        panic!(
            "the agent row leads to the name prompt, got {:?}",
            app.modal
        );
    };
    assert_eq!(input.value, "fix-foo", "the head branch is the suggestion");
    press(&mut app, Key::Enter);
    let plan = app
        .agent_requested
        .as_ref()
        .expect("confirming the name hands the plan to the loop");
    assert_eq!(plan.branch, "fix-foo");
    assert_eq!(plan.agent.id, AgentId::Hermes);
    assert!(app.modal.is_none());
}

#[test]
fn a_bad_name_keeps_the_prompt_open_with_the_reason() {
    let (mut app, _tmp) = pr_app();
    open_menu(&mut app);
    confirm_first_row(&mut app);
    for _ in 0.."fix-foo".len() {
        press(&mut app, Key::Backspace);
    }
    press(&mut app, Key::Char('-'));
    press(&mut app, Key::Enter);
    assert!(
        matches!(app.modal, Some(Modal::NameInput(_))),
        "a bad name keeps the prompt open"
    );
    assert!(
        app.status.contains("bad worktree name"),
        "the reason is on screen: {}",
        app.status
    );
    assert!(app.agent_requested.is_none(), "nothing launches");
}

#[test]
fn an_existing_tree_is_used_not_created() {
    let (mut app, tmp) = pr_app();
    let root = tmp.path().join("wt");
    let path = plant_worktree(&root, "fix-foo");
    open_menu(&mut app);
    confirm_first_row(&mut app);
    let Some(Modal::Worktrees(menu)) = app.modal.as_ref() else {
        panic!("an existing tree leads to the picker, got {:?}", app.modal);
    };
    assert_eq!(menu.rows.len(), 1);
    assert_eq!(menu.rows[0].path, path);
    press(&mut app, Key::Enter);
    let (plan, place) = app
        .agent_place
        .as_ref()
        .expect("using the row hands the plan plus its path to the loop");
    assert_eq!(plan.branch, "fix-foo");
    assert_eq!(*place, path, "the row's real path — never re-derived");
}

#[test]
fn a_renamed_tree_launches_where_it_is_not_where_it_was() {
    // A rename outside the app leaves the directory out of sync with the
    // branch: the dir is still `fix-foo` but the tree is on `fix-bar`.
    // Using the row must hand the loop the real path, or it would ask to
    // create a tree that already exists elsewhere.
    let (mut app, tmp) = pr_app();
    let root = tmp.path().join("wt");
    let path = plant_worktree(&root, "fix-foo");
    let gitdir = root.join("gitdir-").join("fix-foo");
    std::fs::write(gitdir.join("HEAD"), "ref: refs/heads/fix-bar\n").unwrap();
    open_menu(&mut app);
    confirm_first_row(&mut app);
    let Some(Modal::Worktrees(menu)) = app.modal.as_ref() else {
        panic!("the drifted tree is still listed, got {:?}", app.modal);
    };
    assert_eq!(menu.rows.len(), 1);
    assert_eq!(menu.rows[0].branch.as_str(), "fix-bar");
    press(&mut app, Key::Enter);
    let (plan, place) = app
        .agent_place
        .as_ref()
        .expect("using the row hands the plan plus its path to the loop");
    assert_eq!(plan.branch, "fix-bar");
    assert_eq!(*place, path, "the real directory, not resolve_in(fix-bar)");
}

#[test]
fn n_names_a_parallel_tree_r_renames_d_deletes() {
    let (mut app, tmp) = pr_app();
    let root = tmp.path().join("wt");
    plant_worktree(&root, "fix-foo");
    open_menu(&mut app);
    confirm_first_row(&mut app);

    // `n`: a new parallel tree, suggested from the head branch.
    press(&mut app, Key::Char('n'));
    let Some(Modal::NameInput(input)) = app.modal.as_ref() else {
        panic!("n opens the name prompt, got {:?}", app.modal);
    };
    assert_eq!(input.value, "fix-foo");
    for c in "-parallel".chars() {
        press(&mut app, Key::Char(c));
    }
    press(&mut app, Key::Enter);
    let plan = app
        .agent_requested
        .as_ref()
        .expect("the new name is a plan");
    assert_eq!(plan.branch, "fix-foo-parallel");

    // `r`: rename the row's tree — the prompt opens on the old name.
    let (mut rename_app, rename_tmp) = pr_app();
    plant_worktree(&rename_tmp.path().join("wt"), "fix-foo");
    open_menu(&mut rename_app);
    confirm_first_row(&mut rename_app);
    press(&mut rename_app, Key::Char('r'));
    let Some(Modal::NameInput(input)) = rename_app.modal.as_ref() else {
        panic!("r opens the rename prompt, got {:?}", rename_app.modal);
    };
    assert_eq!(input.value, "fix-foo");
    for c in "-v2".chars() {
        press(&mut rename_app, Key::Char(c));
    }
    press(&mut rename_app, Key::Enter);
    let order = rename_app
        .rename_go
        .as_ref()
        .expect("the rename goes to the loop");
    assert_eq!(order.old_branch.as_str(), "fix-foo");
    assert_eq!(order.new_branch.as_str(), "fix-foo-v2");
    assert!(rename_app.modal.is_none());

    // `d`: delete asks first, `y` moves the order to the loop.
    let (mut delete_app, delete_tmp) = pr_app();
    let path = plant_worktree(&delete_tmp.path().join("wt"), "fix-foo");
    open_menu(&mut delete_app);
    confirm_first_row(&mut delete_app);
    press(&mut delete_app, Key::Char('d'));
    assert!(
        matches!(delete_app.modal, Some(Modal::Confirm(_))),
        "d asks before deleting"
    );
    press(&mut delete_app, Key::Char('y'));
    let order = delete_app
        .delete_go
        .as_ref()
        .expect("y hands the delete to the loop");
    assert_eq!(order.path, path);
    assert_eq!(order.branch.as_str(), "fix-foo");
}

#[test]
fn w_opens_the_worktree_menu_without_any_agent() {
    let (mut app, tmp) = pr_app();
    let root = tmp.path().join("wt");
    let path = plant_worktree(&root, "fix-foo");
    press(&mut app, Key::Char('W'));
    let Some(Modal::Worktrees(menu)) = app.modal.as_ref() else {
        panic!("W opens the worktree menu, got {:?}", app.modal);
    };
    assert!(menu.agent.is_none(), "no agent picked yet");
    assert_eq!(menu.rows.len(), 1);
    assert_eq!(menu.rows[0].path, path);
    assert!(
        app.agent_requested.is_none(),
        "nothing launches from listing"
    );
}

#[test]
fn w_with_no_trees_still_opens_the_menu() {
    let (mut app, _tmp) = pr_app();
    press(&mut app, Key::Char('W'));
    assert!(
        matches!(app.modal, Some(Modal::Worktrees(_))),
        "even empty: the menu is where `n` lives, got {:?}",
        app.modal
    );
}

#[test]
fn w_refuses_issues_like_g_does() {
    let (mut app, _tmp) = pr_app();
    app.apply_outcome(FetchOutcome {
        items: vec![item("o/r", 9, ItemKind::Issue, None)],
        failures: Vec::new(),
    });
    press(&mut app, Key::Char('W'));
    assert!(app.modal.is_none(), "no menu without a branch");
    assert!(
        app.status.contains("no branch"),
        "the reason is on screen: {}",
        app.status
    );
}

#[test]
fn standalone_use_leads_to_the_agent_menu_with_the_tree_preset() {
    let (mut app, tmp) = pr_app();
    let root = tmp.path().join("wt");
    let path = plant_worktree(&root, "fix-foo");
    press(&mut app, Key::Char('W'));
    press(&mut app, Key::Enter);
    let Some(Modal::AgentMenu(menu)) = app.modal.as_ref() else {
        panic!("using a tree asks which agent, got {:?}", app.modal);
    };
    assert_eq!(menu.place.as_deref(), Some(path.as_path()));
    assert_eq!(menu.plan.branch, "fix-foo");
    press(&mut app, Key::Enter);
    let (plan, place) = app
        .agent_place
        .as_ref()
        .expect("confirming the agent launches on the preset tree");
    assert_eq!(plan.branch, "fix-foo");
    assert_eq!(place, &path);
    assert_eq!(plan.agent.id, AgentId::Hermes, "first row, hermes");
}

#[test]
fn standalone_new_names_then_picks_the_agent() {
    let (mut app, _tmp) = pr_app();
    press(&mut app, Key::Char('W'));
    press(&mut app, Key::Char('n'));
    // The head branch is the suggestion: confirming untouched keeps it.
    press(&mut app, Key::Enter);
    let Some(Modal::AgentMenu(menu)) = app.modal.as_ref() else {
        panic!("naming a tree asks which agent, got {:?}", app.modal);
    };
    assert_eq!(menu.plan.branch, "fix-foo");
    assert!(menu.place.is_some(), "the named path travels along");
    assert!(
        app.agent_requested.is_none(),
        "no launch before the agent pick"
    );
    press(&mut app, Key::Enter);
    let (plan, _place) = app
        .agent_place
        .as_ref()
        .expect("confirming the agent launches on the named tree");
    assert_eq!(plan.branch, "fix-foo");
}

#[test]
fn the_list_marks_items_that_have_a_worktree() {
    let (app, tmp) = pr_app();
    plant_worktree(&tmp.path().join("wt"), "fix-foo");
    let view = gtasks::ui::view_for(&app);
    assert_eq!(view.item_worktrees, vec![true]);
    let (app, _tmp) = pr_app();
    let view = gtasks::ui::view_for(&app);
    assert_eq!(view.item_worktrees, vec![false]);
}

#[test]
fn clicking_a_tab_opens_it_clicking_between_does_not() {
    use gtasks::ui::{tab_at, tabs_row};
    let (mut app, _tmp) = pr_app();
    app.apply_outcome(FetchOutcome {
        items: vec![
            item("o/r", 7, ItemKind::Pr, Some("fix-foo")),
            item("o/r", 8, ItemKind::Pr, Some("fix-bar")),
        ],
        failures: Vec::new(),
    });
    app.update(Msg::Key(Key::Enter));
    app.list_cursor = 1;
    app.update(Msg::Key(Key::Esc)); // Enter focuses right: back to the list
    app.update(Msg::Key(Key::Enter));
    assert_eq!(app.tabs.len(), 2);

    let area = ratatui::layout::Rect::new(0, 0, 200, 30);
    let row = tabs_row(area, app.col_pct);
    // Scan the strip instead of hand-computing widths: kind icons depend on
    // the host's fonts, so the test must not assume them.
    let mut seen_none = false;
    let mut seen_one = false;
    for x in row.x..row.x + row.width {
        match tab_at(&app, area, x, row.y) {
            Some(0) => assert!(!seen_none && !seen_one, "tab 0 comes first"),
            None => seen_none = true,
            Some(1) => {
                assert!(seen_none, "a separator sits between the tabs");
                seen_one = true;
            }
            Some(other) => panic!("only two tabs exist, got {other}"),
        }
    }
    assert!(seen_none, "the separator maps to nothing");
    assert!(seen_one, "the second tab is clickable");
    // The mark of the first tab, the row below, the empty strip past the end.
    assert_eq!(tab_at(&app, area, row.x, row.y), Some(0));
    assert_eq!(tab_at(&app, area, row.x + 1, row.y + 1), None);
    assert_eq!(tab_at(&app, area, row.x + row.width - 1, row.y), None);
    let (empty, _tmp) = pr_app();
    assert_eq!(tab_at(&empty, area, row.x + 1, row.y), None);
}

#[test]
fn every_agent_takes_the_seed_as_a_file_never_as_text() {
    let hermes = agent_argv(
        PathBuf::from("/bin/hermes").as_path(),
        "default",
        PathBuf::from("/tmp/seed-1").as_path(),
        PathBuf::from("/tmp/wt").as_path(),
    );
    let hermes_args: Vec<String> = hermes
        .as_argv()
        .args
        .iter()
        .map(|a| a.to_string_lossy().into_owned())
        .collect();
    // The dispatcher agrees with the proven builder for hermes.
    assert_eq!(argv_args(AgentId::Hermes, "/bin/hermes"), hermes_args);

    assert_eq!(
        argv_args(AgentId::Pi, "/bin/pi"),
        vec!["@/tmp/seed-1"],
        "pi takes the @file form from its own examples"
    );
    assert_eq!(
        argv_args(AgentId::Omp, "/bin/omp"),
        vec!["--cwd", "/tmp/wt", "@/tmp/seed-1"],
        "omp adds the explicit cwd to the same family shape"
    );
    assert_eq!(
        argv_args(AgentId::OpenCode, "/bin/opencode"),
        vec!["run", "--file", "/tmp/seed-1", "/tmp/wt"],
        "opencode run streams the session, then exits"
    );
    assert_eq!(
        argv_args(AgentId::Agy, "/bin/agy"),
        vec!["--prompt-interactive", "@/tmp/seed-1"],
        "agy runs the initial prompt interactively"
    );
    // A seed starting with `-` can never land in flag position: every argv
    // carries it as a path (`@…` or a flag value), never bare.
    for id in [
        AgentId::Hermes,
        AgentId::Pi,
        AgentId::Omp,
        AgentId::OpenCode,
        AgentId::Agy,
    ] {
        let args = argv_args(id, "/bin/x");
        assert!(
            args.iter().any(|a| a.contains("/tmp/seed-1")),
            "{id:?} carries the seed path"
        );
        assert!(
            !args.iter().any(|a| a == "-"),
            "{id:?} never passes a bare dash"
        );
    }
}

#[test]
fn worktree_argv_has_the_documented_git_shapes() {
    use gtasks::worktree::{move_argv, remove_argv, rename_branch_argv};
    let binaries =
        gtasks::binaries::Binaries::new(PathBuf::from("/bin/gh"), PathBuf::from("/usr/bin/git"));
    let clone = PathBuf::from("/c");
    let old = gtasks::worktree::BranchName::parse("fix-foo").unwrap();
    let new = gtasks::worktree::BranchName::parse("fix-bar").unwrap();

    let args: Vec<String> = rename_branch_argv(&binaries, &clone, &old, &new)
        .args
        .iter()
        .map(|a| a.to_string_lossy().into_owned())
        .collect();
    assert!(
        args.windows(4)
            .any(|w| w == ["branch", "-m", "fix-foo", "fix-bar"])
    );

    let args: Vec<String> = move_argv(
        &binaries,
        &clone,
        PathBuf::from("/o").as_path(),
        PathBuf::from("/n").as_path(),
    )
    .args
    .iter()
    .map(|a| a.to_string_lossy().into_owned())
    .collect();
    assert!(args.windows(3).any(|w| w == ["worktree", "move", "/o"]));

    let args: Vec<String> = remove_argv(&binaries, &clone, PathBuf::from("/o").as_path())
        .args
        .iter()
        .map(|a| a.to_string_lossy().into_owned())
        .collect();
    assert!(
        args.windows(4)
            .any(|w| w == ["worktree", "remove", "--force", "/o"])
    );
}

fn repos_focus_on(app: &mut App, cursor: usize) {
    app.pane = Pane::Repos;
    app.repos_cursor = cursor;
}

#[test]
fn w_in_the_repos_pane_suggests_a_branch_for_that_repo() {
    let (mut app, _tmp) = pr_app();
    repos_focus_on(&mut app, 1); // o/r, not All
    press(&mut app, Key::Char('W'));
    let Some(Modal::NameInput(input)) = app.modal.as_ref() else {
        panic!("W on a repo names the new branch, got {:?}", app.modal);
    };
    assert_eq!(input.value, "r-dev");
}

#[test]
fn w_on_all_or_without_a_clone_refuses_with_the_reason() {
    let (mut app, _tmp) = pr_app();
    repos_focus_on(&mut app, 0); // All
    press(&mut app, Key::Char('W'));
    assert!(app.modal.is_none());
    assert_eq!(app.status, "pick one repo first — All is not a place");

    let (mut app, _tmp) = pr_app();
    app.clone_roots = vec![];
    repos_focus_on(&mut app, 1);
    press(&mut app, Key::Char('W'));
    assert!(app.modal.is_none(), "no clone, no prompt");
    assert!(
        app.status.contains("no clone_roots configured"),
        "names the missing piece: {}",
        app.status
    );
}

#[test]
fn confirming_the_suggested_name_leads_to_the_agent_menu_with_place() {
    let (mut app, _tmp) = pr_app();
    repos_focus_on(&mut app, 1);
    press(&mut app, Key::Char('W'));
    press(&mut app, Key::Enter); // keep the suggestion
    let Some(Modal::AgentMenu(menu)) = app.modal.as_ref() else {
        panic!("naming asks which agent, got {:?}", app.modal);
    };
    assert_eq!(menu.plan.branch, "r-dev");
    assert_eq!(menu.plan.number, 0, "a repo tree works on no PR");
    let place = menu.place.as_ref().expect("the tree travels with the pick");
    assert!(
        place.ends_with("r-dev") || place.to_string_lossy().contains("r--dev"),
        "placed under the new branch: {}",
        place.display()
    );
}

fn with_plain_icons(f: impl FnOnce()) {
    // SAFETY: process-global like the tabs_session twin, same discipline —
    // the only reader is `icons::nerd()` and the previous value is restored
    // before returning. No other test in this binary touches the override.
    let previous = std::env::var_os("GTASKS_NERD");
    unsafe {
        std::env::set_var("GTASKS_NERD", "0");
    }
    f();
    unsafe {
        match previous {
            Some(value) => std::env::set_var("GTASKS_NERD", value),
            None => std::env::remove_var("GTASKS_NERD"),
        }
    }
}

fn render_200x30(app: &App) -> String {
    use gtasks::ui::{draw, view_for};
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    let view = view_for(app);
    let backend = TestBackend::new(200, 30);
    let mut terminal = Terminal::new(backend).unwrap();
    terminal.draw(|frame| draw(frame, app, &view)).unwrap();
    let buffer = terminal.backend().buffer().clone();
    let mut out = String::new();
    for y in 0..30 {
        for x in 0..200 {
            out.push_str(buffer[(x, y)].symbol());
        }
        out.push('\n');
    }
    out
}

#[test]
fn the_worktree_marker_keeps_its_gap_in_list_detail_and_tabs() {
    let (mut app, tmp) = pr_app();
    plant_worktree(&tmp.path().join("wt"), "fix-foo");
    app.tabs.push(gtasks::app::Tab {
        repo: "o/r".to_string(),
        number: 7,
    });
    app.active_tab = Some(0);
    // Hide the list rows so the only markers on screen are the detail
    // title and the tabs strip.
    app.set_repos(vec![
        gtasks::actions::RepoSlug::parse("o/r").expect("valid slug"),
        gtasks::actions::RepoSlug::parse("o/other").expect("valid slug"),
    ]);
    // Cursor onto o/other through keys, like a user: the filter follows.
    app.pane = gtasks::app::Pane::Repos;
    app.update(gtasks::app::Msg::Key(gtasks::app::Key::Down));
    app.update(gtasks::app::Msg::Key(gtasks::app::Key::Down));
    with_plain_icons(|| {
        let screen = render_200x30(&app);
        // Twice: once in the detail title, once in the tabs strip — the
        // list shows nothing, so a third would mean a leak.
        assert_eq!(
            screen.matches("◉").count(),
            2,
            "marker in detail + tabs: {screen}"
        );
        assert!(screen.contains("◉  #7"), "marker, gap, number: {screen}");
    });
    // And the strip knows it structurally, not just visually.
    let infos = app.tab_infos();
    assert_eq!(infos.len(), 1);
    assert!(infos[0].has_worktree);
}
