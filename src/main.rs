//! gtasks — multi-repo GitHub issues & PRs in the terminal.
//!
//! Thin wrapper: logging, terminal lifetime, the event loop, and the one place
//! that owns the fetch thread. All state lives in `gtasks::app`, all drawing in
//! `gtasks::ui`.

#![deny(clippy::print_stdout)]

use std::collections::HashMap;
use std::io;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::time::Duration;

use clap::Parser;
use gtasks::actions;
use gtasks::actions::RepoSlug;
use gtasks::agent::{self, AgentPlan};
use gtasks::app::{App, Key, Modal, Msg, Pending};
use gtasks::binaries::Binaries;
use gtasks::config::Config;
use gtasks::embedded::EmbeddedSession;
use gtasks::fetch::{self, FetchOutcome};
use gtasks::input::map_key;
use gtasks::logging::{init_file_logging, init_stderr_logging};
use gtasks::model::ItemKind;
use gtasks::state::State;
use gtasks::ui;
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use ratatui::crossterm::event::{self, Event, MouseButton, MouseEventKind};
use ratatui::layout::Rect;

/// gtasks: GitHub issues & PRs across repos, in the terminal.
#[derive(Debug, Parser)]
#[command(name = "gtasks", version, about)]
struct Args {}

/// Set from a signal handler, so it must be a plain atomic store.
static TERMINATE: AtomicBool = AtomicBool::new(false);

extern "C" fn on_signal(_signal: libc::c_int) {
    TERMINATE.store(true, Ordering::SeqCst);
}

/// SIGTERM and SIGHUP would otherwise leave the terminal in raw mode.
fn install_signal_handlers() {
    // SAFETY: the handler only stores into an atomic, which is async-signal-safe.
    // The cast goes through a raw pointer because a bare function-item-to-integer
    // cast is deprecated (function_casts_as_integer).
    unsafe {
        let handler = on_signal as *const () as libc::sighandler_t;
        libc::signal(libc::SIGTERM, handler);
        libc::signal(libc::SIGHUP, handler);
        // SIGINT too: raw mode turns Ctrl-C into a key event, but an external
        // `kill -INT` would otherwise leave the terminal in the alternate
        // screen (S15).
        libc::signal(libc::SIGINT, handler);
    }
}

fn main() -> anyhow::Result<()> {
    let _args = Args::parse();

    match init_file_logging() {
        Ok(path) => {
            tracing::info!(
                version = env!("CARGO_PKG_VERSION"),
                log = %path.display(),
                "gtasks starting"
            );
        }
        Err(err) => {
            eprintln!("gtasks: file logging unavailable ({err:#}); logging to stderr");
            init_stderr_logging();
        }
    }

    run_tui()
}

/// `$XDG_CONFIG_HOME/gtasks/config.toml`, falling back to `~/.config`.
fn config_path() -> Option<PathBuf> {
    app_dir().map(|base| base.join("config.toml"))
}

/// `$XDG_CONFIG_HOME/gtasks/state.json`: the user's own picks (0600).
fn state_path() -> Option<PathBuf> {
    app_dir().map(|base| base.join("state.json"))
}

fn app_dir() -> Option<PathBuf> {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".config")))?;
    Some(base.join("gtasks"))
}

/// The config from disk when it is valid and non-empty, otherwise the demo
/// repo — plus what went wrong, for the screen. The fallback stays, but it
/// never stays silent: a missing file is a first run (no message), anything
/// present but unusable is shown until dismissed.
fn load_config() -> (Config, Option<String>) {
    let Some(path) = config_path() else {
        return (gtasks::config::demo_config(), None);
    };
    if !path.exists() {
        tracing::info!(path = %path.display(), "no config, using the demo repo");
        return (gtasks::config::demo_config(), None);
    }
    let (config, problem) = gtasks::config::load_or_demo(&path);
    let message = problem.as_ref().map(|p| {
        tracing::warn!(path = %path.display(), problem = ?p, "config unusable, demo fallback with a notice");
        p.message()
    });
    (config, message)
}

/// The loop's half of the agent handoff (`g` validated, `y` agreed).
/// `agreed` is `Some` when the user already said yes to creating the tree;
/// `None` means resolve first and ask before writing. The agent spawns
/// embedded in the agent box — the TUI never suspends.
fn start_agent(
    app: &mut App,
    binaries: &Binaries,
    config: &Config,
    plan: AgentPlan,
    agreed: Option<PathBuf>,
    term_size: (u16, u16),
    sessions: &mut HashMap<(String, u64), EmbeddedSession>,
) {
    // One agent per tab: the box shows one session, so a second launch on
    // the same tab is refused instead of orphaning the first. Other tabs
    // keep running untouched.
    let key = App::tab_key(&plan.repo, plan.number);
    if sessions.contains_key(&key) {
        app.status = "an agent is already running in this tab".to_string();
        return;
    }
    let timeout = Duration::from_secs(config.timeout_s.max(1));
    let root = match config.worktree_root.clone() {
        Some(root) => root,
        None => match gtasks::worktree::default_root() {
            Ok(root) => root,
            Err(err) => {
                app.status = format!("cannot locate the worktree root: {err:#}");
                return;
            }
        },
    };
    // The root is ours (data dir), not the user's: create it on first use so
    // `create` — which canonicalises and refuses to invent parents — has a
    // real directory to start from. A configured root must already exist:
    // silently building a tree under a typo'd path would hide the typo.
    if config.worktree_root.is_none() {
        if let Err(err) = gtasks::logging::ensure_state_dir(&root) {
            app.status = format!("cannot prepare the worktree root: {err:#}");
            return;
        }
    }
    let slug = match RepoSlug::parse(&plan.repo) {
        Ok(slug) => slug,
        Err(_) => {
            app.status = format!("cannot place the agent: invalid repo {}", plan.repo);
            return;
        }
    };
    let branch = match gtasks::worktree::BranchName::parse(&plan.branch) {
        Ok(branch) => branch,
        Err(_) => {
            app.status = format!("cannot place the agent: invalid branch {}", plan.branch);
            return;
        }
    };
    let target = match gtasks::worktree::resolve_in(&root, &plan.clone, &slug, &branch) {
        Ok(target) => target,
        Err(err) => {
            app.status = format!("cannot place the agent: {err:#}");
            return;
        }
    };
    // An agreed creation goes straight to git; anything else is asked first
    // — a write to disk is never a side effect of pressing `g`.
    if agreed.is_some() {
        match gtasks::worktree::create(&root, &plan.clone, &slug, &branch, binaries) {
            Ok(created) => {
                launch_agent(
                    app,
                    &plan,
                    &created.path,
                    term_size,
                    sessions,
                    binaries,
                    timeout,
                );
            }
            // The refusal goes to the screen as it is: a check failure must
            // not become indistinguishable from a bug in the check.
            Err(err) => app.status = format!("{err:#}"),
        }
        return;
    }
    match agent::next_step(target) {
        agent::AgentNext::Launch(path) => {
            launch_agent(app, &plan, &path, term_size, sessions, binaries, timeout);
        }
        agent::AgentNext::AskToCreate(path) => {
            let question = format!(
                "no worktree for {}@{} — create it and start the agent?",
                slug.as_str(),
                branch.as_str()
            );
            app.modal = Some(Modal::Confirm(question));
            app.pending = Some(Pending::Agent(plan, path));
        }
    }
}

