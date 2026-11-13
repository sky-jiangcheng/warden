//! The read-only half of Phase 3: local commands the model may run.
//!
//! roadmap Phase 3 draws the line first — Human-in-the-loop, no fully
//! automatic Agent. The execution rules split cleanly in two:
//!
//! * **reads need no confirmation and ship here**: `git status` · `git diff`
//!   · `git log` · `git branch` · `git show` · directory listing · text
//!   reading · grep;
//! * **writes and build/test commands** (`npm run build`, `cargo test`)
//!   execute arbitrary repo-controlled scripts and are NOT in this layer —
//!   they wait for the confirmation step, the same order product.md set
//!   for the MCP write face. "危险命令默认禁止" starts as "only a fixed
//!   allowlist exists at all".
//!
//! The discipline, load-bearing for the whole surface:
//!
//! * **Fixed argv, never a shell.** Each tool names one git subcommand or
//!   one filesystem operation; the model supplies VALUES (paths, a rev, a
//!   pattern), never flags. Validation rejects anything that could read
//!   as an option (`-…`), traverse upward, arrive absolute, or parse as a
//!   revision range.
//! * **Confined to the thread's repository.** The child's cwd IS the
//!   repository; filesystem reads canonicalize and re-check containment,
//!   so a symlink out of the repo is refused, not read.
//! * **Bounded and killable.** Every child runs through the same capped
//!   reader and watchdog as the diff injection (`git.rs::run_confined`):
//!   one runner, written once — byte caps, a hard timeout, a killed child.
//!   A wedged command degrades to a tool error, never a frozen
//!   conversation.
//! * **Every execution is audited** (Phase 3: 每次执行记录到本地日志):
//!   one JSONL line per call in `exec-audit.jsonl` beside the provider
//!   config — time, repository, tool, the FULL argv, exit, bytes,
//!   truncation. Created owner-only (0600) like every sensitive file this
//!   app writes: the log records what the user's machine ran.
//! * **The complete command travels to the UI.** Each ToolEvent carries
//!   the argv it ran — "所有命令必须显示完整命令" without a second log.
//!
//! Naming: `warden_*`. The memory layer's face is `repotory_*` and belongs
//! to Cinder; these are Warden's own. The separation is also what keeps
//! the two whitelists from ever being confused for one contract copy.

use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use crate::git::{self, ProcOut};
use crate::llm::{ToolFunctionSpec, ToolSpec};

/// The tool names this layer answers. The generation loop routes here
/// BEFORE the MCP whitelist check.
pub const LOCAL_TOOLS: &[&str] = &[
    "warden_git_status",
    "warden_git_diff",
    "warden_git_log",
    "warden_git_branch",
    "warden_git_show",
    "warden_list_dir",
    "warden_read_file",
    "warden_grep",
];

/// Byte cap per command output. Big enough for a status or a focused read,
/// small enough that one tool round cannot flood the context.
const OUT_BYTES: usize = 6_000;
/// Build output is often a screenful of warnings before the answer. Big
/// enough to keep the failing test's name, still bounded.
const BUILD_OUT_BYTES: usize = 16_000;
const LIST_ENTRIES: usize = 200;
/// A pathspec list longer than this is the model losing the plot.
const MAX_PATH_ITEMS: usize = 8;

/// The write/build half of Phase 3 — the tools whose execution the user
/// must approve first. They are a CLOSED set, not a shell: each entry is
/// one fixed argv shape with values the model supplies, and nothing here
/// can be turned into "run whatever I say".
///
/// Why these and not arbitrary commands: `npm run build` and `cargo test`
/// execute code the REPOSITORY controls (a `package.json` script, a build
/// script, a test harness). That is exactly why they need the human gate
/// Phase 3 names, and exactly why the gate — not a sandbox — is the right
/// answer: the user already trusts that repository enough to work in it.
pub const WRITE_TOOLS: &[&str] = &[
    "warden_npm_build",
    "warden_cargo_test",
    "warden_git_add",
    "warden_git_commit",
];

/// The ceiling for a build/test command. A cold `cargo test` on a real
/// crate is minutes; anything past this is not "slow", it is stuck.
const BUILD_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(300);

pub struct Executed {
    /// The answer handed to the model — an error sentence when the command
    /// refused or failed: a refusal is information, not a silent void.
    pub text: String,
    /// The full command line for the UI chip and the audit; empty for a
    /// pre-flight refusal, which never ran anything.
    pub command: String,
    pub ok: bool,
    pub truncated: bool,
    pub exit: Option<i32>,
}

