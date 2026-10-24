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
use crate::gh::{Argv, CheckedArgv, run_checked};

/// What the user is asked to confirm.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Confirmation {
    /// The action in words, for a human.
    pub summary: String,
    /// The command that will run, exactly as composed.
    pub argv: Vec<String>,
}

/// Build the pair (sentence, argv) shown before executing.
///
/// The argv is shown through the **same sanitiser as the log**, with newlines
/// escaped rather than removed: escaping keeps the text faithful, removing
/// would make it a different string. A body carrying `\n` therefore cannot draw
/// a second line in the screen where the user decides (U-2), while what the user
/// approves, what is executed and what reaches the log stay three views of the
/// same string.
pub fn confirmation_for(action: &Action, argv: &Argv) -> Confirmation {
    Confirmation {
        summary: summarise(action),
        argv: std::iter::once(argv.program.display().to_string())
            .chain(argv.args.iter().map(|a| a.to_string_lossy().into_owned()))
            .map(|text| shown(&text))
            .collect(),
    }
}

/// One sanitised, escaped line: faithful and impossible to mistake for a
/// second record.
fn shown(text: &str) -> String {
    crate::sanitize::sanitize(text).replace('\n', "\\n")
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
        Action::Close {
            repo,
            number,
            reason,
            comment,
        } => {
            let mut extra = String::new();
            if let Some(reason) = reason {
                extra.push_str(&format!(", reason: {reason}"));
            }
            if comment.is_some() {
                extra.push_str(", with a comment");
            }
            format!("close PR #{number} in {repo}{extra}")
        }
        Action::Checkout { repo, number } => format!("check out PR #{number} from {repo}"),
    }
}

/// How an execution ended.
#[derive(Debug)]
pub enum Executed {
    /// The process ran to completion, whatever its exit status.
    Finished(ExitStatus),
    /// The transport refused **before** spawning (allowlist, forbidden flag, …):
    /// nothing ran, so nothing happened on the remote.
    Refused(String),
    /// The process ran but the outcome is unknown (timeout, IO, signal): the
    /// remote **may** have applied the write, which is exactly why there is no
    /// retry. The UI must say so and must not offer one.
    Unknown(String),
}

/// Run the action once.
///
/// There is deliberately **no retry loop**: `run_pipe_with_caps` kills the child
/// and its group at the deadline, and a timed-out write must surface as an
/// error, not be repeated. `ETXTBSY` retries live inside the transport and
/// concern `exec`, not the remote effect of the command.
pub fn execute(argv: &CheckedArgv, timeout: Duration) -> anyhow::Result<Executed> {
    match run_checked(argv, timeout) {
        Ok(output) => Ok(Executed::Finished(output.status)),
        Err(err) if err.downcast_ref::<crate::gh::Timeout>().is_some() => {
            Ok(Executed::Unknown(err.to_string()))
        }
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