/// Launch on the exact tree the worktree picker handed over (it exists —
/// the menu listed it). No resolve: after an outside rename the directory
/// no longer matches the branch name, and re-deriving the path would point
/// at a directory that was never created. If the tree vanished meanwhile,
/// it falls back to the creation question instead of failing silently.
/// The path is re-validated at launch (not a symlink, still under the root):
/// the menu row was recorded earlier and the disk may have changed since.
fn start_placed(
    app: &mut App,
    plan: AgentPlan,
    path: PathBuf,
    term_size: (u16, u16),
    sessions: &mut HashMap<(String, u64), EmbeddedSession>,
    binaries: &Binaries,
    timeout: Duration,
) {
    // One agent per tab, same as every other launch.
    let key = App::tab_key(&plan.repo, plan.number);
    if sessions.contains_key(&key) {
        app.status = "an agent is already running in this tab".to_string();
        return;
    }
    if !agent::placed_ok_in(&path, app.worktree_root.as_deref()) {
        let question = format!(
            "no worktree for {}@{} — create it and start the agent?",
            plan.repo, plan.branch,
        );
        app.modal = Some(Modal::Confirm(question));
        app.pending = Some(Pending::Agent(plan, path));
        return;
    }
    match agent::next_step(path) {
        agent::AgentNext::Launch(path) => {
            launch_agent(app, &plan, &path, term_size, sessions, binaries, timeout);
        }
        agent::AgentNext::AskToCreate(path) => {
            let question = format!(
                "no worktree for {}@{} — create it and start the agent?",
                plan.repo, plan.branch,
            );
            app.modal = Some(Modal::Confirm(question));
            app.pending = Some(Pending::Agent(plan, path));
        }
    }
}

/// Rename a worktree from a validated order: the branch first, then the
/// move. Branch-first means a failed move leaves a valid tree at the old
/// path on the new branch — still usable, never half-moved. Refused while a
/// live session works inside the tree.
fn rename_worktree(
    app: &mut App,
    binaries: &Binaries,
    order: gtasks::worktree::WorktreeRename,
    sessions: &HashMap<(String, u64), EmbeddedSession>,
) {
    if sessions.values().any(|s| s.worktree() == order.old_path) {
        app.status = "an agent is still running in that worktree".to_string();
        return;
    }
    let rename = gtasks::worktree::rename_branch_argv(
        binaries,
        &order.clone,
        &order.old_branch,
        &order.new_branch,
    );
    match gtasks::worktree::run_mutation(&rename, std::time::Duration::from_secs(60)) {
        Ok(out) if out.status.success() => {}
        Ok(out) => {
            app.status = format!(
                "cannot rename the branch: {}",
                String::from_utf8_lossy(&out.stderr).trim()
            );
            return;
        }
        Err(err) => {
            app.status = format!("cannot rename the branch: {err:#}");
            return;
        }
    }
    let mv = gtasks::worktree::move_argv(binaries, &order.clone, &order.old_path, &order.new_path);
    match gtasks::worktree::run_mutation(&mv, std::time::Duration::from_secs(60)) {
        Ok(out) if out.status.success() => {
            for agent in app.tab_agents.values_mut() {
                if let Some(record) = agent.record.as_mut() {
                    if record.worktree == order.old_path {
                        record.worktree = order.new_path.clone();
                    }
                }
            }
            app.status = format!("renamed {} to {}", order.old_branch, order.new_branch);
        }
        Ok(out) => {
            app.status = format!(
                "branch renamed, move failed (tree still at {}): {}",
                order.old_path.display(),
                String::from_utf8_lossy(&out.stderr).trim()
            );
        }
        Err(err) => {
            app.status = format!(
                "branch renamed, move failed (tree still at {}): {err:#}",
                order.old_path.display()
            );
        }
    }
}

/// Delete a worktree from a confirmed order. The branch is kept on purpose.
/// Refused while a live session works inside; a finished record for the
/// tree is cleared, so no card points at a deleted directory.
fn delete_worktree(
    app: &mut App,
    binaries: &Binaries,
    order: gtasks::worktree::WorktreeDelete,
    sessions: &HashMap<(String, u64), EmbeddedSession>,
) {
    if sessions.values().any(|s| s.worktree() == order.path) {
        app.status = "an agent is still running in that worktree".to_string();
        return;
    }
    match gtasks::worktree::remove_tree(binaries, &order) {
        Ok(()) => {
            for agent in app.tab_agents.values_mut() {
                if agent
                    .record
                    .as_ref()
                    .is_some_and(|r| r.worktree == order.path)
                {
                    agent.record = None;
                    agent.dismissed_for = None;
                }
            }
            app.status = format!(
                "deleted worktree {} (branch {} kept)",
                order.path.display(),
                order.branch
            );
        }
        Err(err) => {
            app.status = format!("cannot delete the worktree: {err:#}");
        }
    }
}

/// The tree the plan wants, from repo + branch: used when the picked tree
/// turned out to be on the wrong branch, to offer building the right one.
/// `None` when the names do not parse — the plan itself came from validated
/// names, so this only fires on state that changed under our feet.
fn correct_tree_path(app: &App, plan: &AgentPlan) -> Option<PathBuf> {
    let root = app.worktree_root.clone()?;
    let slug = RepoSlug::parse(&plan.repo).ok()?;
    let branch = gtasks::worktree::BranchName::parse(&plan.branch).ok()?;
    gtasks::worktree::resolve_in(&root, &plan.clone, &slug, &branch).ok()
}

