//! Reading a repository's recent git state for the model request.
//!
//! This closes the last open loop of the Phase 2 promise "支持附加最近 git
//! diff": the repository path already travels in the system prompt, and the
//! read-only tool face already answers "what does the memory layer know";
//! what was missing is the working tree itself — the uncommitted change the
//! user is in the middle of. Without it the model is asked "why did I
//! change this line" and can only guess.
//!
//! Three rules, all load-bearing:
//!
//! * **Read-only and allowlisted.** One fixed argv, `git --no-pager diff`,
//!   never a shell, never a user-supplied argument beyond the repository
//!   directory itself. This is not Phase 3's tool-execution layer; it shares
//!   its discipline but not its surface.
//! * **Bounded output.** A dirty repository can diff to megabytes. The stat
//!   and the patch each get a cap, the reader stops at the cap, and a
//!   watchdog kills the git child after a hard timeout — a wedged git
//!   (index.lock, huge binary diffs) must degrade to "no diff in this
//!   request", never freeze a conversation.
//! * **Absence is silent, loss is not.** No git binary, or a path that is
//!   not a directory, means there is nothing to add and the request
//!   proceeds. But when the diff WAS there and a cap cut it short, the text
//!   says so — a model reading a truncated patch must know it is truncated.
//!   Same no-silent-degradation rule as the memory banner.

use std::io::Read;
use std::sync::{Arc, Mutex};

/// Hard cap (bytes) on the `--stat` block. A file list this long is already
/// past what a small local model can weigh against a question.
const STAT_BYTES: usize = 2_000;
/// Hard cap (bytes) on the unified-diff block. The excerpt is an
/// orientation, not a patch review; beyond this the model needs the file,
/// not the prompt.
const PATCH_BYTES: usize = 6_000;
/// A git child that has not delivered by now is not going to. Killed; the
/// diff block says the excerpt was unavailable; the request continues.
const GIT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);

/// The repository's uncommitted working state as prose for the system
/// prompt: `diff --stat` plus an excerpt of the working-tree patch.
///
/// `None` when there is nothing to say: no git binary, the path is not a
/// directory, or the diff came back empty (a clean tree — and a plain
/// non-repository, which answers with an empty stdout and a fatal on the
/// dropped stderr). Truncation and wedges are named INSIDE the text, because
/// "the model did not see all your changes" is information the answer
/// depends on.
pub fn recent_diff(repository: &str) -> Option<String> {
    if repository.trim().is_empty() || !std::path::Path::new(repository).is_dir() {
        return None;
    }
    let stat = run_git(
        repository,
        &["--no-pager", "diff", "--stat", "--no-color"],
        STAT_BYTES,
    )?;
    if stat.text.trim().is_empty() {
        return None;
    }
    let mut block = String::from("Current uncommitted changes (git diff --stat):\n");
    block.push_str(stat.text.trim_end());
    if stat.truncated {
        block.push_str("\n(file list truncated)");
    }
    block.push('\n');
    match run_git(
        repository,
        &["--no-pager", "diff", "--no-color", "--unified=2"],
        PATCH_BYTES,
    ) {
        Some(patch) if patch.text.trim().is_empty() => {}
        Some(patch) => {
            block.push_str("\nWorking-tree patch excerpt:\n");
            block.push_str(patch.text.trim_end());
            if patch.truncated {
                block.push_str(
                    "\n(patch excerpt truncated by the client; read the file for the rest)",
                );
            }
        }
        None => {
            block.push_str("\n(the patch excerpt was unavailable: git timed out or could not run)");
        }
    }
    Some(block)
}

struct GitOut {
    text: String,
    truncated: bool,
}

