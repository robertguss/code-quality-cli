//! Task IDs for new features (R6, KTD10, KTD11): found in the branch name, or
//! on push in a pushed commit message.

use super::scope::ScopeChange;
use super::{MAP_FILE, TASK, TASK_PENDING, count};
use crate::cli::Finding;

/// Where a run looked for task IDs.
pub struct Searched<'a> {
    pub pattern: &'a regex::Regex,
    pub branch: Option<&'a str>,
    /// Pushed commit messages; `None` at commit.
    pub messages: Option<&'a [String]>,
}

impl Searched<'_> {
    fn finds(&self, text: &str, task: &str) -> bool {
        self.pattern.find_iter(text).any(|m| m.as_str() == task)
    }

    fn where_(&self) -> String {
        let branch = match self.branch {
            Some(b) => format!("branch `{b}`"),
            None => "no branch (detached HEAD)".to_string(),
        };
        match self.messages {
            Some(m) => format!("{branch} and {}", count(m.len(), "pushed commit message")),
            None => branch,
        }
    }
}

/// Findings for the new, named features in `diff`.
pub fn check(diff: &[ScopeChange], s: &Searched) -> Vec<Finding> {
    let mut out = Vec::new();
    for change in diff {
        let ScopeChange::New {
            name,
            task,
            placeholder: false,
        } = change
        else {
            continue;
        };
        let Some(task) = task else {
            out.push(
                Finding::error(
                    TASK,
                    format!(
                        "new feature `{name}` has no task; add `task = \"<ID>\"` naming the task it was built for"
                    ),
                )
                .at(MAP_FILE, None),
            );
            continue;
        };
        if s.branch.is_some_and(|b| s.finds(b, task)) {
            continue;
        }
        let finding = match s.messages {
            Some(messages) if messages.iter().any(|m| s.finds(m, task)) => continue,
            Some(_) => Finding::error(
                TASK,
                format!(
                    "new feature `{name}` needs its task {task} in the branch name or a pushed commit message; \
                     searched {}. Put {task} in a commit title, such as `{task}: ...`",
                    s.where_()
                ),
            ),
            None => Finding::info(
                TASK_PENDING,
                format!(
                    "new feature `{name}`: task {task} is not in {}; it must be in the branch name or a commit message by push",
                    s.where_()
                ),
            ),
        };
        out.push(finding.at(MAP_FILE, None));
    }
    out
}
