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
use gtasks::app::{App, Modal, Msg, Pending};
use gtasks::binaries::Binaries;
use gtasks::config::Config;
use gtasks::fetch::{self, FetchOutcome};
use gtasks::input::map_key;
use gtasks::logging::{init_file_logging, init_stderr_logging};
use gtasks::model::ItemKind;
use gtasks::state::State;
use gtasks::ui;
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use ratatui::crossterm::event::{self, Event};
use ratatui::crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use ratatui::crossterm::{cursor, execute};

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

/// Restores the terminal on drop: normal exit, `?` and panics all go through it.
struct TuiGuard;

impl TuiGuard {
    fn enter() -> io::Result<Self> {
        enable_raw_mode()?;
        execute!(io::stdout(), EnterAlternateScreen)?;
        Ok(Self)
    }
}

impl Drop for TuiGuard {
    fn drop(&mut self) {
        let _ = disable_raw_mode();
        let _ = execute!(io::stdout(), LeaveAlternateScreen, cursor::Show);
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

/// The config from disk when it is valid and non-empty, otherwise the defaults
/// with one demo repo so the TUI has something to show on a fresh machine.
fn load_config() -> Config {
    let Some(path) = config_path() else {
        return demo_config();
    };
    if !path.exists() {
        tracing::info!(path = %path.display(), "no config, using the demo repo");
        return demo_config();
    }
    match Config::load(&path) {
        Ok(config) if config.validate().is_ok() && !config.repos.is_empty() => config,
        Ok(_) => {
            tracing::warn!(path = %path.display(), "config has no usable repos");
            demo_config()
        }
        Err(err) => {
            tracing::warn!(path = %path.display(), error = %err, "cannot read config");
            demo_config()
        }
    }
}

fn demo_config() -> Config {
    Config {
        repos: vec!["meviusisback/git-tasks-tui".to_string()],
        ..Config::default()
    }
}

/// The loop's half of the agent handoff (`g` validated, `y` agreed).
/// `agreed` is `Some` when the user already said yes to creating the tree;
/// `None` means resolve first and ask before writing. Returns whether the
/// terminal was handed over — the loop repaints everything after that.
fn start_agent(
    app: &mut App,
    binaries: &Binaries,
    config: &Config,
    plan: AgentPlan,
    agreed: Option<PathBuf>,
) -> bool {
    let root = match config.worktree_root.clone() {
        Some(root) => root,
        None => match gtasks::worktree::default_root() {
            Ok(root) => root,
            Err(err) => {
                app.status = format!("cannot locate the worktree root: {err:#}");
                return false;
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
            return false;
        }
    }
    let slug = match RepoSlug::parse(&plan.repo) {
        Ok(slug) => slug,
        Err(_) => {
            app.status = format!("cannot place the agent: invalid repo {}", plan.repo);
            return false;
        }
    };
    let branch = match gtasks::worktree::BranchName::parse(&plan.branch) {
        Ok(branch) => branch,
        Err(_) => {
            app.status = format!("cannot place the agent: invalid branch {}", plan.branch);
            return false;
        }
    };
    let target = match gtasks::worktree::resolve_in(&root, &plan.clone, &slug, &branch) {
        Ok(target) => target,
        Err(err) => {
            app.status = format!("cannot place the agent: {err:#}");
            return false;
        }
    };
    // An agreed creation goes straight to git; anything else is asked first
    // — a write to disk is never a side effect of pressing `g`.
    if agreed.is_some() {
        match gtasks::worktree::create(&root, &plan.clone, &slug, &branch, binaries) {
            Ok(created) => return launch_agent(app, binaries, &plan, &created.path),
            // The refusal goes to the screen as it is: no sentence of ours
            // around it, or a check failure becomes indistinguishable from a
            // bug in the check.
            Err(err) => {
                app.status = format!("{err:#}");
                return false;
            }
        }
    }
    match agent::next_step(target) {
        agent::AgentNext::Launch(path) => launch_agent(app, binaries, &plan, &path),
        agent::AgentNext::AskToCreate(path) => {
            let question = format!(
                "no worktree for {}@{} — create it and start the agent?",
                slug.as_str(),
                branch.as_str()
            );
            app.modal = Some(Modal::Confirm(question));
            app.pending = Some(Pending::Agent(plan, path));
            false
        }
    }
}

/// Run the handoff the user validated: seed file, suspended TUI, agent on
/// the terminal, record on return. The seed dies with the run on every path.
/// Returns whether the terminal was handed over.
fn launch_agent(
    app: &mut App,
    binaries: &Binaries,
    plan: &AgentPlan,
    path: &std::path::Path,
) -> bool {
    let Some(hermes) = binaries.hermes.as_deref() else {
        app.status = "no agent binary (hermes) found".to_string();
        return false;
    };
    let seeds = match gtasks::agent::seeds_dir() {
        Ok(seeds) => seeds,
        Err(err) => {
            app.status = format!("cannot prepare the seed directory: {err:#}");
            return false;
        }
    };
    let body = agent::agent_seed_body(&plan.repo, plan.number, &plan.url, None);
    let lines = body.lines().count();
    let seed = match gtasks::agent::SeedFile::create_in(&seeds, &body) {
        Ok(seed) => seed,
        Err(err) => {
            app.status = format!("cannot write the agent seed: {err:#}");
            return false;
        }
    };
    let argv = agent::agent_argv(hermes, &plan.profile, seed.path(), path);
    // The handoff ran in both arms below: the screen always needs a full
    // repaint, even when the run itself failed.
    match agent::run_agent_handoff(&argv, seed, lines) {
        Ok((status, _)) => {
            let summary = status.summary();
            app.agent_record = Some(gtasks::agent::AgentRecord {
                repo: plan.repo.clone(),
                number: plan.number,
                profile: plan.profile.clone(),
                worktree: path.to_path_buf(),
                seed_lines: lines,
                status,
            });
            app.agent_dismissed_for = None;
            app.status = format!("agent on {}#{}: {}", plan.repo, plan.number, summary);
        }
        Err(err) => app.status = format!("cannot run the agent: {err:#}"),
    }
    true
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

fn run_tui() -> anyhow::Result<()> {
    install_signal_handlers();

    // A panic must not leave the user with an unusable terminal.
    let previous_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let _ = disable_raw_mode();
        let _ = execute!(io::stdout(), LeaveAlternateScreen, cursor::Show);
        previous_hook(info);
    }));

    let _guard = TuiGuard::enter()?;
    let mut terminal = Terminal::new(CrosstermBackend::new(io::stdout()))?;

    let binaries = match Binaries::detect() {
        Ok(binaries) => binaries,
        Err(err) => anyhow::bail!("gh and git are required: {err:#}"),
    };

    let config = load_config();
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
        clone_root: config.clone_root.clone(),
        ..App::default()
    };
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
    let mut pending: Option<Receiver<FetchOutcome>> = Some(spawn_fetch(
        binaries.clone(),
        all.clone(),
        limit,
        timeout,
        parallel,
    ));
    let mut pending_detail: Option<PendingDetail> = None;

