//! What the panel holds.

use std::path::PathBuf;

use crate::api::{
    Branch, Discussion, FileDiff, Job, MergeOptions, MergeRequest, NewMergeRequest, Pipeline, Tag,
    User,
};
use crate::remote::Checkout;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Tab {
    #[default]
    MergeRequests,
    Pipelines,
    Tags,
}

/// Whether the plugin can talk to the server of the project.
#[derive(Debug, Clone, Default, PartialEq)]
pub enum Link {
    /// Not looked at yet, or being tried.
    #[default]
    Checking,
    /// No token for this server: one is asked for.
    NoToken,
    Ready(User),
    Failed(String),
}

/// The states of merge requests that can be listed.
pub const FILTERS: [&str; 4] = ["opened", "merged", "closed", "all"];

/// A merge request on show.
#[derive(Debug, Clone, Default)]
pub struct Shown {
    pub request: MergeRequest,
    pub files: Option<Vec<FileDiff>>,
    pub discussions: Option<Vec<Discussion>>,
    pub pipelines: Vec<Pipeline>,
    pub merge: MergeOptions,
    /// The comment being written.
    pub comment: String,
    /// The thread being answered, and the answer.
    pub reply: Option<(String, String)>,
}

#[derive(Debug, Default)]
pub struct State {
    pub workspace: Vec<PathBuf>,
    pub file: Option<PathBuf>,
    /// None: no repository with a remote was found.
    pub checkout: Option<Checkout>,
    pub link: Link,
    pub tab: Tab,
    pub busy: bool,
    /// The last thing said, and whether it is an error.
    pub notice: Option<(String, bool)>,
    /// A button that asks to be clicked twice, and was clicked once.
    pub armed: Option<String>,

    /// Typed to connect.
    pub token: String,
    pub address: String,

    pub filter: usize,
    pub requests: Option<Vec<MergeRequest>>,
    pub shown: Option<Shown>,
    pub new: Option<NewMergeRequest>,
    pub branches: Vec<Branch>,
    pub default_branch: String,

    pub pipelines: Option<Vec<Pipeline>>,
    /// The pipeline on show, with its jobs.
    pub pipeline: Option<(u64, Option<Vec<Job>>)>,
    pub run_ref: String,

    pub tags: Option<Vec<Tag>>,
    pub tag_name: String,
    pub tag_ref: String,
    pub tag_message: String,
}

impl State {
    /// Forgets what was read of a project (another one is looked at).
    pub fn reset(&mut self) {
        *self = State {
            workspace: std::mem::take(&mut self.workspace),
            file: self.file.take(),
            checkout: self.checkout.take(),
            tab: self.tab,
            filter: self.filter,
            ..State::default()
        };
    }

    pub fn host(&self) -> Option<&str> {
        self.checkout.as_ref().map(|c| c.project.host.as_str())
    }

    /// Whether `action` was clicked a second time: else it now waits for it.
    pub fn confirm(&mut self, action: &str) -> bool {
        if self.armed.as_deref() == Some(action) {
            self.armed = None;
            true
        } else {
            self.armed = Some(action.to_string());
            false
        }
    }

    /// The merge request to make from the branch checked out.
    pub fn draft(&self) -> NewMergeRequest {
        let branch = self
            .checkout
            .as_ref()
            .and_then(|c| c.branch.clone())
            .filter(|b| *b != self.default_branch)
            .unwrap_or_default();
        NewMergeRequest {
            title: title_of(&branch),
            source: branch,
            target: self.default_branch.clone(),
            ..NewMergeRequest::default()
        }
    }

    /// Something shown can still change by itself.
    pub fn pipeline_running(&self) -> bool {
        let list = match self.tab {
            Tab::Pipelines => self.pipelines.as_deref().unwrap_or_default(),
            Tab::MergeRequests => self
                .shown
                .as_ref()
                .map_or(&[][..], |s| s.pipelines.as_slice()),
            Tab::Tags => &[],
        };
        list.iter().any(Pipeline::running)
    }
}

/// `feat/add-a-thing` → `Add a thing`.
pub fn title_of(branch: &str) -> String {
    let name = branch.rsplit('/').next().unwrap_or(branch);
    let words = name.replace(['-', '_'], " ");
    let mut chars = words.trim().chars();
    chars
        .next()
        .map(|first| first.to_uppercase().chain(chars).collect())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::remote::Project;

    #[test]
    fn a_new_merge_request_starts_from_the_branch_checked_out() {
        let mut state = State {
            default_branch: "main".into(),
            checkout: Some(Checkout {
                project: Project {
                    host: "gitlab.com".into(),
                    path: "a/b".into(),
                },
                branch: Some("feat/add-a_thing".into()),
            }),
            ..State::default()
        };
        let new = state.draft();
        assert_eq!(
            (new.source.as_str(), new.target.as_str(), new.title.as_str()),
            ("feat/add-a_thing", "main", "Add a thing")
        );
        // On the default branch there is nothing to propose.
        state.checkout.as_mut().unwrap().branch = Some("main".into());
        assert_eq!(state.draft().source, "");
        assert_eq!(state.draft().title, "");
    }

    #[test]
    fn a_button_that_destroys_is_clicked_twice() {
        let mut state = State::default();
        assert!(!state.confirm("a"));
        assert!(!state.confirm("b"), "another button starts over");
        assert!(state.confirm("b"));
        assert!(!state.confirm("b"));
    }
}
