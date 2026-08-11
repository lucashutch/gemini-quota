pub mod base;
pub mod github_copilot;
pub mod openai;
pub mod opencode;
pub mod openrouter;
use crate::model::Account;
use anyhow::{bail, Result};
use base::Provider;
pub fn create(account: Account) -> Result<Box<dyn Provider>> {
    Ok(match account.provider_type.as_str() {
        "github_copilot" => Box::new(github_copilot::GitHubCopilotProvider::new(account)),
        "openai" => Box::new(openai::OpenAiProvider::new(account)),
        "opencode" => Box::new(opencode::OpenCodeProvider::new(account)),
        "openrouter" => Box::new(openrouter::OpenRouterProvider::new(account)),
        other => bail!("unsupported provider: {other}"),
    })
}
pub fn available() -> [(&'static str, &'static str); 4] {
    [
        ("github_copilot", "GitHub Copilot"),
        ("openai", "OpenAI Codex"),
        ("opencode", "OpenCode Go"),
        ("openrouter", "OpenRouter"),
    ]
}
