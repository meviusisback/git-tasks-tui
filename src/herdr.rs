//! Herdr backend: agents Herdr owns, gtasks embeds.
//!
//! Herdr runs the agent process (tab + pane + `agent start`); gtasks embeds
//! `herdr agent attach <name>` as the agent-box child, so typing in the box
//! and typing in Herdr reach the same agent — and the agent stays visible in
//! Herdr when gtasks closes. No Herdr daemon or socket access beyond its CLI:
//! every call below is one `herdr` invocation with a fixed subcommand and
//! validated arguments, through the same checked transport as `gh`.
//!
//! Sandbox note: Herdr starts the canonical agent binary, never our
//! `<agent>-sandboxed` wrapper, and a pane-run wrapper is not recognised as
//! an agent (`agent list` stays empty). So the Herdr path is used only when
//! `sandbox_agents` is off; a sandboxed launch always stays direct.

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::Context;

use crate::gh::{Argv, CheckedArgv, run_checked};

/// Herdr agent names: lowercase, digits, `_`/`-`, starting with a letter,
/// at most this long. Our names are built to fit.
pub const MAX_NAME_LEN: usize = 31;

/// How long `agent start` may wait for interactive readiness (Herdr itself
/// waits up to 30s by default); the tab/pane calls are instant queries.
pub const TIMEOUT_START: Duration = Duration::from_secs(60);
const TIMEOUT_QUICK: Duration = Duration::from_secs(15);

/// Build our side of one `herdr` call: absolute program, our own words as
/// arguments, base-only environment. Labels and names come from validated
/// repo/number/agent pieces, never raw user text.
fn herdr_argv(program: &Path, cwd: &Path, args: Vec<OsString>) -> CheckedArgv {
    CheckedArgv::from_checked(Argv {
        program: program.to_path_buf(),
        args,
        env: crate::env::base_env(),
        cwd: cwd.to_path_buf(),
    })
}

fn os(text: &str) -> OsString {
    OsString::from(text)
}

/// The agent name for one tab: `g-<repo>-<number>-<agent>` (plus profile
/// when it is not the default `coder`), lowercased, anything outside the
/// Herdr charset folded to `-`, the repo part trimmed to fit. Two different
/// owners with the same repo name share the prefix; the launch retries with
/// a numeric suffix when the name is taken by another tree (see
/// [`ensure_unique`]).
pub fn agent_name(repo: &str, number: u64, agent: &str, profile: &str) -> String {
    let owner_repo = repo.split('/').next_back().unwrap_or(repo);
    let mut repo_part: String = owner_repo
        .to_lowercase()
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '-'
            }
        })
        .collect();
    while repo_part.starts_with('-') || repo_part.starts_with('_') {
        repo_part.remove(0);
    }
    if repo_part.is_empty() {
        repo_part.push_str("repo");
    }
    let profile_part = if profile == "coder" {
        String::new()
    } else {
        format!("-{profile}")
    };
    let suffix = format!("-{number}-{agent}{profile_part}");
    let keep = MAX_NAME_LEN.saturating_sub(2 + suffix.len());
    if repo_part.len() > keep {
        repo_part.truncate(keep);
    }
    let mut full = format!("g-{repo_part}{suffix}");
    // Profiles are validated but not length-capped: a hard cut keeps the
    // Herdr charset/length rule structural instead of hoped-for. Everything
    // here is ASCII (repo folded, number/agent/profile validated), so the
    // byte cut cannot split a character.
    full.truncate(MAX_NAME_LEN);
    full
}

/// What `agent get` told us about a live agent.
#[derive(Debug, Clone)]
pub struct AgentInfo {
    pub name: String,
    pub pane_id: String,
    pub cwd: String,
}

/// The Herdr agent behind one tree, persisted across restarts so a later
/// launch re-attaches instead of starting a second agent in the tree.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct HerdrBinding {
    pub name: String,
    pub tab_id: String,
    pub pane_id: String,
}

/// One Herdr-side start, owned by the worker thread: the main thread only
/// polls the receiver, so the 30s+ `agent start` readiness wait never
/// freezes the TUI.
#[derive(Debug, Clone)]
pub struct HerdrJob {
    pub program: PathBuf,
    pub cli_cwd: PathBuf,
    pub name: String,
    pub kind: String,
    pub agent_args: Vec<String>,
    pub repo: String,
    pub tree: PathBuf,
    pub label: String,
    pub binding: Option<HerdrBinding>,
}