/// Runs one fixed git argv in `repository`, capturing stdout up to `cap`
/// bytes. `None` means git itself could not be spawned or produced no
/// readable pipe.
///
/// The child lives in an `Arc<Mutex<Option<..>>>` so two owners can reap it
/// without racing: a watchdog thread that kills after `GIT_TIMEOUT` (a
/// blocked git — full pipe nobody drains, index.lock — would otherwise hang
/// the conversation forever), and the reader, which kills early when the cap
/// is hit so the still-writing git unblocks before `wait`. Whoever `take()`s
/// the child reaps it; the other sees `None`. Killing never happens after a
/// reap, so a recycled pid is never targeted. No lock is held across the
/// blocking read or the `wait` of the other path... the watchdog does hold
/// it across kill+wait — bounded, because its own kill is what ends `wait`.
fn run_git(repository: &str, args: &[&str], cap: usize) -> Option<GitOut> {
    let child = std::process::Command::new("git")
        .arg("-C")
        .arg(repository)
        .args(args)
        .stdout(std::process::Stdio::piped())
        // git must not talk to a terminal; stderr is dropped: the answer is
        // in stdout and a "warning: ..." line is not worth another channel.
        .stderr(std::process::Stdio::null())
        .spawn()
        .ok()?;
    let shared = Arc::new(Mutex::new(Some(child)));

    {
        let shared = Arc::clone(&shared);
        std::thread::spawn(move || {
            std::thread::sleep(GIT_TIMEOUT);
            if let Some(mut wedged) = lock(&shared).take() {
                let _ = wedged.kill();
                let _ = wedged.wait();
            }
        });
    }

    let stdout = lock(&shared).as_mut()?.stdout.take()?;
    // cap + 1: a read that fills the extra byte proves there was more.
    let mut buffer: Vec<u8> = Vec::with_capacity(cap + 1);
    let read = stdout.take((cap + 1) as u64).read_to_end(&mut buffer);
    let truncated = buffer.len() > cap;

    if let Some(mut mine) = lock(&shared).take() {
        if truncated || read.is_err() {
            // git is still writing into a pipe nobody drains; kill unlocks
            // the wait below.
            let _ = mine.kill();
        }
        let _ = mine.wait();
    }
    if read.is_err() {
        return None;
    }
    if truncated {
        // The byte cap can split a character; `from_utf8_lossy` replaces the
        // partial tail with U+FFFD, and the "truncated" note that follows
        // tells the model that is exactly what happened.
        Some(GitOut {
            text: String::from_utf8_lossy(&buffer[..cap]).into_owned(),
            truncated,
        })
    } else {
        Some(GitOut {
            text: String::from_utf8_lossy(&buffer).into_owned(),
            truncated,
        })
    }
}

fn lock<T>(m: &Arc<Mutex<T>>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Runs git in `dir`, failing loudly — fixture setup is not the subject.
    fn git(dir: &std::path::Path, args: &[&str]) {
        let status = std::process::Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(args)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .expect("git is installed for the test suite");
        assert!(status.success(), "git {args:?} failed");
    }

    fn repo(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("warden-git-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("create the repo dir");
        git(&dir, &["init", "-b", "main"]);
        git(&dir, &["config", "user.email", "t@t"]);
        git(&dir, &["config", "user.name", "t"]);
        dir
    }

    #[test]
    fn a_clean_tree_adds_nothing() {
        let dir = repo("clean");
        std::fs::write(dir.join("a.txt"), "one\n").unwrap();
        git(&dir, &["add", "."]);
        git(&dir, &["commit", "-m", "first"]);
        assert!(recent_diff(dir.to_str().unwrap()).is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_dirty_tree_names_files_and_carries_the_patch() {
        let dir = repo("dirty");
        std::fs::write(dir.join("a.txt"), "one\n").unwrap();
        git(&dir, &["add", "."]);
        git(&dir, &["commit", "-m", "first"]);
        std::fs::write(dir.join("a.txt"), "one\ntwo\n").unwrap();
        let block = recent_diff(dir.to_str().unwrap()).expect("dirty tree gives a block");
        assert!(block.contains("a.txt"), "{block}");
        assert!(block.contains("+two"), "{block}");
        assert!(block.contains("git diff --stat"), "{block}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn untracked_or_absent_paths_stay_silent() {
        let dir = repo("untracked");
        std::fs::write(dir.join("a.txt"), "one\n").unwrap();
        // Nothing committed: a.txt is untracked and `git diff` never sees it.
        assert!(recent_diff(dir.to_str().unwrap()).is_none());
        let _ = std::fs::remove_dir_all(&dir);

        // A directory that is not a repository — and whose parents up the
        // tree are not repositories either — answers with an empty stdout.
        let outside = std::env::temp_dir().join(format!("warden-git-plain-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&outside);
        std::fs::create_dir_all(&outside).unwrap();
        if !outside
            .parent()
            .map(|p| p.join(".git").exists())
            .unwrap_or(false)
        {
            assert!(recent_diff(outside.to_str().unwrap()).is_none());
        }
        let _ = std::fs::remove_dir_all(&outside);

        assert!(recent_diff("").is_none());
        assert!(recent_diff("/does/not/exist/at/all").is_none());
    }

    #[test]
    fn a_huge_patch_is_cut_at_the_cap_and_says_so() {
        let dir = repo("huge");
        std::fs::write(dir.join("a.txt"), "base\n").unwrap();
        git(&dir, &["add", "."]);
        git(&dir, &["commit", "-m", "first"]);
        let big: String = (0..5_000).map(|i| format!("line {i}\n")).collect();
        std::fs::write(dir.join("a.txt"), format!("base\n{big}")).unwrap();
        let block = recent_diff(dir.to_str().unwrap()).expect("dirty tree");
        assert!(
            block.contains("truncated by the client"),
            "{}",
            &block[block.len().saturating_sub(200)..]
        );
        // The block stays bounded, never 5000 added lines.
        assert!(
            block.len() < STAT_BYTES + PATCH_BYTES + 600,
            "{}",
            block.len()
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