fn object(properties: Value, required: &[&str]) -> Value {
    json!({
        "type": "object",
        "properties": properties,
        "required": required,
    })
}

fn string_prop(description: &str) -> Value {
    json!({"type": "string", "description": description})
}

/// Builds the model-facing specs for the local face. These schemas are
/// WARDEN's own contract — the server does not know these tools — so they
/// are written here deliberately; the no-copy rule covers the memory
/// layer's surface, not Warden's.
pub fn tool_specs() -> Vec<ToolSpec> {
    let entries: Vec<(&str, &str, Value)> = vec![
        (
            "warden_git_status",
            "Show the repository's working-tree status (git status --short --branch).",
            object(
                json!({"paths": {"type": "array", "items": {"type": "string"}, "description": "Optional relative paths to restrict the status to"}}),
                &[],
            ),
        ),
        (
            "warden_git_diff",
            "Show the uncommitted diff of the working tree. stat=true lists files only.",
            object(
                json!({
                    "stat": {"type": "boolean", "description": "List changed files instead of the patch"},
                    "paths": {"type": "array", "items": {"type": "string"}, "description": "Optional relative paths to restrict the diff to"},
                }),
                &[],
            ),
        ),
        (
            "warden_git_log",
            "Show recent commits, one line each (hash, date, author, subject).",
            object(
                json!({"limit": {"type": "number", "description": "How many commits (1–50, default 20)"}}),
                &[],
            ),
        ),
        (
            "warden_git_branch",
            "List local branches and mark the current one.",
            object(json!({}), &[]),
        ),
        (
            "warden_git_show",
            "Show one commit by its ref: subject, author, date, changed files.",
            object(
                json!({"rev": string_prop("A single revision: commit hash or branch/tag name")}),
                &["rev"],
            ),
        ),
        (
            "warden_list_dir",
            "List a directory inside the repository (default the repository root).",
            object(
                json!({"path": string_prop("Relative directory path, default \".\"")}),
                &[],
            ),
        ),
        (
            "warden_read_file",
            "Read a text file inside the repository. Output is byte-capped; binary is refused.",
            object(
                json!({"path": string_prop("Relative file path")}),
                &["path"],
            ),
        ),
        (
            "warden_grep",
            "Search committed text in the repository with a fixed-string grep (git grep -n -I).",
            object(
                json!({
                    "pattern": string_prop("The literal text to find"),
                    "path": string_prop("Optional relative path to restrict the search to"),
                }),
                &["pattern"],
            ),
        ),
    ];
    entries
        .into_iter()
        .map(|(name, description, parameters)| ToolSpec {
            kind: "function".to_string(),
            function: ToolFunctionSpec {
                name: name.to_string(),
                description: description.to_string(),
                parameters: Some(parameters),
            },
        })
        .collect()
}

/// Whether a tool name belongs to the read-only layer.
pub fn is_local(name: &str) -> bool {
    LOCAL_TOOLS.contains(&name)
}

/// Whether a tool name belongs to the confirmation-gated layer.
pub fn is_write(name: &str) -> bool {
    WRITE_TOOLS.contains(&name)
}

/// Whether the name belongs to Warden's own local face at all.
pub fn is_warden_tool(name: &str) -> bool {
    is_local(name) || is_write(name)
}

/// The model-facing specs for the write/build set. Every one of them is
/// marked as needing approval, both in the description the model reads and
/// in the enforcement (the loop asks before it runs anything here).
pub fn write_specs() -> Vec<ToolSpec> {
    let entries: Vec<(&str, &str, Value)> = vec![
        (
            "warden_git_add",
            "Stage paths for the next commit. REQUIRES USER APPROVAL: the user sees the exact command and accepts or refuses it before anything runs.",
            object(
                json!({"paths": {"type": "array", "items": {"type": "string"}, "description": "Relative paths to stage; omit for all changes in the repository"}}),
                &[],
            ),
        ),
        (
            "warden_git_commit",
            "Create a commit from the staged changes. REQUIRES USER APPROVAL. The message is used verbatim.",
            object(
                json!({"message": string_prop("The commit message, one line and a body if needed")}),
                &["message"],
            ),
        ),
        (
            "warden_npm_build",
            "Run `npm run build` in the repository. REQUIRES USER APPROVAL: it executes the repository's own build script.",
            object(json!({}), &[]),
        ),
        (
            "warden_cargo_test",
            "Run `cargo test` in the repository. REQUIRES USER APPROVAL: it compiles and runs the repository's tests.",
            object(
                json!({"package": string_prop("Optional package name to test (--package)")}),
                &[],
            ),
        ),
    ];
    entries
        .into_iter()
        .map(|(name, description, parameters)| ToolSpec {
            kind: "function".to_string(),
            function: ToolFunctionSpec {
                name: name.to_string(),
                description: description.to_string(),
                parameters: Some(parameters),
            },
        })
        .collect()
}