    while !app.quit && !TERMINATE.load(Ordering::SeqCst) {
        // Take a finished fetch, if any.
        if let Some(rx) = &pending {
            match rx.try_recv() {
                Ok(outcome) => {
                    app.apply_outcome(outcome);
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

        // `R`: the key handler already updated the live list; the file is
        // the loop's job, atomically and 0600, like every private write.
        // An empty check means "everything", like an empty state file: one
        // meaning, said out loud — not "0 repos" with all of them shown.
        if let Some(selected) = app.selection_applied.take() {
            let cleared = selected.is_empty();
            state.selected_repos = selected;
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
                    }
                    Err(err) => app.status = format!("cannot save selection: {err:#}"),
                },
                None => app.status = "selection applied, not saved (no state file)".to_string(),
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
        // creation): the loop owns git, the seed file and the terminal.
        // After a handoff the alternate screen is re-entered but ratatui's
        // diff buffer is stale, so the loop repaints everything once.
        let mut resumed = false;
        if let Some(plan) = app.agent_requested.take() {
            resumed |= start_agent(&mut app, &binaries, &config, plan, None);
        }
        if let Some((plan, path)) = app.agent_go.take() {
            resumed |= start_agent(&mut app, &binaries, &config, plan, Some(path));
        }
        if resumed {
            terminal.clear()?;
        }

        let view = ui::view_for(&app);
        terminal.draw(|frame| ui::draw(frame, &app, &view))?;

        if event::poll(Duration::from_millis(250))? {
            match event::read()? {
                Event::Key(key) => {
                    if let Some(msg) = map_key(key) {
                        app.update(msg);
                    }
                }
                // Resizes only need a redraw, which the loop does anyway.
                Event::Resize(_, _) => {}
                _ => {}
            }
        } else {
            app.update(Msg::Tick);
        }
    }

    Ok(())
}
