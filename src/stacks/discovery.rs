use crate::github::PullRequestDetail;

use super::{
    model::{
        RepoContext, ReviewStack, StackDiscoveryError, StackDiscoveryOptions,
        StackProviderMetadata, StackSource,
    },
    providers,
};

pub trait StackProvider {
    fn source(&self) -> StackSource;

    fn discover(
        &self,
        selected_pr: &PullRequestDetail,
        repo_context: &RepoContext,
    ) -> Result<Option<ReviewStack>, StackDiscoveryError>;
}

pub fn discover_review_stack(
    selected_pr: &PullRequestDetail,
    repo_context: &RepoContext,
    options: StackDiscoveryOptions,
) -> Result<ReviewStack, StackDiscoveryError> {
    let mut provider_errors = Vec::<String>::new();

    if options.enable_github_native {
        match providers::github_native::discover(selected_pr, repo_context) {
            Ok(Some(stack)) => return Ok(stack),
            Ok(None) => {}
            Err(error) => provider_errors.push(format!("GitHub native: {}", error.message)),
        }
    }

    if options.enable_branch_topology {
        match providers::branch_topology::discover(selected_pr, repo_context) {
            Ok(Some(stack)) => return Ok(stack),
            Ok(None) => {}
            Err(error) => provider_errors.push(format!("Branch topology: {}", error.message)),
        }
    }

    if options.enable_local_metadata {
        match providers::local_metadata::discover(selected_pr, repo_context) {
            Ok(Some(stack)) => return Ok(stack),
            Ok(None) => {}
            Err(error) => provider_errors.push(format!("Local metadata: {}", error.message)),
        }
    }

    if options.enable_ai_virtual {
        if let Some(provider) = options.ai_provider {
            match providers::ai_virtual::discover(
                selected_pr,
                repo_context,
                &options.sizing,
                provider,
            ) {
                Ok(Some(stack)) => return Ok(stack),
                Ok(None) => {}
                Err(error) => provider_errors.push(format!("AI virtual: {}", error.message)),
            }
        }
    }

    if options.enable_virtual_commits {
        match providers::virtual_commits::discover(selected_pr, repo_context) {
            Ok(Some(stack)) => return Ok(stack),
            Ok(None) => {}
            Err(error) => provider_errors.push(format!("Virtual commits: {}", error.message)),
        }
    }

    if options.enable_virtual_semantic {
        let mut stack =
            providers::virtual_semantic::discover(selected_pr, repo_context, &options.sizing)?
                .ok_or_else(|| StackDiscoveryError::new("No stack provider produced a stack."))?;
        if !provider_errors.is_empty() {
            let payload = serde_json::json!({ "softErrors": provider_errors });
            stack.provider = Some(StackProviderMetadata {
                provider: stack
                    .provider
                    .as_ref()
                    .map(|provider| provider.provider.clone())
                    .unwrap_or_else(|| "virtual_semantic".to_string()),
                raw_payload: Some(payload),
            });
        }
        return Ok(stack);
    }

    Err(StackDiscoveryError::new(
        "No stack providers were enabled for discovery.",
    ))
}