/// The exact command a write/build tool WOULD run, for the confirmation card.
///
/// This is what the user approves, so it is computed by the same planner the
/// execution uses — a card that showed something different from what runs
/// would be worse than no card. `Err` carries the refusal (bad arguments),
/// which the loop hands back to the model instead of asking the user.
pub fn plan_write(name: &str, args: &Value, repository: &Path) -> Result<String, String> {
    let argv = build_write(name, args, repository)?;
    Ok(argv.join(" "))
}

fn build_write(name: &str, args: &Value, _repository: &Path) -> Result<Vec<String>, String> {
    let argv = match name {
        "warden_git_add" => {
            // `--all` is an OPTION, so it must precede `--`: `git add --
            // --all` stages the file literally named "--all" (which is what
            // the first version did, and it staged nothing).
            let mut argv = owned(&["git", "add"]);
            match args.get("paths") {
                None | Some(Value::Null) => argv.push("--all".to_string()),
                Some(Value::Array(items)) => {
                    if items.is_empty() {
                        return Err(
                            "paths must not be an empty array; omit it to stage everything".into(),
                        );
                    }
                    if items.len() > MAX_PATH_ITEMS {
                        return Err(format!("at most {MAX_PATH_ITEMS} paths"));
                    }
                    for item in items {
                        let path = item.as_str().ok_or("paths must be strings")?;
                        validate_rel(path, _repository)?;
                    }
                    argv.push("--".to_string());
                    argv.extend(
                        items
                            .iter()
                            .map(|item| item.as_str().expect("validated").to_string()),
                    );
                }
                Some(_) => return Err("paths must be an array of strings".into()),
            }
            argv
        }
        "warden_git_commit" => {
            let message = args
                .get("message")
                .and_then(Value::as_str)
                .ok_or("message is required")?;
            let message = message.trim();
            if message.is_empty() || message.len() > 5_000 {
                return Err("message must be 1–5000 bytes".into());
            }
            // One `-m` with the message as a single argv element: no shell,
            // no newline tricks, no second message the card would not show.
            owned(&["git", "commit", "-m", message])
        }
        "warden_npm_build" => owned(&["npm", "run", "build"]),
        "warden_cargo_test" => {
            let mut argv = owned(&["cargo", "test"]);
            if let Some(package) = args.get("package").and_then(Value::as_str) {
                if package.is_empty()
                    || package.len() > 200
                    || package.starts_with('-')
                    || !package
                        .chars()
                        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_'))
                {
                    return Err(format!("package contains unsafe characters: {package}"));
                }
                argv.push("--package".to_string());
                argv.push(package.to_string());
            }
            argv
        }
        other => return Err(format!("unknown write tool {other}")),
    };
    Ok(argv)
}

/// Runs an APPROVED write/build tool. The caller has already shown the user
/// `plan_write`'s exact command and received a yes; this function does not
/// ask again — it re-plans (same arguments, same command) and executes.
pub fn run_write(name: &str, args: &Value, repository: &Path) -> Executed {
    let argv = match build_write(name, args, repository) {
        Ok(argv) => argv,
        Err(reason) => return refused(reason),
    };
    let refs: Vec<&str> = argv.iter().map(String::as_str).collect();
    let command = refs.join(" ");
    // A build writes a lot: keep the tail-sized cap generous but bounded,
    // and let the longer timeout be the build-specific part.
    let executed = git::run_confined_within(repository, &refs, BUILD_OUT_BYTES, BUILD_TIMEOUT);
    match executed {
        Some(out) => child_answer(name, out, command),
        None => Executed {
            text: format!("the command could not run or timed out: {command}"),
            command,
            ok: false,
            truncated: false,
            exit: None,
        },
    }
}

