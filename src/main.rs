//! gtasks — multi-repo GitHub issues & PRs in the terminal.
//!
//! Thin wrapper: logging, terminal lifetime, the event loop, and the one place
//! that owns the fetch thread. All state lives in `gtasks::app`, all drawing in
//! `gtasks::ui`.

#![deny(clippy::print_stdout)]

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
    session: &mut Option<EmbeddedSession>,
) {
    // One agent at a time: the box shows one session, so a second `g` while
    // one runs is refused instead of orphaning the first.
    if session.is_some() {
        app.status = "an agent is already running in the box below".to_string();
        return;
    }
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
                launch_agent(app, &plan, &created.path, term_size, session);
            }
            // The refusal goes to the screen as it is: a check failure must
            // not become indistinguishable from a bug in the check.
            Err(err) => app.status = format!("{err:#}"),
        }
        return;
    }
    match agent::next_step(target) {
        agent::AgentNext::Launch(path) => {
            launch_agent(app, &plan, &path, term_size, session);
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
fn start_placed(
    app: &mut App,
    plan: AgentPlan,
    path: PathBuf,
    term_size: (u16, u16),
    session: &mut Option<EmbeddedSession>,
) {
    // One agent at a time, same as every other launch.
    if session.is_some() {
        app.status = "an agent is already running in the box below".to_string();
        return;
    }
    match agent::next_step(path) {
        agent::AgentNext::Launch(path) => {
            launch_agent(app, &plan, &path, term_size, session);
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
    session: &Option<EmbeddedSession>,
) {
    if session
        .as_ref()
        .is_some_and(|s| s.worktree() == order.old_path)
    {
        app.status = "an agent is still running in that worktree".to_string();
        return;
    }
    let rename = gtasks::worktree::rename_branch_argv(
        binaries,
        &order.clone,
        &order.old_branch,
        &order.new_branch,
    );
    match gtasks::gh::run_pipe(&rename, std::time::Duration::from_secs(60)) {
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
    match gtasks::gh::run_pipe(&mv, std::time::Duration::from_secs(60)) {
        Ok(out) if out.status.success() => {
            if let Some(record) = app.agent_record.as_mut() {
                if record.worktree == order.old_path {
                    record.worktree = order.new_path.clone();
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
    session: &Option<EmbeddedSession>,
) {
    if session.as_ref().is_some_and(|s| s.worktree() == order.path) {
        app.status = "an agent is still running in that worktree".to_string();
        return;
    }
    let remove = gtasks::worktree::remove_argv(binaries, &order.clone, &order.path);
    match gtasks::gh::run_pipe(&remove, std::time::Duration::from_secs(60)) {
        Ok(out) if out.status.success() => {
            if let Some(record) = app.agent_record.as_ref() {
                if record.worktree == order.path {
                    app.agent_record = None;
                    app.agent_dismissed_for = None;
                }
            }
            app.status = format!(
                "deleted worktree {} (branch {} kept)",
                order.path.display(),
                order.branch
            );
        }
        Ok(out) => {
            app.status = format!(
                "cannot delete the worktree: {}",
                String::from_utf8_lossy(&out.stderr).trim()
            );
        }
        Err(err) => {
            app.status = format!("cannot delete the worktree: {err:#}");
        }
    }
}

/// Run the handoff the user validated: seed file plus an agent spawned
/// embedded in the agent box. The TUI never suspends; the child lives in a
/// pty sized to the box, and the loop polls it every tick.
fn launch_agent(
    app: &mut App,
    plan: &AgentPlan,
    path: &std::path::Path,
    term_size: (u16, u16),
    session: &mut Option<EmbeddedSession>,
) {
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
        agent::agent_argv_for(
            plan.agent.id,
            &program,
            &plan.profile,
            seed.path(),
            path,
            app.sandbox_agents,
        )
    };
    // The box the pty is sized to: the same geometry the draw uses, so the
    // terminal and its box never disagree by a row.
    let outer = ui::agent_box(Rect::new(0, 0, term_size.0, term_size.1));
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
            app.agent_running = true;
            app.agent_screen = sess.snapshot();
            app.agent_record = Some(gtasks::agent::AgentRecord {
                repo: plan.repo.clone(),
                number: plan.number,
                profile: plan.profile.clone(),
                agent: plan.agent.id.name().to_string(),
                worktree: path.to_path_buf(),
                seed_lines: lines,
                status: gtasks::agent::AgentStatus::Running,
            });
            app.agent_dismissed_for = None;
            app.pane = gtasks::app::Pane::Agent;
            app.status = if app.sandbox_agents {
                "agent running below — sandboxed, Esc unfocuses, Ctrl-C interrupts".to_string()
            } else {
                "agent running below — Esc unfocuses, Ctrl-C interrupts".to_string()
            };
            *session = Some(sess);
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

    let (config, config_notice) = load_config();
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
        all.clone(),
        limit,
        timeout,
        parallel,
    ));
    let mut pending_detail: Option<PendingDetail> = None;
    let mut pending_diff: Option<PendingDiff> = None;
    // The embedded agent, if one is running inside the box. Owned here, not
    // in App: the loop polls it, routes keys to it and kills it on quit.
    let mut embedded: Option<EmbeddedSession> = None;

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
                    // No prefetch: the detail body is fetched only on explicit
                    // request (Enter). The list already shows every field.
                }
                Err(TryRecvError::Empty) => {}
                Err(TryRecvError::Disconnected) => {
                    app.loading = false;
                    app.status = "fetch ended unexpectedly".to_string();
                    pending = None;
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
        }
        if let Some(result) = detail_result {
            if let Some((repo, number, _)) = pending_detail.take() {
                match result {
                    Ok(body) => app.apply_detail(&repo, number, body),
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
        }
        if let Some(result) = diff_result {
            if let Some((repo, number, _)) = pending_diff.take() {
                if let Err(err) = &result {
                    // Do not leave the request pending: it would be retried
                    // on every loop iteration. The modal stays open showing
                    // the failure (apply_diff writes it), so the status
                    // line echo is for the record, not the discovery.
                    app.status = format!("diff failed: {err}");
                }
                app.apply_diff(&repo, number, result);
            }
        }

        // `R`: the key handler already updated the live list; the file is
        // the loop's job, atomically and 0600, like every private write.
        // An empty check means "everything", like an empty state file: one
        // meaning, said out loud — not "0 repos" with all of them shown.
        // The session (pane, cursors, tabs, filter) travels with it, so a
        // temporary exit keeps what the user was doing.
        if let Some(selected) = app.selection_applied.take() {
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
                    }
                },
                None => app.session_dirty = false,
            }
        }

        // `o`: the key handler already validated the URL.
        if let Some(url) = app.open_requested.take() {
            match &binaries.xdg_open {
                Some(program) => match actions::open_in_browser(program, &url) {
                    Ok(pid) => app.status = format!("opened in the browser (pid {pid})"),
                    Err(err) => app.status = format!("cannot open: {err:#}"),
                },
                None => app.status = "no browser launcher (xdg-open) found".to_string(),
            }
        }

        // A request from the key handler.
        if pending_detail.is_none() {
            if let Some((repo, number)) = app.detail_requested.clone() {
                pending_detail = Some((
                    repo.clone(),
                    number,
                    spawn_detail(binaries.clone(), repo, number, ItemKind::Pr, timeout),
                ));
            }
        }

        // A diff request from the key handler.
        if pending_diff.is_none() {
            if let Some((repo, number)) = app.diff_requested.clone() {
                pending_diff = Some((
                    repo.clone(),
                    number,
                    spawn_diff(binaries.clone(), repo, number, timeout),
                ));
            }
        }

        // One refresh in flight at a time (single-flight).
        if app.refresh_requested && pending.is_none() {
            app.refresh_requested = false;
            pending = Some(spawn_fetch(
                binaries.clone(),
                all.clone(),
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
            start_agent(
                &mut app,
                &binaries,
                &config,
                plan,
                None,
                term_size,
                &mut embedded,
            );
        }
        if let Some((plan, path)) = app.agent_go.take() {
            start_agent(
                &mut app,
                &binaries,
                &config,
                plan,
                Some(path),
                term_size,
                &mut embedded,
            );
        }
        if let Some((plan, path)) = app.agent_place.take() {
            start_placed(&mut app, plan, path, term_size, &mut embedded);
        }
        if let Some(order) = app.rename_go.take() {
            rename_worktree(&mut app, &binaries, order, &embedded);
        }
        if let Some(order) = app.delete_go.take() {
            delete_worktree(&mut app, &binaries, order, &embedded);
        }

        // The embedded agent, if any: keep the pty sized to its box, feed
        // fresh output into the screen, and finalize the record on exit.
        if let Some(sess) = embedded.as_mut() {
            let outer = ui::agent_box(Rect::new(0, 0, term_size.0, term_size.1));
            sess.resize(
                outer.width.saturating_sub(2),
                outer.height.saturating_sub(2),
            );
            if sess.drain() {
                app.agent_screen = sess.snapshot();
            }
            if let Some(record) = sess.poll_exit() {
                let summary = record.status.summary();
                let key = (record.repo.clone(), record.number);
                app.agent_record = Some(record);
                // The finish is announced in the status line; the detail
                // returns at once instead of gluing the finished card to
                // the item. A new run clears the dismissal at launch.
                app.agent_dismissed_for = Some(key);
                app.agent_running = false;
                app.agent_screen = Vec::new();
                app.status = format!("agent finished: {summary}");
                embedded = None;
            }
        }

        let view = ui::view_for(&app);
        terminal.draw(|frame| ui::draw(frame, &app, &view))?;

        if event::poll(Duration::from_millis(250))? {
            match event::read()? {
                Event::Key(key) => {
                    // Focused on a running agent, every key but Esc/Tab is
                    // agent input — including Ctrl-C, which interrupts it.
                    // Esc unfocuses, Tab cycles panes, Shift+arrows move
                    // between boxes: all through the app. Anywhere else,
                    // keys are app keys (where Ctrl-C quits).
                    let to_app = matches!(
                        map_key(key),
                        Some(Msg::Key(
                            Key::Esc
                                | Key::Tab
                                | Key::BackTab
                                | Key::ShiftUp
                                | Key::ShiftDown
                                | Key::ShiftLeft
                                | Key::ShiftRight
                        ))
                    );
                    let to_agent = embedded.is_some()
                        && app.agent_running
                        && app.pane == gtasks::app::Pane::Agent
                        && !to_app;
                    if to_agent {
                        if let Some(Msg::Key(k)) = map_key(key) {
                            if let Some(bytes) = gtasks::embedded::key_to_pty(&k) {
                                if let Some(sess) = embedded.as_mut() {
                                    sess.write(&bytes);
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
                        if let MouseEventKind::Down(MouseButton::Left) = me.kind {
                            let area = Rect::new(0, 0, term_size.0, term_size.1);
                            if let Some(idx) = ui::tab_at(&app, area, me.column, me.row) {
                                app.activate_tab(idx);
                            }
                        }
                    }
                    if !matches!(app.modal, Some(Modal::Diff)) {
                        let box_shown = app.agent_active() || app.pane == gtasks::app::Pane::Agent;
                        match me.kind {
                            MouseEventKind::ScrollUp if box_shown => {
                                if let Some(sess) = embedded.as_mut() {
                                    sess.scroll(3);
                                    app.agent_screen = sess.snapshot();
                                } else {
                                    app.agent_scroll = app.agent_scroll.saturating_add(3);
                                }
                            }
                            MouseEventKind::ScrollDown if box_shown => {
                                if let Some(sess) = embedded.as_mut() {
                                    sess.scroll(-3);
                                    app.agent_screen = sess.snapshot();
                                } else {
                                    app.agent_scroll = app.agent_scroll.saturating_sub(3);
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

    // Kill the embedded agent, if any: quitting must not leave an agent
    // running without a box. The seed file is unlinked on drop either way.
    if let Some(mut sess) = embedded.take() {
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