/// The thread's answer: attach parameters, or a reason to fall back to a
/// direct launch (Herdr missing, daemon down, start refused).
#[derive(Debug, Clone)]
pub enum HerdrOutcome {
    Attached {
        name: String,
        tab_id: String,
        pane_id: String,
    },
    Failed(String),
}

/// Everything the loop needs while the thread works: the receiver plus the
/// launch it completes (the seed stays alive here until the attach consumes
/// it — dropping it early would unlink the seed mid-launch).
pub struct HerdrPending {
    pub rx: std::sync::mpsc::Receiver<HerdrOutcome>,
    pub plan: crate::agent::AgentPlan,
    pub path: PathBuf,
    pub body: String,
    pub lines: usize,
    pub seed: crate::agent::SeedFile,
    pub resume: bool,
}

/// The box child for a Herdr agent: `herdr agent attach <name>`. Input typed
/// in the box flows through the attach client into the agent; the same agent
/// stays visible (and typable) in Herdr's own tabs.
pub fn attach_argv(program: &Path, tree: &Path, name: &str) -> CheckedArgv {
    herdr_argv(program, tree, vec![os("agent"), os("attach"), os(name)])
}

/// Run one start-or-attach in the worker thread.
pub fn run_job(job: HerdrJob) -> HerdrOutcome {
    let failed = |err: anyhow::Error| HerdrOutcome::Failed(format!("{err:#}"));
    // A recorded binding may still be live (tab closed in gtasks, agent
    // running in Herdr): re-attach instead of starting a second agent.
    if let Some(binding) = &job.binding {
        match agent_get(&job.program, &job.cli_cwd, &binding.name) {
            Ok(Some(info)) if Path::new(&info.cwd) == job.tree => {
                return HerdrOutcome::Attached {
                    name: binding.name.clone(),
                    tab_id: binding.tab_id.clone(),
                    pane_id: binding.pane_id.clone(),
                };
            }
            Ok(_) => {}
            Err(err) => return failed(err),
        }
        // The name is ours but the agent is gone (or it now points
        // elsewhere): the pane sits at its shell prompt again, so start the
        // same name there. A vanished pane falls through to a fresh tab.
        if agent_start(
            &job.program,
            &job.cli_cwd,
            &binding.name,
            &job.kind,
            &binding.pane_id,
            &job.agent_args,
        )
        .is_ok()
        {
            return HerdrOutcome::Attached {
                name: binding.name.clone(),
                tab_id: binding.tab_id.clone(),
                pane_id: binding.pane_id.clone(),
            };
        }
    }
    // Fresh name, unique among live agents: a stale same-name agent from a
    // renamed tree would otherwise join the wrong session on re-attach. A
    // failed lookup is not a green light to skip the dance, but `agent
    // start` refuses a taken name with a clear error, so falling through
    // to it is still safe — the failure just surfaces one step later.
    let mut name = job.name.clone();
    if matches!(
        name_taken_elsewhere(&job.program, &job.cli_cwd, &name, &job.tree),
        Ok(true)
    ) {
        let mut free = false;
        let stem: String = job.name.chars().take(MAX_NAME_LEN - 2).collect();
        for n in 2..=5 {
            name = format!("{stem}-{n}");
            match name_taken_elsewhere(&job.program, &job.cli_cwd, &name, &job.tree) {
                Ok(false) => {
                    free = true;
                    break;
                }
                Ok(true) => continue,
                Err(err) => return failed(err),
            }
        }
        if !free {
            return HerdrOutcome::Failed("no free Herdr agent name".to_string());
        }
    }
    let start_in = |pane_id: &str| {
        agent_start(
            &job.program,
            &job.cli_cwd,
            &name,
            &job.kind,
            pane_id,
            &job.agent_args,
        )
    };
    // The repo's Space first: one workspace per repo, created named after
    // the repo when missing. A fresh Space already owns a root tab in the
    // tree, so the agent starts there; otherwise a tab grows in it.
    let slot = match workspace_ensure(&job.program, &job.cli_cwd, &job.repo, &job.tree) {
        Ok(slot) => slot,
        Err(err) => return failed(err),
    };
    match slot {
        WorkspaceSlot::ReuseRoot { tab_id, pane_id } => {
            let _ = tab_rename(&job.program, &job.cli_cwd, &tab_id, &job.label);
            match start_in(&pane_id) {
                Ok(()) => HerdrOutcome::Attached {
                    name,
                    tab_id,
                    pane_id,
                },
                Err(err) => failed(err),
            }
        }
        WorkspaceSlot::NewTab { workspace_id } => {
            match tab_create(
                &job.program,
                &job.cli_cwd,
                &workspace_id,
                &job.tree,
                &job.label,
            ) {
                Ok((tab_id, pane_id)) => match start_in(&pane_id) {
                    Ok(()) => HerdrOutcome::Attached {
                        name,
                        tab_id,
                        pane_id,
                    },
                    Err(err) => failed(err),
                },
                Err(err) => failed(err),
            }
        }
    }
}