/// Runs one local tool for a repository-scoped conversation.
///
/// Blocking (children and filesystem) — the caller runs it on the blocking
/// pool. Returns an outcome even for refusals; `ok=false` marks a
/// refusal/failure for the UI chip and the audit, and the text says why.
pub fn run(name: &str, args: &Value, repository: &Path) -> Executed {
    match name {
        "warden_list_dir" => list_dir(args, repository),
        "warden_read_file" => read_file(args, repository),
        _ => run_child(name, args, repository),
    }
}

fn run_child(name: &str, args: &Value, repository: &Path) -> Executed {
    let planned = build(name, args, repository);
    let argv = match planned {
        Ok(argv) => argv,
        Err(refusal) => {
            // A pre-flight refusal never ran: the command line it WOULD
            // have used stays empty, and the sentence names the rule.
            return refused(refusal);
        }
    };
    let refs: Vec<&str> = argv.iter().map(String::as_str).collect();
    let command = refs.join(" ");
    let executed = git::run_confined(repository, &refs, OUT_BYTES);
    match executed {
        Some(out) => child_answer(name, out, command),
        None => Executed {
            text: format!("the command could not run or timed out: {command}"),
            command,
            ok: false,
            truncated: false,
            exit: None,
        },
    }
}

fn child_answer(name: &str, out: ProcOut, command: String) -> Executed {
    // `git grep` exits 1 for "no matches" — that is the answer, not a
    // failure. Every other non-zero exit is the child refusing, and its
    // stdout was dropped, so the honest sentence is what ran and what it
    // returned.
    let no_match = name == "warden_grep" && out.exit == Some(1);
    let ok = out.exit == Some(0) || no_match;
    let text = if !ok {
        match out.exit {
            Some(code) => format!("the command failed with exit code {code}: {command}"),
            None => format!("the command was killed before it finished: {command}"),
        }
    } else if no_match {
        "no matches".to_string()
    } else if out.text.trim().is_empty() {
        "(the command returned no output)".to_string()
    } else if out.truncated {
        format!("{}\n(output truncated by the client)", out.text.trim_end())
    } else {
        out.text
    };
    Executed {
        text,
        command,
        ok,
        truncated: out.truncated,
        exit: out.exit,
    }
}

/// Plans the fixed argv for one child tool; Err is the refusal sentence.
fn build(name: &str, args: &Value, repository: &Path) -> Result<Vec<String>, String> {
    let argv = match name {
        "warden_git_status" => {
            let mut argv = owned(&["git", "--no-pager", "status", "--short", "--branch"]);
            extend_pathspecs(&mut argv, args.get("paths"), repository)?;
            argv
        }
        "warden_git_diff" => {
            let mut argv = owned(&["git", "--no-pager", "diff", "--no-color"]);
            if args.get("stat").and_then(Value::as_bool).unwrap_or(false) {
                argv.push("--stat".to_string());
            } else {
                argv.push("--unified=2".to_string());
            }
            extend_pathspecs(&mut argv, args.get("paths"), repository)?;
            argv
        }
        "warden_git_log" => {
            // Explicit null must not silently become 20 — `get` on a JSON
            // null yields Value::Null, and as_u64 on it is None either way,
            // so the default below is the single documented fallback for
            // "absent or null".
            let limit = args.get("limit").and_then(Value::as_u64).unwrap_or(20);
            if !(1..=50).contains(&limit) {
                return Err("limit must be between 1 and 50".into());
            }
            owned(&[
                "git",
                "--no-pager",
                "log",
                "-n",
                &limit.to_string(),
                "--date=short",
                "--pretty=%h %ad %an %s",
            ])
        }
        "warden_git_branch" => owned(&["git", "--no-pager", "branch", "--list"]),
        "warden_git_show" => {
            let rev = args
                .get("rev")
                .and_then(Value::as_str)
                .ok_or("rev is required")?;
            validate_rev(rev)?;
            // No `--` before `rev`: after `--` git treats everything as a
            // pathspec, so `git show --stat -- HEAD` prints nothing at
            // all. `validate_rev` already guarantees the rev cannot look
            // like an option (no leading '-'/'+'), which is the property
            // `--` would have bought here.
            owned(&[
                "git",
                "--no-pager",
                "show",
                "--stat",
                "--no-color",
                "--format=%h %ad %an %s",
                "--date=short",
                rev,
            ])
        }
        "warden_grep" => {
            let pattern = args
                .get("pattern")
                .and_then(Value::as_str)
                .ok_or("pattern is required")?;
            if pattern.is_empty() || pattern.len() > 200 {
                return Err("pattern must be 1–200 bytes".into());
            }
            // No `--max-count`: that is grep(1)'s option, not git grep's
            // (git grep exits 129 with "unknown option"). The byte cap in
            // the shared runner is what bounds the answer, and `-I` keeps
            // binaries out of it — the two controls this surface needs.
            let mut argv = owned(&[
                "git",
                "--no-pager",
                "grep",
                "-n",
                "-I",
                "--fixed-strings",
                "-e",
                pattern,
            ]);
            if let Some(path) = args.get("path").and_then(Value::as_str) {
                validate_rel(path, repository)?;
                argv.extend(owned(&["--", path]));
            }
            argv
        }
        other => return Err(format!("unknown local tool {other}")),
    };
    Ok(argv)
}

