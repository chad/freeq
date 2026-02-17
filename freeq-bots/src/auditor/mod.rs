//! Architecture Auditor bot.
//!
//! Triggered by `/audit <github-url>` — clones the repo, analyzes structure,
//! and posts findings: system diagram, bottlenecks, coupling, suggestions.

use anyhow::Result;
use std::path::Path;

use crate::llm::LlmClient;
use crate::output::{self, AgentId};
use crate::tools::{self, Workspace};
use freeq_sdk::client::ClientHandle;

fn auditor() -> AgentId {
    AgentId { role: "auditor".to_string(), color: None }
}

const SYSTEM: &str = r#"You are a principal engineer performing an architecture audit.

Given a repository's file tree and key file contents, produce a structured audit:

1. **System Overview**: What this project does, in one paragraph.
2. **Architecture Diagram**: ASCII diagram of major components and data flow.
3. **Stack**: Languages, frameworks, databases, infrastructure.
4. **Strengths**: What's well-designed (2-3 bullets).
5. **Risks & Bottlenecks**: Scaling risks, single points of failure, tight coupling (3-5 bullets).
6. **Refactor Suggestions**: Concrete, actionable improvements (3-5 bullets, prioritized).

Be specific. Reference actual file names and patterns you see. No generic advice."#;

/// Run an architecture audit on a GitHub repo or local path.
pub async fn audit(
    handle: &ClientHandle,
    channel: &str,
    target: &str,
    llm: &LlmClient,
    workspace_base: &Path,
) -> Result<()> {
    output::status(handle, channel, &auditor(), "🔍", &format!("Starting audit: {target}")).await?;

    let workspace = Workspace::create(workspace_base, "audit-workspace").await?;

    // Clone if it's a URL, otherwise treat as local
    if target.starts_with("http") || target.contains("github.com") {
        output::status(handle, channel, &auditor(), "📥", "Cloning repository...").await?;
        let clone_result = tools::shell(&workspace, &format!("git clone --depth 1 {target} repo 2>&1"), 60).await?;
        if clone_result.contains("fatal") {
            output::error(handle, channel, &auditor(), &format!("Clone failed: {clone_result}")).await?;
            return Ok(());
        }
    }

    // Find the repo root
    let repo_dir = if workspace.root.join("repo").exists() {
        workspace.root.join("repo")
    } else {
        workspace.root.clone()
    };

    // Gather file tree
    output::status(handle, channel, &auditor(), "📁", "Scanning file tree...").await?;
    let tree = tools::shell(&workspace, &format!(
        "find {} -type f -not -path '*/.git/*' -not -path '*/node_modules/*' -not -path '*/target/*' -not -path '*/__pycache__/*' -not -path '*/.next/*' | head -200 | sort",
        repo_dir.display()
    ), 10).await?;

    // Read key files
    output::status(handle, channel, &auditor(), "📄", "Reading key files...").await?;
    let key_files = [
        "Cargo.toml", "package.json", "requirements.txt", "go.mod",
        "Dockerfile", "docker-compose.yml", "docker-compose.yaml",
        "Procfile", "Makefile", ".github/workflows/ci.yml",
        "README.md", "src/main.rs", "src/lib.rs", "app.py", "main.py",
        "src/index.ts", "src/index.js", "main.go", "cmd/main.go",
    ];

    let mut file_contents = String::new();
    for name in &key_files {
        let path = repo_dir.join(name);
        if path.exists() {
            if let Ok(content) = tokio::fs::read_to_string(&path).await {
                let truncated = if content.len() > 3000 {
                    format!("{}... (truncated)", &content[..3000])
                } else {
                    content
                };
                file_contents.push_str(&format!("\n### {name}\n```\n{truncated}\n```\n"));
            }
        }
    }

    // Also try to find the main source structure
    let src_tree = tools::shell(&workspace, &format!(
        "find {} -maxdepth 3 -name '*.rs' -o -name '*.py' -o -name '*.ts' -o -name '*.go' -o -name '*.js' 2>/dev/null | head -50 | sort",
        repo_dir.display()
    ), 10).await.unwrap_or_default();

    // Build the audit prompt
    let prompt = format!(
        "Audit this repository.\n\n## File Tree\n```\n{tree}\n```\n\n## Source Files\n```\n{src_tree}\n```\n\n## Key File Contents\n{file_contents}"
    );

    output::status(handle, channel, &auditor(), "🧠", "Analyzing architecture...").await?;
    let analysis = llm.complete(SYSTEM, &prompt).await?;

    // Post the audit results
    output::say(handle, channel, &auditor(), &analysis).await?;

    // Clean up
    let _ = tokio::fs::remove_dir_all(&workspace.root).await;

    output::status(handle, channel, &auditor(), "✅", "Audit complete").await?;
    Ok(())
}
