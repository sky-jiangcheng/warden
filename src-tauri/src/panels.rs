//! The Phase 4 memory panels: three READ-ONLY views over what Cinder knows.
//!
//! The phase's own framing is the design constraint: this is where the
//! copilot separates itself from a plain chat window, and **the panels are
//! read-only — approval always happens in Cinder Desktop** (product.md open
//! question 2, settled as option C). So every tool reachable from here is a
//! read on the memory layer's face; nothing in this module can approve,
//! reject, create or edit anything, and no new MCP tool is called that the
//! main repo has not published.
//!
//! Three panels, three different sources — deliberately not one merged blob:
//!
//! * **context**: the local repository state (path, branch, uncommitted
//!   diff) that the conversation is actually scoped to. This one does NOT
//!   come from Cinder: it is this machine's working tree, and asking the
//!   memory layer about it would answer a different question.
//! * **knowledge**: `repotory_wiki_search` with an empty query — the browse
//!   feed of APPROVED pages. Unapproved pages never appear on the MCP face
//!   at all, which is why this panel cannot show them and must not pretend
//!   otherwise.
//! * **memory**: `repotory_context` for the repository — the previous
//!   session's handoff leads that briefing, which is exactly "跨会话记忆".
//!
//! Each panel reports its OWN failure. A memory layer that is down must not
//! make the local context panel look broken, and vice versa: the price of
//! one combined payload is that one dead optional dependency blanks a screen
//! full of working facts.

use serde::Serialize;
use serde_json::Value;

use crate::conversations::call_text;
use crate::git;
use crate::mcp_client::McpSession;

/// How many approved pages the browse feed asks for. Enough to browse, few
/// enough that the panel stays a glance rather than a table.
const KNOWLEDGE_LIMIT: u64 = 20;

#[derive(Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct Panels {
    pub context: ContextPanel,
    pub knowledge: KnowledgePanel,
    pub memory: MemoryPanel,
}

/// The local working state. Never a Cinder fact.
#[derive(Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct ContextPanel {
    /// The repository this conversation is scoped to, or None for a
    /// conversation with no repository (the panel then says so instead of
    /// showing an empty one).
    pub repository: Option<String>,
    /// The current branch, when git can tell us.
    pub branch: Option<String>,
    /// `git diff --stat` output — the same snapshot the model's prompt is
    /// built from, so the panel and the answer describe one tree.
    pub diff_stat: Option<String>,
    /// Why the panel has less than it should, when it does.
    pub problem: Option<String>,
}

/// The approved-knowledge browse feed.
#[derive(Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct KnowledgePanel {
    pub pages: Vec<KnowledgePage>,
    /// True when the server answered, even with zero pages — the difference
    /// between "nothing approved yet" and "the memory layer did not answer"
    /// is the whole reason this field exists.
    pub connected: bool,
    pub problem: Option<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct KnowledgePage {
    pub id: i64,
    pub title: String,
    pub slug: String,
    pub kind: String,
    /// 0 means a global page (ADR-0031) — a fact the reader can see.
    pub project_id: i64,
    pub updated_at: String,
    pub snippet: String,
}

/// The cross-session record: what the last session left behind.
#[derive(Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct MemoryPanel {
    /// Cinder's own `repotory_context` briefing, verbatim. It leads with the
    /// previous handoff, which is the "可继续任务" part of the panel; the
    /// briefing is Markdown the server rendered, so this is prose, not a
    /// structure this client invented.
    pub briefing: Option<String>,
    pub connected: bool,
    pub problem: Option<String>,
}

/// Builds all three panels.
///
/// Blocking git work runs on the blocking pool by the caller; the two Cinder
/// reads are async and each records its own failure without touching the
/// other panel.
pub async fn load(session: &McpSession, repository: Option<&str>) -> Panels {
    let context = context_panel(repository).await;
    let knowledge = knowledge_panel(session).await;
    let memory = memory_panel(session, repository).await;
    Panels {
        context,
        knowledge,
        memory,
    }
}