/// Run the handoff the user validated: seed file plus an agent spawned
/// embedded in the agent box. The TUI never suspends; the child lives in a
/// pty sized to the box, and the loop polls it every tick.
///
/// The agent never starts working on its own: it opens on an empty prompt
/// and the loop types the seed as a draft the user reviews and sends.
/// One Enter to start is cheaper than one run in the wrong direction.
fn launch_agent(
    app: &mut App,
    plan: &AgentPlan,
    path: &std::path::Path,
    term_size: (u16, u16),
    sessions: &mut HashMap<(String, u64), EmbeddedSession>,
    binaries: &Binaries,
    timeout: Duration,
) {
    let key = App::tab_key(&plan.repo, plan.number);
    // The tree may have drifted onto another branch since it was made
    // (renamed outside, checked out by hand): starting the agent there
    // works on the wrong code. Offer the fix instead of starting
    // silently: build the tree the plan wants and launch there. `n` in
    // the menu does the same; this catches an Enter on the wrong row.
    match gtasks::worktree::current_branch(binaries, path, timeout) {
        Some(have) if have != plan.branch => {
            match correct_tree_path(app, plan) {
                Some(target) => {
                    let question = format!(
                        "tree {} is on branch {have}, not {} — create {} and launch there?",
                        path.display(),
                        plan.branch,
                        target.display()
                    );
                    app.modal = Some(Modal::Confirm(question));
                    app.pending = Some(Pending::Agent(plan.clone(), target));
                }
                None => {
                    app.status = format!(
                        "tree {} is on branch {have}, not {} — fix the branch or delete the tree and relaunch",
                        path.display(),
                        plan.branch
                    );
                }
            }
            return;
        }
        _ => {}
    }
    let seeds = match gtasks::agent::seeds_dir() {
        Ok(seeds) => seeds,
        Err(err) => {
            app.status = format!("cannot prepare the seed directory: {err:#}");
            return;
        }
    };
    let body = agent::agent_seed_body(&plan.repo, plan.number, &plan.url, None);
    let lines = body.lines().count();
    let seed = match gtasks::agent::SeedFile::create_in(&seeds, &body) {
        Ok(seed) => seed,
        Err(err) => {
            app.status = format!("cannot write the agent seed: {err:#}");
            return;
        }
    };
    let argv = {
        // Sandboxed: the `<agent>-sandboxed` wrapper next to the resolved
        // binary. A wrapper that was never installed refuses here, naming
        // itself — launching the plain binary instead would silently drop
        // the containment the setting promises.
        let program = if app.sandbox_agents {
            match agent::sandboxed_program(&plan.agent.program) {
                Some(wrapped) => wrapped,
                None => {
                    let wrapper = plan.agent.program.file_name().map(|stem| {
                        let mut name = stem.to_os_string();
                        name.push("-sandboxed");
                        name.to_string_lossy().into_owned()
                    });
                    app.status = format!(
                        "sandbox_agents is on but {} is not installed",
                        wrapper.as_deref().unwrap_or("the wrapper")
                    );
                    return;
                }
            }
        } else {
            plan.agent.program.clone()
        };
        // No initial prompt, ever: the agent opens on an empty prompt and
        // the loop types the seed as a draft the user reviews and sends.
        agent::agent_argv_for_draft(
            plan.agent.id,
            &program,
            &plan.profile,
            path,
            app.sandbox_agents,
        )
    };
    // The box the pty is sized to: the same geometry the draw uses, so the
    // terminal and its box never disagree by a row.
    let outer = ui::agent_box(
        Rect::new(0, 0, term_size.0, term_size.1),
        app.col_pct,
        app.row_pct,
    );
    let cols = outer.width.saturating_sub(2);
    let rows = outer.height.saturating_sub(2);
    let parent: std::collections::HashMap<String, String> = std::env::vars().collect();
    match EmbeddedSession::spawn(
        &argv,
        seed,
        lines,
        plan.repo.clone(),
        plan.number,
        plan.profile.clone(),
        plan.agent.id.name().to_string(),
        path.to_path_buf(),
        cols,
        rows,
        &parent,
    ) {
        Ok(sess) => {
            // The agent lives in its item's tab: ensure the tab, show it,
            // and file this session under it. Other tabs keep running.
            if !app
                .tabs
                .iter()
                .any(|tab| tab.repo == plan.repo && tab.number == plan.number)
            {
                app.tabs.push(gtasks::app::Tab {
                    repo: plan.repo.clone(),
                    number: plan.number,
                });
            }
            if let Some(idx) = app
                .tabs
                .iter()
                .position(|tab| tab.repo == plan.repo && tab.number == plan.number)
            {
                app.activate_tab(idx);
            }
            let screen = sess.snapshot();
            let entry = app.tab_agents.entry(key.clone()).or_default();
            entry.running = true;
            entry.screen = screen;
            entry.record = Some(gtasks::agent::AgentRecord {
                repo: plan.repo.clone(),
                number: plan.number,
                profile: plan.profile.clone(),
                agent: plan.agent.id.name().to_string(),
                worktree: path.to_path_buf(),
                seed_lines: lines,
                status: gtasks::agent::AgentStatus::Running,
            });
            entry.dismissed_for = None;
            app.pane = gtasks::app::Pane::Agent;
            // The detail pane shows where the agent works while no
            // issue/PR is selected; without it the right column above the
            // box goes blank on repos with no rows.
            entry.context = Some(gtasks::app::AgentContext {
                repo: plan.repo.clone(),
                branch: plan.branch.clone(),
                path: path.to_path_buf(),
            });
            // The prompt is typed, not sent: the agent opens ready, and
            // the user reviews the draft and presses Enter. The seed file
            // still travels to the record so the run is reproducible.
            entry.draft = Some(body);
            app.status = "agent ready — review the draft, edit it, Enter sends it".to_string();
            sessions.insert(key, sess);
        }
        Err(err) => app.status = format!("cannot run the agent: {err:#}"),
    }
}

/// Start one batched fetch in the background. Dropping the receiver with the app
/// means a late result is simply discarded.
fn spawn_fetch(
    binaries: Binaries,
    repos: Vec<RepoSlug>,
    limit: usize,
    timeout: Duration,
    parallel: usize,
) -> Receiver<FetchOutcome> {
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let outcome = fetch::fetch_graphql_many(&binaries, &repos, limit, timeout, parallel);
        let _ = tx.send(outcome);
    });
    rx
}

/// Start one body fetch in the background.
fn spawn_detail(
    binaries: Binaries,
    repo: String,
    number: u64,
    kind: ItemKind,
    timeout: Duration,
) -> Receiver<DetailResult> {
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let result = fetch::fetch_detail(&binaries, &repo, kind, number, timeout);
        let _ = tx.send(result);
    });
    rx
}