fn parse_agent_info(value: &serde_json::Value) -> Option<AgentInfo> {
    let agent = value.get("agent")?;
    Some(AgentInfo {
        name: agent.get("name")?.as_str()?.to_string(),
        pane_id: agent.get("pane_id")?.as_str()?.to_string(),
        cwd: agent.get("cwd")?.as_str().unwrap_or("").to_string(),
    })
}

/// The live agent behind `target` (our name or a pane id), if Herdr still
/// knows it. A stale binding (agent exited, tab closed in Herdr) is `Ok(None)`,
/// not an error — the caller rebinds.
pub fn agent_get(program: &Path, cwd: &Path, target: &str) -> anyhow::Result<Option<AgentInfo>> {
    let argv = herdr_argv(program, cwd, vec![os("agent"), os("get"), os(target)]);
    let out = run_checked(&argv, TIMEOUT_QUICK)?;
    if !out.status.success() {
        return Ok(None);
    }
    let value: serde_json::Value =
        serde_json::from_slice(&out.stdout).context("cannot parse `herdr agent get` output")?;
    Ok(value.get("result").and_then(parse_agent_info))
}

/// Create a Herdr tab rooted in the worktree inside `workspace`.
/// Returns `(tab_id, pane_id)`.
pub fn tab_create(
    program: &Path,
    cwd: &Path,
    workspace: &str,
    tree: &Path,
    label: &str,
) -> anyhow::Result<(String, String)> {
    let argv = herdr_argv(
        program,
        cwd,
        vec![
            os("tab"),
            os("create"),
            os("--workspace"),
            os(workspace),
            os("--cwd"),
            tree.as_os_str().to_os_string(),
            os("--label"),
            os(label),
            os("--no-focus"),
        ],
    );
    let out = run_checked(&argv, TIMEOUT_QUICK)?;
    anyhow::ensure!(out.status.success(), "`herdr tab create` failed");
    let value: serde_json::Value =
        serde_json::from_slice(&out.stdout).context("cannot parse `herdr tab create` output")?;
    let tab = value
        .pointer("/result/tab/tab_id")
        .and_then(|v| v.as_str())
        .context("`herdr tab create` returned no tab id")?;
    let pane = value
        .pointer("/result/root_pane/pane_id")
        .and_then(|v| v.as_str())
        .context("`herdr tab create` returned no pane id")?;
    Ok((tab.to_string(), pane.to_string()))
}

/// Rename a tab (the auto-created root tab of a fresh Space says "1":
/// give it the agent's label). Best effort — a missed rename never fails
/// the launch.
pub fn tab_rename(program: &Path, cwd: &Path, tab_id: &str, label: &str) -> anyhow::Result<()> {
    let argv = herdr_argv(
        program,
        cwd,
        vec![os("tab"), os("rename"), os(tab_id), os(label)],
    );
    let out = run_checked(&argv, TIMEOUT_QUICK)?;
    anyhow::ensure!(out.status.success(), "`herdr tab rename` failed");
    Ok(())
}

/// Where the agent goes: a fresh Space already owns a root tab sitting at
/// a shell prompt (start there, no extra tab), while an existing Space
/// grows one tab per agent.
pub enum WorkspaceSlot {
    ReuseRoot { tab_id: String, pane_id: String },
    NewTab { workspace_id: String },
}

