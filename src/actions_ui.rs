//! Confirmation and execution of an action.
//!
//! Two guarantees, both structural rather than disciplinary:
//!
//! * the confirmation shows the **argv that will run**, verbatim, next to the
//!   sentence describing the action. If the sentence and the argv disagree, the
//!   bug is in the sentence, and the only way to notice is to show both;
//! * an action is run **once**. A write that times out is not retried: we cannot
//!   know whether the remote applied it, so a second attempt would be a second
//!   write.

use std::process::ExitStatus;
use std::time::Duration;

use crate::actions::{Action, MergeMethod};
use crate::gh::{Argv, Caps, run_pipe_with_caps};

/// What the user is asked to confirm.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Confirmation {
    /// The action in words, for a human.
    pub summary: String,
    /// The command that will run, exactly as composed.
    pub argv: Vec<String>,
}

/// Build the pair (sentence, argv) shown before executing.
pub fn confirmation_for(action: &Action, argv: &Argv) -> Confirmation {
    Confirmation {
        summary: summarise(action),
        argv: std::iter::once(argv.program.display().to_string())
            .chain(argv.args.iter().map(|a| a.to_string_lossy().into_owned()))
            .collect(),
    }
}

fn summarise(action: &Action) -> String {
    match action {
        Action::Merge {
            repo,
            number,
            method,
            delete_branch,
        } => {
            let how = match method {
                MergeMethod::Merge => "merge",
                MergeMethod::Squash => "squash",
                MergeMethod::Rebase => "rebase",
            };
            let delete = if *delete_branch {
                ", then delete the branch"
            } else {
                ""
            };
            format!("{how} PR #{number} in {repo}{delete}")
        }
        Action::Comment { repo, number, .. } => format!("comment on PR #{number} in {repo}"),
        Action::Close { repo, number, .. } => format!("close PR #{number} in {repo}"),
        Action::Checkout { repo, number } => format!("check out PR #{number} from {repo}"),
    }
}

/// How an execution ended.
#[derive(Debug)]
pub enum Executed {
    /// The process ran to completion, whatever its exit status.
    Finished(ExitStatus),
    /// The transport refused before spawning (allowlist, forbidden flag, …).
    Refused(String),
}

/// Run the action once.
///
/// There is deliberately **no retry loop**: `run_pipe_with_caps` kills the child
/// and its group at the deadline, and a timed-out write must surface as an
/// error, not be repeated. `ETXTBSY` retries live inside the transport and
/// concern `exec`, not the remote effect of the command.
pub fn execute(argv: &Argv, timeout: Duration) -> anyhow::Result<Executed> {
    match run_pipe_with_caps(argv, timeout, Caps::default()) {
        Ok(output) => Ok(Executed::Finished(output.status)),
        Err(err) => Ok(Executed::Refused(err.to_string())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::actions::RepoSlug;

    fn slug() -> RepoSlug {
        RepoSlug::parse("acme/widget").expect("valid slug")
    }

    #[test]
    fn the_confirmation_shows_the_argv_it_will_run() {
        let action = Action::Merge {
            repo: slug(),
            number: 7,
            method: MergeMethod::Squash,
            delete_branch: true,
        };
        let binaries = crate::binaries::Binaries::new(
            std::path::PathBuf::from("/bin/true"),
            std::path::PathBuf::from("/bin/true"),
        );
        let argv = crate::actions::argv_for(&binaries, &action).expect("builds");
        let shown = confirmation_for(&action, &argv);

        assert!(shown.summary.contains("#7"), "{}", shown.summary);
        assert!(
            shown.argv.iter().any(|a| a == "--squash"),
            "the argv is shown verbatim: {:?}",
            shown.argv
        );
    }
}