fn owned(items: &[&str]) -> Vec<String> {
    items.iter().map(|s| s.to_string()).collect()
}

fn extend_pathspecs(
    argv: &mut Vec<String>,
    value: Option<&Value>,
    repository: &Path,
) -> Result<(), String> {
    let items = match value.filter(|v| !v.is_null()) {
        None => return Ok(()),
        Some(items) => items
            .as_array()
            .ok_or("paths must be an array of strings")?,
    };
    if items.len() > MAX_PATH_ITEMS {
        return Err(format!("at most {MAX_PATH_ITEMS} paths"));
    }
    if items.is_empty() {
        return Ok(());
    }
    for item in items {
        let path = item.as_str().ok_or("paths must be strings")?;
        validate_rel(path, repository)?;
    }
    argv.push("--".to_string());
    argv.extend(
        items
            .iter()
            .map(|item| item.as_str().expect("validated").to_string()),
    );
    Ok(())
}

/// A path argument: relative, no traversal, never an option. git pathspecs
/// are textual, so the textual rules are the containment; the filesystem
/// tools additionally canonicalize and re-check (read_file).
fn validate_rel(path: &str, _repository: &Path) -> Result<(), String> {
    if path.is_empty() {
        return Err("path must not be empty".into());
    }
    if path.starts_with('-') {
        return Err(format!("path must not start with '-': {path}"));
    }
    if path.starts_with('/') || path.contains('\\') || Path::new(path).is_absolute() {
        return Err(format!("path must be relative to the repository: {path}"));
    }
    if path.split('/').any(|part| part == "..") {
        return Err(format!("path must not traverse upward: {path}"));
    }
    Ok(())
}

/// A revision the child may be handed: safe characters only, cannot read
/// as an option, cannot be a range. `--` before it closes the option space.
fn validate_rev(rev: &str) -> Result<(), String> {
    if rev.is_empty() || rev.len() > 100 || rev.starts_with('-') || rev.starts_with('+') {
        return Err("rev must be 1–100 characters and cannot start with '-' or '+'".into());
    }
    if !rev
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '/' | '-'))
    {
        return Err(format!("rev contains unsafe characters: {rev}"));
    }
    if rev.contains("..") || rev.contains("@{") {
        return Err(format!(
            "rev must be a single revision, not a range or query: {rev}"
        ));
    }
    Ok(())
}

fn list_dir(args: &Value, repository: &Path) -> Executed {
    let rel = args.get("path").and_then(Value::as_str).unwrap_or(".");
    let command = format!("list-dir {rel}");
    if let Err(reason) = validate_rel_or_dot(rel, repository) {
        return refused(reason);
    }
    let Ok(entries) = std::fs::read_dir(repository.join(rel)) else {
        return refused(format!("the directory could not be read: {rel}"));
    };
    let mut names: Vec<String> = entries
        .flatten()
        .map(|entry| {
            let name = entry.file_name().to_string_lossy().into_owned();
            if entry.path().is_dir() {
                format!("{name}/")
            } else {
                name
            }
        })
        .collect();
    names.sort();
    let truncated = names.len() > LIST_ENTRIES;
    names.truncate(LIST_ENTRIES);
    Executed {
        text: names.join("\n"),
        command,
        ok: true,
        truncated,
        exit: Some(0),
    }
}