/// The repo's Space on Herdr: one workspace per repo, labeled with the
/// repo name alone (`payment_nexi`, not `owner/payment_nexi` — the owner
/// is always him and only eats label room). Reused when it exists,
/// created named after the repo when it does not. A fresh Space arrives
/// with its root tab already in the tree, so the agent starts there
/// directly.
pub fn workspace_ensure(
    program: &Path,
    cwd: &Path,
    repo: &str,
    tree: &Path,
) -> anyhow::Result<WorkspaceSlot> {
    // Short name only: every repo here is his, the owner adds nothing.
    let short = repo.split('/').next_back().unwrap_or(repo);
    let argv = herdr_argv(program, cwd, vec![os("workspace"), os("list")]);
    let out = run_checked(&argv, TIMEOUT_QUICK)?;
    anyhow::ensure!(out.status.success(), "`herdr workspace list` failed");
    let value: serde_json::Value = serde_json::from_slice(&out.stdout)
        .context("cannot parse `herdr workspace list` output")?;
    if let Some(spaces) = value
        .pointer("/result/workspaces")
        .and_then(|v| v.as_array())
    {
        for space in spaces {
            let label = space.get("label").and_then(|v| v.as_str()).unwrap_or("");
            // Full slugs are legacy: Spaces used to carry `owner/name`.
            if label == short || label == repo {
                let id = space
                    .get("workspace_id")
                    .and_then(|v| v.as_str())
                    .context("workspace without id")?;
                return Ok(WorkspaceSlot::NewTab {
                    workspace_id: id.to_string(),
                });
            }
        }
    }
    let argv = herdr_argv(
        program,
        cwd,
        vec![
            os("workspace"),
            os("create"),
            os("--label"),
            os(short),
            os("--cwd"),
            tree.as_os_str().to_os_string(),
            os("--no-focus"),
        ],
    );
    let out = run_checked(&argv, TIMEOUT_START)?;
    anyhow::ensure!(out.status.success(), "`herdr workspace create` failed");
    let value: serde_json::Value = serde_json::from_slice(&out.stdout)
        .context("cannot parse `herdr workspace create` output")?;
    match value
        .pointer("/result/root_pane/pane_id")
        .and_then(|v| v.as_str())
    {
        Some(pane_id) => Ok(WorkspaceSlot::ReuseRoot {
            tab_id: value
                .pointer("/result/tab/tab_id")
                .and_then(|v| v.as_str())
                .context("fresh Space without tab id")?
                .to_string(),
            pane_id: pane_id.to_string(),
        }),
        None => anyhow::bail!("fresh Space without root pane"),
    }
}
/// Start the agent Herdr-side in an existing pane: `agent start <name>
/// --kind <kind> --pane <pane> -- <our args…>`. The pane must sit at its
/// shell prompt; a reused pane whose agent already exited qualifies.
pub fn agent_start(
    program: &Path,
    cwd: &Path,
    name: &str,
    kind: &str,
    pane_id: &str,
    agent_args: &[String],
) -> anyhow::Result<()> {
    let mut args = vec![
        os("agent"),
        os("start"),
        os(name),
        os("--kind"),
        os(kind),
        os("--pane"),
        os(pane_id),
        os("--"),
    ];
    args.extend(agent_args.iter().map(|a| os(a)));
    let argv = herdr_argv(program, cwd, args);
    let out = run_checked(&argv, TIMEOUT_START)?;
    anyhow::ensure!(
        out.status.success(),
        "`herdr agent start {name}` failed: {}",
        String::from_utf8_lossy(&out.stderr).trim()
    );
    Ok(())
}

/// Close our Herdr tab. Best effort at quit time: the agent keeps whatever
/// state Herdr gives a closed tab; a failure only lands in the log.
pub fn tab_close(program: &Path, cwd: &Path, tab_id: &str) -> anyhow::Result<()> {
    let argv = herdr_argv(program, cwd, vec![os("tab"), os("close"), os(tab_id)]);
    let out = run_checked(&argv, TIMEOUT_QUICK)?;
    anyhow::ensure!(out.status.success(), "`herdr tab close` failed");
    Ok(())
}

/// Is the name taken by a live agent rooted in a *different* tree? Then a
/// plain re-attach would join the wrong session, and a plain start would
/// fail: the caller retries with a numeric suffix.
pub fn name_taken_elsewhere(
    program: &Path,
    cwd: &Path,
    name: &str,
    tree: &Path,
) -> anyhow::Result<bool> {
    match agent_get(program, cwd, name)? {
        None => Ok(false),
        Some(info) => Ok(Path::new(&info.cwd) != tree),
    }
}
