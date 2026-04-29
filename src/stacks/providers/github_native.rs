use crate::github::PullRequestDetail;

use super::super::model::{RepoContext, ReviewStack, StackDiscoveryError};

pub fn discover(
    _selected_pr: &PullRequestDetail,
    _repo_context: &RepoContext,
) -> Result<Option<ReviewStack>, StackDiscoveryError> {
    Ok(None)
}