fn read_file(args: &Value, repository: &Path) -> Executed {
    let Some(rel) = args.get("path").and_then(Value::as_str) else {
        return refused("path is required".into());
    };
    let command = format!("read-file {rel}");
    if let Err(reason) = validate_rel(rel, repository) {
        return refused(reason);
    }
    // Containment the hard way: canonicalize both sides and compare. A
    // symlink that points out of the repository is refused, not read —
    // the textual rules alone would let `escape` reach the machine.
    let (Ok(root), Ok(target)) = (
        repository.canonicalize(),
        repository.join(rel).canonicalize(),
    ) else {
        return refused(format!("the file could not be read: {rel}"));
    };
    if !target.starts_with(&root) {
        return refused("path escapes the repository".into());
    }
    let Ok(bytes) = std::fs::read(&target) else {
        return refused(format!("the file could not be read: {rel}"));
    };
    // Text-ish check: a NUL marks binary; handing it to the model would be
    // noise dressed as content.
    if bytes.contains(&0) {
        return refused(format!("{rel} is binary; the read-only face serves text"));
    }
    let truncated = bytes.len() > OUT_BYTES;
    let mut text = String::from_utf8_lossy(&bytes[..bytes.len().min(OUT_BYTES)]).into_owned();
    if truncated {
        text.push_str("\n(file truncated by the client; read the file for the rest)");
    }
    Executed {
        text,
        command,
        ok: true,
        truncated,
        exit: Some(0),
    }
}

fn validate_rel_or_dot(path: &str, repository: &Path) -> Result<(), String> {
    if path == "." {
        return Ok(());
    }
    validate_rel(path, repository)
}

fn refused(reason: String) -> Executed {
    Executed {
        text: reason,
        command: String::new(),
        ok: false,
        truncated: false,
        exit: None,
    }
}

/// The local execution log (Phase 3: 每次执行记录到本地日志).
///
/// One JSONL line per command: time, repository, tool, the FULL argv,
/// exit, bytes, truncation. Written beside the provider config with the
/// same owner-only discipline as `secrets.json` — the log records the
/// paths and commands the user's machine ran, which is exactly the thing
/// not to hand to other local users.
///
/// The path arrives through a `OnceLock` because the state is managed
/// before the app's config directory exists: setup attaches it once, and
/// every later generation reads it through `&self` — no mutex, no race,
/// and a command executed before the attach simply goes unaudited rather
/// than blocking on a directory that is not resolved yet.
#[derive(Clone, Default)]
pub struct AuditLog {
    path: std::sync::OnceLock<PathBuf>,
}

impl AuditLog {
    /// Attaches the log file. First call wins; later calls are ignored
    /// (the setup hook runs once, and tests use `at`).
    pub fn attach(&self, path: PathBuf) {
        let _ = self.path.set(path);
    }

    /// A log aimed at `path` — the test constructor, and any future code
    /// that wants a standalone audit sink.
    pub fn at(path: PathBuf) -> Self {
        let log = Self::default();
        log.attach(path);
        log
    }

    /// Appends one owner-only line describing a finished execution.
    /// Best effort by design: a failed audit line must not break a working
    /// command — but it must not pass silently either.
    pub fn record(&self, entry: &Value) {
        let Some(path) = self.path.get() else { return };
        let line = format!("{entry}\n");
        use std::io::Write;
        let result = std::fs::OpenOptions::new()
            .append(true)
            .create(true)
            .open(path)
            .and_then(|mut file| {
                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt;
                    let _ = file.set_permissions(std::fs::Permissions::from_mode(0o600));
                }
                file.write_all(line.as_bytes())
            });
        if let Err(error) = result {
            eprintln!("[exec-audit] the line could not be written: {error}");
        }
    }
}