/// What a body fetch returns, and the tuple kept while it is in flight.
type DetailResult = Result<String, fetch::FetchError>;
type PendingDetail = (String, u64, Receiver<DetailResult>);
/// The item's `updated_at` for the cache: a body cached under this stamp is
/// current, anything else is stale. `None` when the item left the list —
/// the fetch then runs and fails honestly instead of trusting the disk.
fn item_updated_at(app: &App, repo: &str, number: u64) -> Option<String> {
    app.items
        .iter()
        .find(|item| item.repo == repo && item.number == number)
        .map(|item| item.updated_at.clone())
}

/// What a diff fetch returns, and the tuple kept while it is in flight.
type DiffResult = Result<String, fetch::FetchError>;
type PendingDiff = (String, u64, Receiver<DiffResult>);

fn spawn_diff(
    binaries: Binaries,
    repo: String,
    number: u64,
    timeout: Duration,
) -> Receiver<DiffResult> {
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let result = fetch::fetch_diff(&binaries, &repo, number, timeout);
        let _ = tx.send(result);
    });
    rx
}

fn run_tui() -> anyhow::Result<()> {
    install_signal_handlers();

    // One owner for the terminal: `Tui::enter` enables raw mode and the alternate screen,
    // installs the chaining panic hook, and restores exactly once on every exit path — normal
    // return, `?`, unwinding, and the half-entry where `Self` is never constructed. The hook
    // that used to be installed here by hand is now a client of the guard (`gtasks::tui`),
    // because two mechanisms for one fact is the polite form of a defect.
    let _guard = gtasks::tui::Tui::enter()?;
    let mut terminal = Terminal::new(CrosstermBackend::new(io::stdout()))?;

    let binaries = match Binaries::detect() {
        Ok(binaries) => binaries,
        Err(err) => anyhow::bail!("gh and git are required: {err:#}"),
    };

    let (mut config, config_notice) = load_config();
    // The notice is dismissed with `Esc`; this flag is what the legend keeps saying afterwards,
    // for as long as the program runs on a repository nobody chose.
    let config_fallback = config_notice.is_some();
    let timeout = Duration::from_secs(config.timeout_s.max(1));
    let parallel = config.parallel;
    let limit = config.limit as usize;

    let mut app = App {
        loading: true,
        status: "loading…".to_string(),
        // Resolved once at startup: the same paths the fetch uses.
        binaries: Some(binaries.clone()),
        write_timeout: timeout,
        agent_profile: config.agent_profile.clone(),
        sandbox_agents: config.sandbox_agents,
        font_size: config.font_size,
        font_family: config.font_family.clone(),
        fonts: gtasks::config::mono_fonts(),
        clone_roots: config.agent_roots(),
        config_fallback,
        // The list icons and the worktree picker read it; `None` means no
        // icons, and the picker explains instead of guessing.
        worktree_root: config
            .worktree_root
            .clone()
            .or_else(|| gtasks::worktree::default_root().ok()),
        ..App::default()
    };
    // A broken config is a notice, not a whisper in the log: it stays on
    // screen until dismissed, while the fetch runs behind it.
    if let Some(notice) = config_notice {
        app.modal = Some(gtasks::app::Modal::Notice(notice));
    }
    // The terminal owns the glyphs: show what Foot really uses, not what the
    // config file says — the two drift when the file is hand-edited. A
    // no-op toggle then writes back exactly what is there.
    if let Some(path) = gtasks::config::foot_ini_path() {
        if let Some((family, px)) = gtasks::config::read_foot_font(&path) {
            app.font_family = family;
            app.font_size = gtasks::config::FontSize::from_px(px);
        }
    }
    // The pane lists what the user configured, including repos with no items.
    // A saved selection narrows it; an empty one means "everything". Both
    // lists are validated `RepoSlug`s from here on — `config` was checked at
    // load, and a hand-edited state file only loses the entries that do not
    // parse instead of breaking the startup.
    let state_path = state_path();
    let mut state = match state_path.as_deref() {
        Some(path) => match State::load(path) {
            Ok(state) => state,
            Err(err) => {
                tracing::warn!(path = %path.display(), "bad state file, starting fresh: {err:#}");
                State::default()
            }
        },
        None => State::default(),
    };
    let all: Vec<RepoSlug> = config
        .repos
        .iter()
        .map(|repo| RepoSlug::parse(repo).expect("config validated at load"))
        .collect();
    let mut skipped = 0;
    let selected: Vec<RepoSlug> = state
        .selected_repos
        .iter()
        .filter_map(|repo| match RepoSlug::parse(repo) {
            Ok(slug) => Some(slug),
            Err(_) => {
                skipped += 1;
                None
            }
        })
        .collect();
    if skipped > 0 {
        tracing::warn!(skipped, "state file holds invalid slugs, ignored");
    }
    let shown = if selected.is_empty() {
        all.clone()
    } else {
        selected
    };
    app.set_repos(shown);
    app.all_repos = all.clone();
    // A temporary exit keeps the session: pane, cursors, filter, query and
    // open tabs come back from `state.json`. Unknown values fall back
    // instead of breaking startup.
    app.restore_session(&state);
    // The items cache paints instantly; the fetch below refreshes in the
    // background. A broken cache is ignored — the fetch is the source of
    // truth, the cache only speed.
    let cache_path = app_dir().map(|base| base.join("items-cache.json"));
    // Bodies and diffs, one file per item: shown at once on selection,
    // revalidated in the background when the list moved on.
    let detail_cache_dir = app_dir().map(|base| base.join("details"));
    let mut cached_items: Option<Vec<gtasks::model::Item>> =
        cache_path.as_deref().and_then(gtasks::cache::load);
    if let Some(items) = cached_items.clone() {
        app.items = items;
        app.set_lengths(app.repos.len(), app.visible_items().len());
        app.refresh_status();
        app.status = format!("{} — cached, refreshing…", app.status);
    }
    let mut pending: Option<Receiver<FetchOutcome>> = Some(spawn_fetch(
        binaries.clone(),
        App::fetch_scope(&all, &state.selected_repos),
        limit,
        timeout,
        parallel,
    ));
    // Last completed-or-started refresh: the auto-refresh door below measures
    // `refresh_s` from here, so the first automatic re-read lands one full
    // interval after startup, never on top of the initial fetch.
    let mut last_auto_refresh = std::time::Instant::now();
    let mut pending_detail: Option<PendingDetail> = None;
    let mut pending_diff: Option<PendingDiff> = None;
    // A repo clone in flight, if the user agreed to one: repo plus result.
    let mut pending_clone: Option<Receiver<(String, anyhow::Result<std::path::PathBuf>)>> = None;
    // Live agent sessions, one per tab: the loop owns the children, the
    // App only carries what the screen needs. Switching tabs switches
    // boxes; background tabs keep running.
    let mut sessions: HashMap<(String, u64), EmbeddedSession> = HashMap::new();
    // Draw only on change: view + paint every tick is what caps the fps
    // budget, so background completions, events and fresh agent output
    // raise this instead.
    let mut needs_draw = true;

    while !app.quit && !TERMINATE.load(Ordering::SeqCst) {
        // Take a finished fetch, if any.
        if let Some(rx) = &pending {
            match rx.try_recv() {
                Ok(outcome) => {
                    // Freshness check: unchanged means the screen and the
                    // cache stay; changed means both are replaced together.
                    let changed = cached_items
                        .as_ref()
                        .map(|cached| !gtasks::cache::is_fresh(cached, &outcome.items))
                        .unwrap_or(true);
                    app.apply_outcome(outcome);
                    if changed {
                        if let Some(path) = cache_path.as_deref() {
                            match gtasks::cache::save(path, &app.items) {
                                Ok(()) => cached_items = Some(app.items.clone()),
                                Err(err) => {
                                    tracing::warn!("cannot save the items cache: {err:#}");
                                    cached_items = Some(app.items.clone());
                                }
                            }
                        }
                    } else {
                        app.status = format!("up to date — {}", app.status);
                    }
                    pending = None;
                    needs_draw = true;
                    // No prefetch: the detail body is fetched only on explicit
                    // request (Enter). The list already shows every field.
                }
                Err(TryRecvError::Empty) => {}
                Err(TryRecvError::Disconnected) => {
                    app.loading = false;
                    app.status = "fetch ended unexpectedly".to_string();
                    pending = None;
                    needs_draw = true;
                }
            }
        }

        // A body fetch that finished.
        let mut detail_result: Option<DetailResult> = None;
        let mut detail_gone = false;
        if let Some((_, _, rx)) = &pending_detail {
            match rx.try_recv() {
                Ok(result) => detail_result = Some(result),
                Err(TryRecvError::Empty) => {}
                Err(TryRecvError::Disconnected) => detail_gone = true,
            }
        }
        if detail_gone {
            pending_detail = None;
            app.detail_requested = None;
            needs_draw = true;
        }
        if let Some(result) = detail_result {
            if let Some((repo, number, _)) = pending_detail.take() {
                needs_draw = true;
                match result {
                    Ok(body) => {
                        // The fetch is the source of truth: it replaces the
                        // cache, stale or not, before painting.
                        if let Some(dir) = detail_cache_dir.as_deref() {
                            if let Some(stamp) = item_updated_at(&app, &repo, number) {
                                if let Err(err) =
                                    gtasks::cache::save_detail(dir, &repo, number, &stamp, &body)
                                {
                                    tracing::warn!("cannot save the body cache: {err:#}");
                                }
                            }
                        }
                        app.apply_detail(&repo, number, body);
                    }
                    Err(err) => {
                        // Do not leave the request pending: it would be retried
                        // on every loop iteration.
                        app.detail_requested = None;
                        app.status = format!("detail failed: {err}");
                    }
                }
            }
        }

        // A diff fetch that finished.
        let mut diff_result: Option<DiffResult> = None;
        let mut diff_gone = false;
        if let Some((_, _, rx)) = &pending_diff {
            match rx.try_recv() {
                Ok(result) => diff_result = Some(result),
                Err(TryRecvError::Empty) => {}
                Err(TryRecvError::Disconnected) => diff_gone = true,
            }
        }
        if diff_gone {
            pending_diff = None;
            app.diff_requested = None;
            needs_draw = true;
        }
        if let Some(result) = diff_result {
            if let Some((repo, number, _)) = pending_diff.take() {
                needs_draw = true;
                match result {
                    Ok(text) => {
                        if let Some(dir) = detail_cache_dir.as_deref() {
                            if let Some(stamp) = item_updated_at(&app, &repo, number) {
                                if let Err(err) =
                                    gtasks::cache::save_diff(dir, &repo, number, &stamp, &text)
                                {
                                    tracing::warn!("cannot save the diff cache: {err:#}");
                                }
                            }
                        }
                        app.apply_diff(&repo, number, Ok(text));
                    }
                    Err(err) => {
                        // Do not leave the request pending: it would be retried
                        // on every loop iteration. The modal stays open showing
                        // the failure (apply_diff writes it), so the status
                        // line echo is for the record, not the discovery.
                        app.status = format!("diff failed: {err}");
                        app.apply_diff(&repo, number, Err(err));
                    }
                }
            }
        }

        // A settings toggle: the key handler flips the flag, the loop writes
        // `config.toml` (0600, atomic, like every private write). A broken
        // config is never overwritten — fix it first, then toggle. Font
        // toggles also rewrite the Foot `font=` line, but only when a font
        // row moved: a sandbox-only toggle never touches the terminal.
        if app.config_save_requested {
            app.config_save_requested = false;
            needs_draw = true;
            if config_fallback {
                app.status =
                    "the config file is broken — fix it before saving settings".to_string();
                app.font_save_requested = false;
            } else if let Some(path) = config_path() {
                config.sandbox_agents = app.sandbox_agents;
                config.font_size = app.font_size;
                config.font_family = app.font_family.clone();
                match config.save(&path) {
                    Ok(()) => {
                        app.status = format!("settings saved to {}", path.display());
                        if app.font_save_requested {
                            app.font_save_requested = false;
                            match gtasks::config::foot_ini_path() {
                                Some(foot) => {
                                    match gtasks::config::set_foot_font(
                                        &foot,
                                        &app.font_family,
                                        app.font_size.px(),
                                    ) {
                                        Ok(true) => {
                                            app.status.push_str(
                                                " · font saved — restart the terminal to apply",
                                            );
                                        }
                                        Ok(false) => {
                                            app.status.push_str(" · font already set");
                                        }
                                        Err(err) => {
                                            app.status = format!("cannot save the font: {err:#}");
                                        }
                                    }
                                }
                                None => {
                                    app.status =
                                        "cannot save the font: no config directory".to_string();
                                }
                            }
                        }
                    }
                    Err(err) => {
                        app.status = format!("cannot save the config: {err:#}");
                        app.font_save_requested = false;
                    }
                }
            } else {
                app.status = "cannot save the settings: no config directory".to_string();
                app.font_save_requested = false;
            }
        }

        // `R`: the key handler already updated the live list; the file is
        // the loop's job, atomically and 0600, like every private write.
        // An empty check means "everything", like an empty state file: one
        // meaning, said out loud — not "0 repos" with all of them shown.
        // The session (pane, cursors, tabs, filter) travels with it, so a
        // temporary exit keeps what the user was doing.
        if let Some(selected) = app.selection_applied.take() {
            needs_draw = true;
            let cleared = selected.is_empty();
            state.selected_repos = selected;
            app.capture_session(&mut state);
            match state_path.as_deref() {
                Some(path) => match state.save(path) {
                    Ok(()) => {
                        // Both sentences are computed from what the panel draws,
                        // never narrated beside it: a status line that reads the
                        // same collection cannot contradict the list. The base
                        // is recomputed first, so the transient below describes
                        // the same list the panel draws.
                        app.refresh_status();
                        app.status = app.selection_summary(cleared);
                        app.session_dirty = false;
                    }
                    Err(err) => app.status = format!("cannot save selection: {err:#}"),
                },
                None => app.status = "selection applied, not saved (no state file)".to_string(),
            }
        }

        // Session autosave: any key that moved a cursor, a filter or a tab
        // sets `session_dirty`; the loop persists it so quitting mid-task
        // keeps tabs, filter and positions.
        if app.session_dirty && app.selection_applied.is_none() {
            app.capture_session(&mut state);
            match state_path.as_deref() {
                Some(path) => match state.save(path) {
                    Ok(()) => app.session_dirty = false,
                    Err(err) => {
                        app.status = format!("cannot save session: {err:#}");
                        app.session_dirty = false;
                        needs_draw = true;
                    }
                },
                None => app.session_dirty = false,
            }
        }

        // `o`: the key handler already validated the URL.
        if let Some(url) = app.open_requested.take() {
            needs_draw = true;
            match &binaries.opener {
                Some(program) => match actions::open_in_browser(program, &url) {
                    Ok(pid) => app.status = format!("opened in the browser (pid {pid})"),
                    Err(err) => app.status = format!("cannot open: {err:#}"),
                },
                None => app.status = "no browser launcher found".to_string(),
            }
        }

        // A request from the key handler. The disk is asked first: a body
        // cached for this `updated_at` paints at once with no fetch, a
        // stale one paints at once and refetches behind.
        if pending_detail.is_none() {
            if let Some((repo, number)) = app.detail_requested.clone() {
                let stamp = item_updated_at(&app, &repo, number);
                let cached = detail_cache_dir
                    .as_deref()
                    .and_then(|dir| gtasks::cache::load_detail(dir, &repo, number));
                match (stamp, cached) {
                    (Some(stamp), cached) => match gtasks::cache::resolve(cached, &stamp) {
                        gtasks::cache::Resolve::Fresh(text) => {
                            app.apply_detail(&repo, number, text);
                        }
                        gtasks::cache::Resolve::Stale(text) => {
                            app.apply_detail(&repo, number, text);
                            // The apply above clears the request; raise it
                            // again for the revalidation below.
                            app.detail_requested = Some((repo.clone(), number));
                            let kind = app.item_kind(&repo, number);
                            pending_detail = Some((
                                repo.clone(),
                                number,
                                spawn_detail(binaries.clone(), repo, number, kind, timeout),
                            ));
                        }
                        gtasks::cache::Resolve::Missing => {
                            let kind = app.item_kind(&repo, number);
                            pending_detail = Some((
                                repo.clone(),
                                number,
                                spawn_detail(binaries.clone(), repo, number, kind, timeout),
                            ));
                        }
                    },
                    (None, _) => {
                        let kind = app.item_kind(&repo, number);
                        pending_detail = Some((
                            repo.clone(),
                            number,
                            spawn_detail(binaries.clone(), repo, number, kind, timeout),
                        ));
                    }
                }
            }
        }

        // A diff request from the key handler. Same disk-first rule as the
        // body: a cached diff for this `updated_at` opens at once, a stale
        // one opens at once and refetches behind.
        if pending_diff.is_none() {
            if let Some((repo, number)) = app.diff_requested.clone() {
                let stamp = item_updated_at(&app, &repo, number);
                let cached = detail_cache_dir
                    .as_deref()
                    .and_then(|dir| gtasks::cache::load_diff(dir, &repo, number));
                match (stamp, cached) {
                    (Some(stamp), cached) => match gtasks::cache::resolve(cached, &stamp) {
                        gtasks::cache::Resolve::Fresh(text) => {
                            app.apply_diff(&repo, number, Ok(text));
                        }
                        gtasks::cache::Resolve::Stale(text) => {
                            app.apply_diff(&repo, number, Ok(text));
                            app.diff_requested = Some((repo.clone(), number));
                            pending_diff = Some((
                                repo.clone(),
                                number,
                                spawn_diff(binaries.clone(), repo, number, timeout),
                            ));
                        }
                        gtasks::cache::Resolve::Missing => {
                            pending_diff = Some((
                                repo.clone(),
                                number,
                                spawn_diff(binaries.clone(), repo, number, timeout),
                            ));
                        }
                    },
                    (None, _) => {
                        pending_diff = Some((
                            repo.clone(),
                            number,
                            spawn_diff(binaries.clone(), repo, number, timeout),
                        ));
                    }
                }
            }
        }

        // One refresh in flight at a time (single-flight).
        // Periodic refresh: the list re-reads itself every `refresh_s`
        // seconds (`0` = off). Same door as `r`, quieter rules: never overlap
        // an in-flight fetch, never move the list under an open dialog.
        if gtasks::app::auto_refresh_due(
            last_auto_refresh,
            std::time::Instant::now(),
            config.refresh_s,
            app.loading || app.modal.is_some() || pending.is_some() || app.refresh_requested,
        ) {
            last_auto_refresh = std::time::Instant::now();
            app.loading = true;
            app.refresh_requested = true;
            app.status = "refreshing…".to_string();
            needs_draw = true;
        }
        if app.refresh_requested && pending.is_none() {
            app.refresh_requested = false;
            pending = Some(spawn_fetch(
                binaries.clone(),
                App::fetch_scope(&all, &state.selected_repos),
                limit,
                timeout,
                parallel,
            ));
        }

        // `g` (validated by the key handler) and `y` (agreed worktree
        // creation): the loop owns git, the seed file and the pty child.
        let term_size = terminal
            .size()
            .map(|size| (size.width, size.height))
            .unwrap_or((80, 24));
        if let Some(plan) = app.agent_requested.take() {
            needs_draw = true;
            start_agent(
                &mut app,
                &binaries,
                &config,
                plan,
                None,
                term_size,
                &mut sessions,
            );
        }
        if let Some((plan, path)) = app.agent_go.take() {
            needs_draw = true;
            start_agent(
                &mut app,
                &binaries,
                &config,
                plan,
                Some(path),
                term_size,
                &mut sessions,
            );
        }
        if let Some((plan, path)) = app.agent_place.take() {
            needs_draw = true;
            start_placed(
                &mut app,
                plan,
                path,
                term_size,
                &mut sessions,
                &binaries,
                timeout,
            );
        }
        if let Some(order) = app.rename_go.take() {
            needs_draw = true;
            rename_worktree(&mut app, &binaries, order, &sessions);
        }
        if let Some(order) = app.delete_go.take() {
            needs_draw = true;
            delete_worktree(&mut app, &binaries, order, &sessions);
        }
        // A clone runs in a thread — network can take minutes, and the UI
        // must stay alive meanwhile. When it lands, the W flow continues
        // where it asked: the clone is there now, so the name prompt opens.
        if let Some((repo, path)) = app.clone_go.take() {
            let (tx, rx) = mpsc::channel();
            let binaries = binaries.clone();
            std::thread::spawn(move || {
                let slug = match gtasks::actions::RepoSlug::parse(&repo) {
                    Ok(slug) => slug,
                    Err(_) => {
                        let _ = tx.send((repo, Err(anyhow::anyhow!("invalid repo slug"))));
                        return;
                    }
                };
                let done = gtasks::worktree::clone_repo(&binaries, &path, &slug).map(|_| path);
                let _ = tx.send((repo, done));
            });
            pending_clone = Some(rx);
        }
        // Take a finished clone, if any.
        if let Some(rx) = &pending_clone {
            match rx.try_recv() {
                Ok((repo, Ok(path))) => {
                    pending_clone = None;
                    needs_draw = true;
                    app.status = format!("cloned into {} — growing the tree", path.display());
                    app.open_repo_worktree_for(&repo);
                }
                Ok((repo, Err(err))) => {
                    pending_clone = None;
                    needs_draw = true;
                    app.status = format!("cannot clone {repo}: {err:#}");
                }
                Err(mpsc::TryRecvError::Empty) => {}
                Err(mpsc::TryRecvError::Disconnected) => {
                    pending_clone = None;
                    needs_draw = true;
                    app.status = "clone thread died — try again".to_string();
                }
            }
        }

        // Every live session, every tick: background tabs keep streaming
        // while another is on screen. Only the active tab's box is sized
        // to the pty — a hidden box has no geometry to match.
        let active_key = app.active_key();
        let outer = ui::agent_box(
            Rect::new(0, 0, term_size.0, term_size.1),
            app.col_pct,
            app.row_pct,
        );
        let mut exited: Vec<((String, u64), String)> = Vec::new();
        for (key, sess) in sessions.iter_mut() {
            if Some(key) == active_key.as_ref() {
                sess.resize(
                    outer.width.saturating_sub(2),
                    outer.height.saturating_sub(2),
                );
            }
            if sess.drain() {
                needs_draw = true;
                if let Some(agent) = app.tab_agents.get_mut(key) {
                    agent.screen = sess.snapshot();
                }
            }
            if let Some(record) = sess.poll_exit() {
                exited.push((key.clone(), record.status.summary()));
                if let Some(agent) = app.tab_agents.get_mut(key) {
                    // The finish is announced in the status line; the detail
                    // returns at once instead of gluing the finished card to
                    // the item. A new run clears the dismissal at launch.
                    agent.dismissed_for = Some((record.repo.clone(), record.number));
                    agent.record = Some(record);
                    agent.running = false;
                    agent.screen = Vec::new();
                    agent.context = None;
                    agent.draft = None;
                }
            }
        }
        for (key, summary) in exited {
            sessions.remove(&key);
            needs_draw = true;
            if Some(&key) == active_key.as_ref() {
                app.status = format!("agent finished: {summary}");
            }
        }
        // A tab closed under a live session: the agent dies with its box
        // instead of working headless. The close message already said the
        // tab is gone; nothing more needs announcing.
        let live_keys: Vec<(String, u64)> = sessions.keys().cloned().collect();
        for key in live_keys {
            if !app
                .tabs
                .iter()
                .any(|tab| (tab.repo.clone(), tab.number) == key)
            {
                if let Some(mut sess) = sessions.remove(&key) {
                    sess.kill();
                }
                app.tab_agents.remove(&key);
            }
        }

        // Fresh launches type their drafts: 1KB per tick so a long seed
        // never blocks the loop on a full pty. The agent boots while the
        // first chunks queue up, then reads them as its input.
        let draft_keys: Vec<(String, u64)> = app
            .tab_agents
            .iter()
            .filter(|(_, agent)| agent.draft.is_some())
            .map(|(key, _)| key.clone())
            .collect();
        for key in draft_keys {
            let text = app
                .tab_agents
                .get_mut(&key)
                .and_then(|agent| agent.draft.take());
            if let Some(mut text) = text {
                if let Some(sess) = sessions.get_mut(&key) {
                    // Byte chunks must end on a char boundary, or a multibyte
                    // character split across two ticks renders as mojibake.
                    let mut n = text.len().min(1024);
                    while !text.is_char_boundary(n) {
                        n -= 1;
                    }
                    let rest = text.split_off(n);
                    sess.write(text.as_bytes());
                    if !rest.is_empty() {
                        if let Some(agent) = app.tab_agents.get_mut(&key) {
                            agent.draft = Some(rest);
                        }
                    }
                } else {
                    // No session to type into (a launch failed between setting
                    // the draft and spawning): keep it. Dropping the user's
                    // direction would be worse than retrying it on the next run,
                    // which overwrites it anyway.
                    if let Some(agent) = app.tab_agents.get_mut(&key) {
                        agent.draft = Some(text);
                    }
                }
            }
        }

        // Paint only on change: a full view + paint every tick is what caps
        // the fps budget. Background completions, events and fresh agent
        // output raise `needs_draw` instead — an idle screen costs one poll.
        if needs_draw {
            needs_draw = false;
            let view = ui::view_for(&app);
            terminal.draw(|frame| ui::draw(frame, &app, &view))?;
        }
        let tick = if sessions.is_empty() { 250 } else { 16 };
        if event::poll(Duration::from_millis(tick))? {
            needs_draw = true;
            match event::read()? {
                Event::Key(key) => {
                    // Focused on a running agent, every key but Shift+arrows
                    // is agent input — including Tab, Esc and Ctrl-C, which
                    // interrupts it. Shift+arrows move between boxes (the
                    // guaranteed way out): all through the app. Anywhere
                    // else, keys are app keys (where Ctrl-C quits).
                    //
                    // Dialogs, search and help still own Esc/Tab while open;
                    // outside the agent box every key stays app input.
                    let app_owns_key = app.modal.is_some() || app.searching || app.show_help;
                    let to_app = matches!(
                        map_key(key),
                        Some(Msg::Key(
                            Key::ShiftUp | Key::ShiftDown | Key::ShiftLeft | Key::ShiftRight
                        ))
                    ) || (matches!(
                        map_key(key),
                        Some(Msg::Key(Key::Esc | Key::Tab | Key::BackTab))
                    ) && (app_owns_key || app.pane != gtasks::app::Pane::Agent));
                    // Note what is missing: Tab and Esc. While the box has
                    // focus and no dialog, search or help is open, they
                    // belong to the agent (completion, vim mode, popups) —
                    // Shift+arrows leave the box.
                    let active_key = app.active_key();
                    let session_live = active_key.as_ref().is_some_and(|key| {
                        sessions.contains_key(key)
                            && app.tab_agents.get(key).is_some_and(|agent| agent.running)
                    });
                    let to_agent = session_live && app.pane == gtasks::app::Pane::Agent && !to_app;
                    if to_agent {
                        if let Some(Msg::Key(k)) = map_key(key) {
                            if let Some(bytes) = gtasks::embedded::key_to_pty(&k) {
                                if let Some(skey) = active_key {
                                    if let Some(sess) = sessions.get_mut(&skey) {
                                        sess.write(&bytes);
                                    }
                                }
                            }
                        }
                    } else if let Some(msg) = map_key(key) {
                        app.update(msg);
                    }
                }
                // Resizes only need a redraw, which the loop does anyway.
                Event::Resize(_, _) => {}
                // Clicking a tab opens it: the same strip draw renders, mapped
                // back through the shared geometry. Clicks while a dialog is
                // open are ignored — the dialog owns every input then.
                // The wheel scrolls the agent box through history — never
                // the agent's input: with mouse capture on, the terminal no
                // longer turns the wheel into arrow keys.
                Event::Mouse(me) => {
                    if matches!(app.modal, Some(Modal::Diff)) {
                        // The diff owns the wheel while open.
                        match me.kind {
                            MouseEventKind::ScrollUp => {
                                app.diff_scroll = app.diff_scroll.saturating_add(3)
                            }
                            MouseEventKind::ScrollDown => {
                                app.diff_scroll = app.diff_scroll.saturating_sub(3)
                            }
                            _ => {}
                        }
                    } else if app.modal.is_none() {
                        let area = Rect::new(0, 0, term_size.0, term_size.1);
                        let g = ui::grid(area, app.col_pct, app.row_pct);
                        match me.kind {
                            MouseEventKind::Down(MouseButton::Left) => {
                                // Divider grabs win over tab clicks: two
                                // border cells per line, the cross only.
                                if (me.column == g.div_x.saturating_sub(1) || me.column == g.div_x)
                                    && me.row >= g.body.y
                                    && me.row < g.body.y.saturating_add(g.body.height)
                                {
                                    app.drag = Some(gtasks::app::DragLine::Cols);
                                } else if (me.row == g.div_y.saturating_sub(1) || me.row == g.div_y)
                                    && me.column >= g.body.x
                                    && me.column < g.body.x.saturating_add(g.body.width)
                                {
                                    app.drag = Some(gtasks::app::DragLine::Rows);
                                } else if let Some(idx) = ui::tab_at(&app, area, me.column, me.row)
                                {
                                    app.activate_tab(idx);
                                }
                            }
                            MouseEventKind::Drag(MouseButton::Left) => {
                                // Move the divider the drag holds; single
                                // boxes are never sized directly. Pixel
                                // floors keep every box usable, percents
                                // persist through `state.json` on autosave.
                                // (crossterm reports the drag button only;
                                // the position rides on the event itself.)
                                let (x, y) = (me.column, me.row);
                                match app.drag {
                                    Some(gtasks::app::DragLine::Cols) => {
                                        if let Some(pct) = ui::col_pct_for(g.body, x) {
                                            app.col_pct = pct;
                                            app.session_dirty = true;
                                        }
                                    }
                                    Some(gtasks::app::DragLine::Rows) => {
                                        if let Some(pct) = ui::row_pct_for(g.body, y) {
                                            app.row_pct = pct;
                                            app.session_dirty = true;
                                        }
                                    }
                                    None => {}
                                }
                            }
                            MouseEventKind::Up(_) => {
                                app.drag = None;
                            }
                            _ => {}
                        }
                    }
                    if !matches!(app.modal, Some(Modal::Diff)) {
                        let box_shown = app.agent_active() || app.pane == gtasks::app::Pane::Agent;
                        match me.kind {
                            MouseEventKind::ScrollUp if box_shown => {
                                if let Some(key) = app.active_key() {
                                    if let Some(sess) = sessions.get_mut(&key) {
                                        sess.scroll(3);
                                        let screen = sess.snapshot();
                                        if let Some(agent) = app.tab_agents.get_mut(&key) {
                                            agent.screen = screen;
                                        }
                                    } else if let Some(agent) = app.tab_agents.get_mut(&key) {
                                        agent.scroll = agent.scroll.saturating_add(3);
                                    }
                                }
                            }
                            MouseEventKind::ScrollDown if box_shown => {
                                if let Some(key) = app.active_key() {
                                    if let Some(sess) = sessions.get_mut(&key) {
                                        sess.scroll(-3);
                                        let screen = sess.snapshot();
                                        if let Some(agent) = app.tab_agents.get_mut(&key) {
                                            agent.screen = screen;
                                        }
                                    } else if let Some(agent) = app.tab_agents.get_mut(&key) {
                                        agent.scroll = agent.scroll.saturating_sub(3);
                                    }
                                }
                            }
                            _ => {}
                        }
                    }
                }
                _ => {}
            }
        } else {
            app.update(Msg::Tick);
        }
    }

    // Kill every live agent: quitting must not leave an agent running
    // without a box. Seed files are unlinked on drop either way.
    for (_, mut sess) in sessions {
        sess.kill();
    }

    // Last save on the way out: a temporary exit must keep the session even
    // when the autosave above never ran (quit right after a fetch, …).
    app.capture_session(&mut state);
    if let Some(path) = state_path.as_deref()
        && let Err(err) = state.save(path)
    {
        tracing::warn!("cannot save the session on exit: {err:#}");
    }

    Ok(())
}