async fn context_panel(repository: Option<&str>) -> ContextPanel {
    let Some(repository) = repository.filter(|path| !path.trim().is_empty()) else {
        return ContextPanel {
            problem: Some("这个对话没有附加仓库。".into()),
            ..ContextPanel::default()
        };
    };
    let repository_owned = repository.to_string();
    let (branch, diff_stat) = match tauri::async_runtime::spawn_blocking(move || {
        (
            current_branch(&repository_owned),
            diff_stat(&repository_owned),
        )
    })
    .await
    {
        Ok(pair) => pair,
        Err(error) => {
            return ContextPanel {
                repository: Some(repository.to_string()),
                problem: Some(format!("读取本机工作区失败：{error}")),
                ..ContextPanel::default()
            };
        }
    };
    let problem = if branch.is_none() {
        Some("这个目录不是 git 仓库，或 git 不可用。".to_string())
    } else {
        None
    };
    ContextPanel {
        repository: Some(repository.to_string()),
        branch,
        diff_stat,
        problem,
    }
}

fn current_branch(repository: &str) -> Option<String> {
    let out = git::run_confined(
        std::path::Path::new(repository),
        &["git", "rev-parse", "--abbrev-ref", "HEAD"],
        BRANCH_BYTES,
    )?;
    if out.exit != Some(0) {
        return None;
    }
    let branch = out.text.trim();
    if branch.is_empty() {
        None
    } else {
        Some(branch.to_string())
    }
}

fn diff_stat(repository: &str) -> Option<String> {
    let out = git::run_confined(
        std::path::Path::new(repository),
        &["git", "--no-pager", "diff", "--stat", "--no-color"],
        DIFF_STAT_BYTES,
    )?;
    if out.exit != Some(0) {
        return None;
    }
    let text = out.text.trim();
    if text.is_empty() {
        // A clean tree is a fact worth stating, not an absence to hide.
        Some("工作区干净，没有未提交的改动。".to_string())
    } else {
        Some(text.to_string())
    }
}

const BRANCH_BYTES: usize = 400;
const DIFF_STAT_BYTES: usize = 4_000;

async fn knowledge_panel(session: &McpSession) -> KnowledgePanel {
    // Empty query is the BROWSE feed (the server's own docs say so): the
    // FTS path answers "what matches", this answers "what is published".
    match call_text(
        session,
        "repotory_wiki_search",
        serde_json::json!({ "query": "", "limit": KNOWLEDGE_LIMIT }),
    )
    .await
    {
        Ok(text) => match serde_json::from_str::<Value>(&text) {
            Ok(Value::Array(items)) => KnowledgePanel {
                pages: items.iter().filter_map(knowledge_page).collect(),
                connected: true,
                problem: None,
            },
            // The tool documents an array. Anything else is a contract
            // change, and reading it as zero pages would report "nothing
            // approved" for a healthy server that simply answered
            // differently — the exact confusion this panel must not create.
            Ok(other) => KnowledgePanel {
                connected: true,
                problem: Some(format!("知识页返回了意料之外的形状：{other}")),
                ..KnowledgePanel::default()
            },
            Err(error) => KnowledgePanel {
                connected: false,
                problem: Some(format!("知识页返回的内容不是 JSON：{error}")),
                ..KnowledgePanel::default()
            },
        },
        Err(error) => KnowledgePanel {
            connected: false,
            problem: Some(error),
            ..KnowledgePanel::default()
        },
    }
}

fn knowledge_page(value: &Value) -> Option<KnowledgePage> {
    Some(KnowledgePage {
        id: value.get("id")?.as_i64()?,
        title: value.get("title")?.as_str()?.to_string(),
        slug: value
            .get("slug")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string(),
        kind: value
            .get("kind")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string(),
        project_id: value.get("project_id").and_then(Value::as_i64).unwrap_or(0),
        updated_at: value
            .get("updated_at")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string(),
        snippet: value
            .get("snippet")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string(),
    })
}