/// The audit entry for one execution, in the fields Phase 3 names:
/// what ran (full argv), where, how it ended, how much it returned.
///
/// `at` is Unix seconds, read at execution time: the log is ordered by
/// line, and the clock is the machine's own. Seconds are enough to place
/// a command in a session; sub-second precision would be noise.
pub fn audit_entry(
    at: i64,
    repository: &str,
    name: &str,
    out: &Executed,
    answer_bytes: usize,
) -> Value {
    json!({
        "at": at,
        "repository": repository,
        "tool": name,
        "argv": out.command,
        "exit": out.exit,
        "ok": out.ok,
        "bytes": answer_bytes,
        "truncated": out.truncated,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn repo(name: &str, dirty: bool, commits: usize) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "warden-exec-{}-{name}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos(),
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let git = |args: &[&str]| {
            let ok = std::process::Command::new("git")
                .arg("-C")
                .arg(&dir)
                .args(args)
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .status()
                .unwrap()
                .success();
            assert!(ok, "git {args:?}");
        };
        git(&["init", "-b", "main"]);
        git(&["config", "user.email", "t@t"]);
        git(&["config", "user.name", "t"]);
        std::fs::write(dir.join("alpha.txt"), "needle inside alpha\n").unwrap();
        git(&["add", "."]);
        git(&["commit", "-m", "first commit"]);
        for i in 1..commits {
            std::fs::write(dir.join(format!("f{i}.txt")), "x\n").unwrap();
            git(&["add", "."]);
            let message = format!("commit {i}");
            git(&["commit", "-m", &message]);
        }
        if dirty {
            std::fs::write(dir.join("alpha.txt"), "needle inside alpha\ntouched\n").unwrap();
        }
        dir
    }

    #[test]
    fn status_names_the_dirty_file_and_shows_the_whole_command() {
        let dir = repo("status", true, 1);
        let out = run("warden_git_status", &json!({}), &dir);
        assert!(out.ok, "{}", out.text);
        assert!(out.text.contains("alpha.txt"), "{}", out.text);
        assert_eq!(out.command, "git --no-pager status --short --branch");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn diff_paths_stat_and_empty() {
        let dir = repo("diff", true, 1);
        let full = run("warden_git_diff", &json!({}), &dir);
        assert!(full.ok, "{}", full.text);
        assert!(full.text.contains("+touched"), "{}", full.text);
        let stat = run("warden_git_diff", &json!({"stat": true}), &dir);
        assert!(stat.ok && stat.text.contains("alpha.txt"));
        let scoped = run("warden_git_diff", &json!({"paths": ["alpha.txt"]}), &dir);
        assert!(scoped.ok);
        assert!(
            scoped.command.ends_with("-- alpha.txt"),
            "{}",
            scoped.command
        );
        let clean = run("warden_git_status", &json!({"paths": ["f1.txt"]}), &dir);
        let _ = clean;
        let empty = repo("diff-clean", false, 1);
        let none = run("warden_git_diff", &json!({}), &empty);
        assert!(none.ok && none.text.contains("no output"), "{}", none.text);
        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::remove_dir_all(&empty);
    }

    #[test]
    fn log_honours_the_cap_and_refuses_a_runaway_limit() {
        let dir = repo("log", false, 4);
        let out = run("warden_git_log", &json!({"limit": 2}), &dir);
        assert!(out.ok, "{}", out.text);
        assert_eq!(out.text.lines().count(), 2, "{}", out.text);
        let refused_run = run("warden_git_log", &json!({"limit": 5000}), &dir);
        assert!(!refused_run.ok);
        assert!(refused_run.text.contains("between 1 and 50"));
        assert!(refused_run.command.is_empty(), "a refusal ran nothing");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn revs_and_paths_that_could_read_as_options_are_refused() {
        let dir = repo("guard", false, 1);
        for bad in [
            "-b", "--help", "HEAD~5", "a..b", "main; rm", "sp ace", "@{u}", "-C/tmp", "",
        ] {
            let out = run("warden_git_show", &json!({"rev": bad}), &dir);
            assert!(!out.ok, "must refuse {bad}: {}", out.text);
        }
        for bad in ["../etc/passwd", "/etc/passwd", "-u", "a/../../b", "C:\\x"] {
            let out = run("warden_git_status", &json!({"paths": [bad]}), &dir);
            assert!(!out.ok, "must refuse {bad}");
        }
        let ok = run("warden_git_show", &json!({"rev": "HEAD"}), &dir);
        assert!(ok.ok, "{}", ok.text);
        assert!(ok.text.contains("first commit"), "{}", ok.text);
        let missing = run("warden_git_show", &json!({"rev": "deadbeefdead"}), &dir);
        assert!(
            !missing.ok,
            "an unknown rev fails with an exit code, honestly"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn branch_lists_and_grep_answers_with_no_match_as_an_answer() {
        let dir = repo("branch-grep", false, 1);
        let branches = run("warden_git_branch", &json!({}), &dir);
        assert!(branches.ok, "{}", branches.text);
        assert!(branches.text.contains("main"), "{}", branches.text);
        let hit = run("warden_grep", &json!({"pattern": "needle"}), &dir);
        assert!(hit.ok, "{}", hit.text);
        assert!(hit.text.contains("alpha.txt"), "{}", hit.text);
        let miss = run("warden_grep", &json!({"pattern": "no-such-text"}), &dir);
        assert!(
            miss.ok,
            "exit 1 for no matches is the answer: {}",
            miss.text
        );
        assert_eq!(miss.text, "no matches");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn filesystem_tools_stay_inside_and_bounded() {
        let dir = repo("fs", false, 1);
        std::fs::write(dir.join("big.txt"), "a".repeat(OUT_BYTES + 500)).unwrap();
        std::fs::write(dir.join("bin.dat"), [0u8, 1, 2]).unwrap();
        let outside = std::env::temp_dir().join(format!("warden-exec-out-{}", std::process::id()));
        std::fs::write(&outside, "secret").unwrap();
        std::os::unix::fs::symlink(&outside, dir.join("escape")).unwrap();

        let listed = run("warden_list_dir", &json!({}), &dir);
        assert!(listed.ok);
        assert!(listed.text.contains("alpha.txt"), "{}", listed.text);

        let big = run("warden_read_file", &json!({"path": "big.txt"}), &dir);
        assert!(big.ok);
        assert!(big.truncated);
        assert!(
            big.text.contains("truncated by the client"),
            "the cap must be named"
        );

        let binary = run("warden_read_file", &json!({"path": "bin.dat"}), &dir);
        assert!(!binary.ok, "binary is refused, not dumped");

        let symlinked = run("warden_read_file", &json!({"path": "escape"}), &dir);
        assert!(!symlinked.ok, "a symlink outside the repo must not be read");

        let escaped = run(
            "warden_read_file",
            &json!({"path": "alpha.txt/../../outside"}),
            &dir,
        );
        assert!(!escaped.ok);

        let missing_path = run("warden_read_file", &json!({}), &dir);
        assert!(!missing_path.ok, "path is required");
        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::remove_file(&outside);
    }

    #[test]
    fn the_audit_log_records_full_argv_and_is_owner_only() {
        let dir = repo("audit", true, 1);
        let log = std::env::temp_dir().join(format!(
            "warden-exec-audit-{}-{}.jsonl",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let audit = AuditLog::at(log.clone());
        let out = run("warden_git_status", &json!({}), &dir);
        // Unix seconds, as the production path writes them.
        audit.record(&audit_entry(
            1_763_000_000,
            "/repo",
            "warden_git_status",
            &out,
            out.text.len(),
        ));
        let body = std::fs::read_to_string(&log).unwrap();
        assert!(
            body.contains("git --no-pager status --short --branch"),
            "{body}"
        );
        assert!(body.contains("\"exit\":0"), "{body}");
        assert!(body.contains("\"at\":1763000000"), "{body}");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&log).unwrap().permissions().mode() & 0o777;
            assert_eq!(
                mode, 0o600,
                "the audit file must be owner-only from the first line"
            );
        }
        let _ = std::fs::remove_file(&log);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn plan_write_shows_the_exact_command_for_every_tool() {
        let args = json!({"message": "commit this for me"});
        assert_eq!(
            plan_write("warden_git_commit", &args, Path::new("/r")).unwrap(),
            "git commit -m commit this for me"
        );
        assert_eq!(
            plan_write("warden_git_add", &json!({}), Path::new("/r")).unwrap(),
            "git add --all"
        );
        assert_eq!(
            plan_write(
                "warden_git_add",
                &json!({"paths": ["a.txt"]}),
                Path::new("/r")
            )
            .unwrap(),
            "git add -- a.txt"
        );
        assert_eq!(
            plan_write("warden_npm_build", &json!({}), Path::new("/r")).unwrap(),
            "npm run build"
        );
        assert_eq!(
            plan_write("warden_cargo_test", &json!({}), Path::new("/r")).unwrap(),
            "cargo test"
        );
        // A bad argument is a refusal BEFORE the user is asked anything.
        assert!(plan_write("warden_git_commit", &json!({}), Path::new("/r")).is_err());
        assert!(
            plan_write(
                "warden_git_add",
                &json!({"paths": ["../x"]}),
                Path::new("/r")
            )
            .is_err()
        );
    }

    #[test]
    fn specs_cover_the_surface() {
        let specs = tool_specs();
        assert_eq!(specs.len(), LOCAL_TOOLS.len());
        for name in LOCAL_TOOLS {
            assert!(specs.iter().any(|s| s.function.name == *name));
            assert!(is_local(name));
        }
        assert!(!is_local("repotory_metrics"));
        assert!(!is_local("warden_delete_everything"));
    }
}