async fn memory_panel(session: &McpSession, repository: Option<&str>) -> MemoryPanel {
    // `path` rather than project_id: the server resolves the workspace to
    // the right repository server-side, and a client that guessed the id
    // would be duplicating a resolution rule it does not own.
    let mut arguments = serde_json::json!({});
    if let Some(path) = repository.filter(|path| !path.trim().is_empty()) {
        arguments["path"] = Value::String(path.to_string());
    }
    match call_text(session, "repotory_context", arguments).await {
        Ok(text) => MemoryPanel {
            briefing: Some(text),
            connected: true,
            problem: None,
        },
        Err(error) => MemoryPanel {
            connected: false,
            problem: Some(error),
            ..MemoryPanel::default()
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_knowledge_page_reads_the_servers_field_names() {
        let page = knowledge_page(&json!({
            "id": 7,
            "slug": "knot-icon",
            "title": "The Knot",
            "kind": "concept",
            "project_id": 0,
            "updated_at": "2026-11-12T00:00:00Z",
            "snippet": "an eight-fold knot"
        }))
        .expect("a full row parses");
        assert_eq!(page.id, 7);
        assert_eq!(page.title, "The Knot");
        // project_id 0 is a GLOBAL page — the panel shows it as such rather
        // than treating 0 as "missing".
        assert_eq!(page.project_id, 0);
        assert_eq!(page.snippet, "an eight-fold knot");
    }

    #[test]
    fn a_row_without_an_id_is_skipped_rather_than_invented() {
        // The id is the handle the detail view would need; showing a page
        // the user cannot open would be worse than omitting it.
        assert!(knowledge_page(&json!({"title": "no id"})).is_none());
        assert!(knowledge_page(&json!({"id": "not a number"})).is_none());
    }

    #[test]
    fn optional_text_fields_degrade_to_empty_strings() {
        let page = knowledge_page(&json!({"id": 1, "title": "t"})).unwrap();
        assert_eq!(page.slug, "");
        assert_eq!(page.kind, "");
        assert_eq!(page.updated_at, "");
        assert_eq!(page.snippet, "");
        assert_eq!(page.project_id, 0);
    }

    #[test]
    fn a_clean_tree_says_so_instead_of_hiding() {
        let dir = std::env::temp_dir().join(format!("warden-panel-clean-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let run = |args: &[&str]| {
            assert!(
                std::process::Command::new("git")
                    .arg("-C")
                    .arg(&dir)
                    .args(args)
                    .stdout(std::process::Stdio::null())
                    .stderr(std::process::Stdio::null())
                    .status()
                    .unwrap()
                    .success()
            );
        };
        run(&["init", "-b", "main"]);
        run(&["config", "user.email", "t@t"]);
        run(&["config", "user.name", "t"]);
        std::fs::write(dir.join("a.txt"), "x\n").unwrap();
        run(&["add", "."]);
        run(&["commit", "-m", "first"]);

        assert_eq!(
            current_branch(dir.to_str().unwrap()).as_deref(),
            Some("main")
        );
        let stat = diff_stat(dir.to_str().unwrap()).expect("a clean tree still answers");
        assert!(stat.contains("干净"), "{stat}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_non_repository_has_no_branch_and_no_diff() {
        let dir = std::env::temp_dir().join(format!("warden-panel-plain-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        // Only assert when the tmp parent is not itself a repository, so the
        // test says the same thing on every machine.
        let parent_is_repo = dir
            .parent()
            .map(|p| p.join(".git").exists())
            .unwrap_or(false);
        if !parent_is_repo {
            assert!(current_branch(dir.to_str().unwrap()).is_none());
            assert!(diff_stat(dir.to_str().unwrap()).is_none());
        }
        let _ = std::fs::remove_dir_all(&dir);
    }
}
