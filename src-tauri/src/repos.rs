// The repositories the copilot watches.
//
// This module is what makes the copilot a copilot of the *machine* rather
// than a front end for Cinder's database. The copilot finds the git
// repositories on this computer by itself, shows them as a list, and lets
// the user say which ones matter. Cinder's own scanner exists, but it
// exists to fill Cinder's knowledge base from the desktop app's scan
// roots; borrowing it would make the copilot depend on that database, and
// this product is meant to stand on its own.
//
// What is stored where:
//
//   * the watchlist (scan roots, favourites, dismissed repositories) goes to
//     a JSON file in the app config directory. It is configuration, and it
//     is meant to be readable.
//
// There is no secret here, so there is nothing in the credential store.
//
// Two rules the rest of the crate depends on:
//
//   1. A repository is a directory that contains a `.git` **directory**. A
//      worktree or submodule has a `.git` *file*, and is not counted. This
//      is the same rule Cinder's scanner uses, deliberately: the two
//      lists must not disagree about what a repository is.
//
//   2. A repository is "new" only while the user has not dealt with it.
//      Favouriting it or dismissing it puts it in the handled set, and a
//      handled repository never surfaces again. Without that rule a user who
//      says "not this one" would be asked again on every scan — the fastest
//      way to make a robot annoying.
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

/// How deep below a scan root the search goes.
///
/// The root itself is always examined, and so are the directories below it,
/// down to this many levels. Two levels is enough to cover the common
/// layouts — `~/work/company/repo` and `~/work/repo` — without walking a
/// home directory that holds a hundred thousand files.
pub const MAX_DEPTH: usize = 2;

/// The most repositories one scan reports.
///
/// A scan is bounded on purpose: the roots are the user's, but a root can
/// be a home directory or a mounted volume, and an unbounded walk is how a
/// background task turns into a complaint. A truncated scan says so rather
/// than silently reporting a partial list as if it were the whole truth.
pub const MAX_REPOS: usize = 500;

/// One repository the copilot knows about.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RepoEntry {
    /// The absolute path. This is the repository's identity: the same
    /// repository reached through two different roots is one entry.
    pub path: String,
    /// The directory's name — what the list shows as the title.
    pub name: String,
    /// Whether the user follows this repository.
    pub favorite: bool,
    /// The repository is followed but was not found in this scan.
    ///
    /// This happens when a scan root is removed or changed, or when a disk
    /// is not mounted. A followed repository that silently disappeared from
    /// the list would look like the app lost the user's settings.
    #[serde(default)]
    pub missing: bool,
}

/// The user's watchlist: where to look, and what the user decided.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RepoWatchlist {
    /// Directories the scan starts from.
    #[serde(default)]
    pub roots: Vec<String>,
    /// Repositories the user follows.
    #[serde(default)]
    pub favorites: Vec<String>,
    /// Repositories the user said "not this one" to.
    #[serde(default)]
    pub dismissed: Vec<String>,
}

impl RepoWatchlist {
    /// Whether the user follows this repository.
    pub fn is_favorite(&self, path: &str) -> bool {
        self.favorites.iter().any(|p| p == path)
    }

    /// Whether the user has already dealt with this repository.
    ///
    /// "Dealt with" means followed or dismissed. Both stop the repository
    /// from being announced again.
    pub fn is_handled(&self, path: &str) -> bool {
        self.is_favorite(path) || self.dismissed.iter().any(|p| p == path)
    }

    /// Adds a scan root. Returns false when it was already there.
    pub fn add_root(&mut self, path: &str) -> bool {
        if self.roots.iter().any(|p| p == path) {
            return false;
        }
        self.roots.push(path.to_string());
        true
    }

    /// Removes a scan root. Returns false when it was not there.
    pub fn remove_root(&mut self, path: &str) -> bool {
        let before = self.roots.len();
        self.roots.retain(|p| p != path);
        self.roots.len() != before
    }

    /// Follows or unfollows a repository.
    ///
    /// Following a repository that was dismissed before takes it out of the
    /// dismissed set: the user changed their mind, and the two sets are
    /// meant to say opposite things about the same repository.
    pub fn set_favorite(&mut self, path: &str, favorite: bool) {
        if favorite {
            if !self.favorites.iter().any(|p| p == path) {
                self.favorites.push(path.to_string());
            }
            self.dismissed.retain(|p| p != path);
        } else {
            self.favorites.retain(|p| p != path);
        }
    }

    /// Marks a repository as "not this one", so it stops being announced.
    pub fn dismiss(&mut self, path: &str) {
        if !self.dismissed.iter().any(|p| p == path) {
            self.dismissed.push(path.to_string());
        }
        self.favorites.retain(|p| p != path);
    }
}

/// One thing the user can adopt as a project.
///
/// A project is not necessarily a repository. A monorepo-like folder holding
/// several checkouts is a better unit of work than any single checkout in it,
/// so the scan offers both the repository and its direct parent as separate,
/// equally selectable candidates. `kind` tells the UI which is which; it is
/// presentation, not a second storage concept — both are stored as a path in
/// `favorites`.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectCandidate {
    /// The absolute path, and the identity of the selection.
    pub path: String,
    /// The directory's name, shown as the row title.
    pub name: String,
    /// `"repo"` for a single checkout, `"parent"` for a folder of checkouts.
    pub kind: String,
    /// How many found repositories live under this path. Zero means the
    /// directory has drifted out of the scan but is still selected.
    pub children: usize,
    /// Whether the user has selected this project.
    pub favorite: bool,
}

/// Everything the repository screen needs, in one payload.
///
/// One command returns one of these so the screen cannot show a list that
/// disagrees with the roots it is showing: the roots, the repositories, the
/// newly found ones, the selectable projects and the problems all come from
/// the same scan.
#[derive(Clone, Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RepoScan {
    /// The scan roots.
    pub roots: Vec<String>,
    /// Every repository the copilot knows about.
    pub repos: Vec<RepoEntry>,
    /// The repositories the user has not dealt with yet.
    pub new_repos: Vec<RepoEntry>,
    /// Repositories and their direct parents, offered for selection together.
    /// This is the list the settings screen renders; nothing is announced.
    pub projects: Vec<ProjectCandidate>,
    /// The scan stopped early because it hit the repository limit.
    pub truncated: bool,
    /// Why the scan could not be read or run.
    pub problem: Option<String>,
}

/// The watchlist on disk.
#[derive(Clone)]
pub struct RepoStore {
    path: PathBuf,
}

impl RepoStore {
    /// Opens the store at the given config file path.
    pub fn new(path: PathBuf) -> Self {
        Self { path }
    }

    /// Reads the watchlist.
    ///
    /// A missing file is an empty watchlist — the first run. A corrupt file
    /// is also an empty watchlist, but the reason is kept: discarding a
    /// user's watchlist silently would be worse than saying the file could
    /// not be read.
    pub fn load(&self) -> (RepoWatchlist, Option<String>) {
        let raw = match std::fs::read_to_string(&self.path) {
            Ok(raw) => raw,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                return (RepoWatchlist::default(), None);
            }
            Err(err) => {
                return (
                    RepoWatchlist::default(),
                    Some(format!("the watchlist could not be read: {err}")),
                );
            }
        };
        match serde_json::from_str(&raw) {
            Ok(list) => (list, None),
            Err(err) => (
                RepoWatchlist::default(),
                Some(format!("the watchlist is not valid JSON: {err}")),
            ),
        }
    }

    /// Writes the watchlist.
    pub fn save(&self, list: &RepoWatchlist) -> Result<(), String> {
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|err| format!("the config directory could not be created: {err}"))?;
        }
        let body = serde_json::to_string_pretty(list)
            .map_err(|err| format!("the watchlist could not be serialized: {err}"))?;
        std::fs::write(&self.path, body)
            .map_err(|err| format!("the watchlist could not be written: {err}"))?;
        restrict_permissions(&self.path);
        Ok(())
    }
}

/// Makes a file owner-only where the platform supports it.
fn restrict_permissions(path: &Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
    }
    #[cfg(not(unix))]
    let _ = path;
}

/// Scans and reports what is there.
pub fn scan_now(store: &RepoStore) -> RepoScan {
    let (list, problem) = store.load();
    scan_into(&list.roots, &list, problem)
}

/// Adds a scan root and reports the new scan.
///
/// The path is stored absolute, so the same directory typed in three
/// different ways is one root and not three. A path that is not a
/// directory is refused: a root that does not exist is a typo, and a root
/// that is a file is a misunderstanding.
pub fn add_root_and_scan(store: &RepoStore, path: &str) -> Result<RepoScan, String> {
    let trimmed = path.trim();
    if trimmed.is_empty() {
        return Err("the scan root needs a path".to_string());
    }
    let absolute = std::path::absolute(trimmed)
        .map_err(|err| format!("the path could not be resolved: {err}"))?;
    if !absolute.is_dir() {
        return Err(format!("{} is not a directory", absolute.display()));
    }

    let (mut list, _) = store.load();
    list.add_root(&absolute.to_string_lossy());
    store.save(&list)?;
    Ok(scan_into(&list.roots, &list, None))
}

/// Removes a scan root and reports the new scan.
pub fn remove_root_and_scan(store: &RepoStore, path: &str) -> Result<RepoScan, String> {
    let trimmed = path.trim();
    let absolute = std::path::absolute(trimmed).unwrap_or_else(|_| trimmed.into());
    let target = absolute.to_string_lossy().into_owned();

    let (mut list, _) = store.load();
    if !list.remove_root(&target) {
        return Err(format!("{target} is not a scan root"));
    }
    store.save(&list)?;
    Ok(scan_into(&list.roots, &list, None))
}

/// Follows or unfollows a repository and reports the new scan.
pub fn set_favorite_and_scan(
    store: &RepoStore,
    path: &str,
    favorite: bool,
) -> Result<RepoScan, String> {
    let (mut list, _) = store.load();
    list.set_favorite(path.trim(), favorite);
    store.save(&list)?;
    Ok(scan_into(&list.roots, &list, None))
}

/// Marks a repository as "not this one" and reports the new scan.
pub fn dismiss_and_scan(store: &RepoStore, path: &str) -> Result<RepoScan, String> {
    let (mut list, _) = store.load();
    list.dismiss(path.trim());
    store.save(&list)?;
    Ok(scan_into(&list.roots, &list, None))
}

/// Applies the user's project selection in one write.
///
/// The settings screen shows the scan result as a checkbox list; saving is a
/// single action over the WHOLE selection, not one prompt per repository.
/// A candidate may be a single checkout or its parent folder — both are just
/// a path here, and the caller already got them from a scan.
pub fn set_projects_and_scan(store: &RepoStore, paths: &[String]) -> Result<RepoScan, String> {
    let (mut list, _) = store.load();
    list.favorites = paths
        .iter()
        .map(|path| path.trim().to_string())
        .filter(|path| !path.is_empty())
        .collect();
    // The user has answered for every candidate in this scan. A leftover
    // dismissal from the old one-at-a-time flow would otherwise hide a
    // repository they just selected.
    list.dismissed.clear();
    store.save(&list)?;
    Ok(scan_into(&list.roots, &list, None))
}

/// Walks the roots and reports the repositories it finds.
///
/// Returns the repositories and whether the scan was cut short by the
/// repository limit.
pub fn scan_roots(roots: &[String]) -> (Vec<RepoEntry>, bool) {
    let mut found: BTreeSet<PathBuf> = BTreeSet::new();
    let mut truncated = false;

    for root in roots {
        let root = root.trim();
        if root.is_empty() {
            continue;
        }
        let path = match absolute_path(root) {
            Ok(path) => path,
            Err(_) => continue,
        };
        if !path.is_dir() {
            continue;
        }
        collect(&path, MAX_DEPTH, &mut found, &mut truncated);
    }

    let repos = found
        .into_iter()
        .map(|path| RepoEntry {
            name: dir_name(&path),
            path: path.to_string_lossy().into_owned(),
            favorite: false,
            missing: false,
        })
        .collect();

    (repos, truncated)
}

/// Builds the payload the repository screen renders.
///
/// The list is the scan plus the repositories the user follows that the
/// scan did not find — a followed repository that vanished from the list
/// would read as the app losing the user's settings, when in fact the
/// drive is not mounted.
pub fn scan_into(roots: &[String], list: &RepoWatchlist, problem: Option<String>) -> RepoScan {
    let (found, truncated) = scan_roots(roots);

    let mut repos: Vec<RepoEntry> = found
        .into_iter()
        .map(|mut entry| {
            entry.favorite = list.is_favorite(&entry.path);
            entry
        })
        .collect();

    // Repositories the user follows but this scan did not see.
    let missing: Vec<RepoEntry> = list
        .favorites
        .iter()
        .filter(|path| !repos.iter().any(|entry| &entry.path == *path))
        .map(|path| RepoEntry {
            name: dir_name(Path::new(path)),
            path: path.clone(),
            favorite: true,
            missing: true,
        })
        .collect();
    repos.extend(missing);
    repos.sort_by(|a, b| a.path.cmp(&b.path));

    let new_repos = repos
        .iter()
        .filter(|entry| !entry.missing && !list.is_handled(&entry.path))
        .cloned()
        .collect();

    let projects = project_candidates(&repos, list);

    RepoScan {
        roots: list.roots.clone(),
        repos,
        new_repos,
        projects,
        truncated,
        problem,
    }
}

/// Builds the selectable project list: every found repository, plus every
/// directory that directly contains one.
///
/// Both appear in ONE list, sorted by path, so the choice is made in the
/// scan result itself rather than in a sequence of pop-ups. A parent is
/// offered even when it holds a single repository — the user may still want
/// the folder as the unit of work. Scan roots themselves are not offered as
/// projects: a root is where to LOOK, not a thing to work on.
fn project_candidates(repos: &[RepoEntry], list: &RepoWatchlist) -> Vec<ProjectCandidate> {
    use std::collections::BTreeMap;

    let mut parents: BTreeMap<String, usize> = BTreeMap::new();
    let mut found: Vec<ProjectCandidate> = Vec::new();
    for repo in repos.iter().filter(|repo| !repo.missing) {
        found.push(ProjectCandidate {
            favorite: list.is_favorite(&repo.path),
            path: repo.path.clone(),
            name: repo.name.clone(),
            kind: "repo".to_string(),
            children: 1,
        });
        if let Some(parent) = Path::new(&repo.path).parent()
            && !list
                .roots
                .iter()
                .any(|root| root == &parent.to_string_lossy())
        {
            *parents
                .entry(parent.to_string_lossy().into_owned())
                .or_insert(0) += 1;
        }
    }
    for (path, children) in parents {
        found.push(ProjectCandidate {
            favorite: list.is_favorite(&path),
            name: dir_name(Path::new(&path)),
            path,
            kind: "parent".to_string(),
            children,
        });
    }

    // A selection whose directory has disappeared must stay visible and
    // removable; silently dropping it would look like lost configuration.
    for path in &list.favorites {
        if !found.iter().any(|candidate| &candidate.path == path) {
            found.push(ProjectCandidate {
                favorite: true,
                name: dir_name(Path::new(path)),
                path: path.clone(),
                kind: "missing".to_string(),
                children: 0,
            });
        }
    }

    found.sort_by(|a, b| a.path.cmp(&b.path));
    found
}

/// Collects the repositories under one directory.
fn collect(dir: &Path, depth: usize, out: &mut BTreeSet<PathBuf>, truncated: &mut bool) {
    if out.len() >= MAX_REPOS {
        *truncated = true;
        return;
    }

    if is_repo(dir) {
        out.insert(dir.to_path_buf());
        return;
    }

    if depth == 0 {
        return;
    }

    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };

    for entry in entries.flatten() {
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        // Symlinks are not followed: a link into another tree can point
        // back at the one we are walking, and the user's other roots cover
        // the places they meant to include.
        if !file_type.is_dir() || file_type.is_symlink() {
            continue;
        }
        collect(&entry.path(), depth - 1, out, truncated);
        if out.len() >= MAX_REPOS {
            *truncated = true;
            return;
        }
    }
}

/// Whether a directory is a git repository.
///
/// The `.git` **directory** is the test. A worktree or submodule has a
/// `.git` file instead, and is not counted — the same rule Cinder's
/// scanner uses.
fn is_repo(dir: &Path) -> bool {
    std::fs::metadata(dir.join(".git"))
        .map(|meta| meta.is_dir())
        .unwrap_or(false)
}

/// Turns a path into an absolute one.
///
/// `std::path::absolute` and not `canonicalize`: canonicalize resolves
/// symlinks, so on macOS a repository under `/tmp` would be stored as
/// `/private/tmp/...` — a path the user did not type and cannot recognise
/// in the list.
fn absolute_path(input: &str) -> Result<PathBuf, String> {
    std::path::absolute(input).map_err(|err| format!("the path could not be resolved: {err}"))
}

/// The directory's own name, falling back to the whole path.
fn dir_name(path: &Path) -> String {
    path.file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.to_string_lossy().into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A unique temporary directory for one test.
    ///
    /// The name carries the process id and a nanosecond stamp, so parallel
    /// tests and two runs of the same test do not share a directory.
    fn temp_dir(name: &str) -> PathBuf {
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let dir = std::env::temp_dir().join(format!(
            "repotory-agent-repos-{name}-{}-{stamp}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("create the temp dir");
        dir
    }

    /// Makes a directory that looks like a git repository.
    fn make_repo(dir: &Path) {
        std::fs::create_dir_all(dir.join(".git")).expect("create the .git dir");
        std::fs::write(dir.join(".git").join("HEAD"), "ref: refs/heads/main\n")
            .expect("write HEAD");
    }

    #[test]
    fn a_git_directory_is_a_repository() {
        let root = temp_dir("is-repo");
        let repo = root.join("alpha");
        make_repo(&repo);

        let (found, truncated) = scan_roots(&[root.to_string_lossy().into_owned()]);
        assert!(!truncated);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].name, "alpha");
        assert_eq!(found[0].path, repo.to_string_lossy());
    }

    /// A worktree's `.git` is a file, not a directory. Counting those would
    /// make this list disagree with Cinder's.
    #[test]
    fn a_git_file_is_not_a_repository() {
        let root = temp_dir("git-file");
        let dir = root.join("linked");
        std::fs::create_dir_all(&dir).expect("create the dir");
        std::fs::write(dir.join(".git"), "gitdir: /elsewhere/.git/worktrees/x")
            .expect("write the .git file");

        let (found, _) = scan_roots(&[root.to_string_lossy().into_owned()]);
        assert!(found.is_empty(), "a .git file must not count: {found:?}");
    }

    /// The root itself may be a repository — that is the layout where the
    /// user points at one checkout.
    #[test]
    fn the_root_itself_can_be_the_repository() {
        let root = temp_dir("root-repo");
        make_repo(&root);

        let (found, _) = scan_roots(&[root.to_string_lossy().into_owned()]);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].path, root.to_string_lossy());
    }

    /// Repositories below the root are found, but only as deep as the depth
    /// allows — a walk that never ends is not a feature.
    ///
    /// The chain that must NOT be found is a chain of plain directories
    /// with a repository at the far end: a repository stops the walk on its
    /// own (the next test), so only a directory chain can reach the depth
    /// limit.
    #[test]
    fn the_search_stops_at_the_depth_limit() {
        let root = temp_dir("depth");
        make_repo(&root.join("alpha"));
        std::fs::create_dir_all(root.join("mid").join("deep")).expect("create the chain");
        make_repo(&root.join("mid").join("deep").join("gamma"));

        let (found, _) = scan_roots(&[root.to_string_lossy().into_owned()]);
        let names: Vec<&str> = found.iter().map(|r| r.name.as_str()).collect();
        assert!(names.contains(&"alpha"), "{names:?}");
        assert!(
            !names.contains(&"gamma"),
            "the third level is past the depth limit: {names:?}"
        );
    }

    /// A repository stops the walk: a checkout inside a checkout is the
    /// repository's own business, and descending into it would report
    /// nested worktrees the user never asked about.
    #[test]
    fn a_repository_is_not_searched_inside() {
        let root = temp_dir("nested");
        make_repo(&root.join("outer"));
        make_repo(&root.join("outer").join("inner"));

        let (found, _) = scan_roots(&[root.to_string_lossy().into_owned()]);
        let names: Vec<&str> = found.iter().map(|r| r.name.as_str()).collect();
        assert_eq!(names, vec!["outer"], "{names:?}");
    }

    /// Two roots that reach the same repository report it once.
    #[test]
    fn the_same_repository_is_reported_once() {
        let root = temp_dir("overlap");
        let repo = root.join("shared");
        make_repo(&repo);

        let (found, _) = scan_roots(&[
            root.to_string_lossy().into_owned(),
            repo.to_string_lossy().into_owned(),
        ]);
        assert_eq!(found.len(), 1, "{found:?}");
    }

    /// A repository the user followed stays in the list even when the scan
    /// does not see it — a disk that is not mounted must not look like the
    /// app forgot the user.
    #[test]
    fn a_followed_repository_that_is_missing_stays_in_the_list() {
        let root = temp_dir("missing");
        let list = RepoWatchlist {
            roots: vec![root.to_string_lossy().into_owned()],
            favorites: vec!["/Volumes/spare/elsewhere".to_string()],
            dismissed: vec![],
        };

        let scan = scan_into(&list.roots, &list, None);
        assert_eq!(scan.repos.len(), 1);
        assert!(scan.repos[0].missing);
        assert!(scan.repos[0].favorite);
    }

    /// The core promise of the dismissed set: a repository the user said
    /// "not this one" to is never announced again.
    #[test]
    fn a_dismissed_repository_is_not_new_again() {
        let root = temp_dir("dismissed");
        make_repo(&root.join("noise"));

        let mut list = RepoWatchlist {
            roots: vec![root.to_string_lossy().into_owned()],
            ..Default::default()
        };

        let first = scan_into(&list.roots, &list, None);
        assert_eq!(first.new_repos.len(), 1, "{:?}", first.new_repos);

        list.dismiss(&first.new_repos[0].path);
        let second = scan_into(&list.roots, &list, None);
        assert!(
            second.new_repos.is_empty(),
            "a dismissed repository must not come back: {:?}",
            second.new_repos
        );
    }

    /// Following a repository also takes it out of the announced set.
    #[test]
    fn a_followed_repository_is_not_new() {
        let root = temp_dir("followed");
        make_repo(&root.join("wanted"));

        let mut list = RepoWatchlist {
            roots: vec![root.to_string_lossy().into_owned()],
            ..Default::default()
        };
        let first = scan_into(&list.roots, &list, None);
        list.set_favorite(&first.new_repos[0].path, true);

        let second = scan_into(&list.roots, &list, None);
        assert!(second.new_repos.is_empty());
        assert!(second.repos[0].favorite);
    }

    /// Following a repository the user had dismissed takes it out of the
    /// dismissed set — the two sets say opposite things about one
    /// repository, and the user just changed their mind.
    #[test]
    fn following_a_dismissed_repository_clears_the_dismissal() {
        let mut list = RepoWatchlist::default();
        list.dismiss("/work/alpha");
        assert!(list.is_handled("/work/alpha"));

        list.set_favorite("/work/alpha", true);
        assert!(list.is_favorite("/work/alpha"));
        assert!(!list.dismissed.iter().any(|p| p == "/work/alpha"));
    }

    /// Unfollowing leaves the repository announced again — the user wants
    /// to hear about it after all.
    #[test]
    fn unfollowing_brings_a_repository_back() {
        let mut list = RepoWatchlist::default();
        list.set_favorite("/work/alpha", true);
        list.set_favorite("/work/alpha", false);

        assert!(!list.is_favorite("/work/alpha"));
        assert!(!list.is_handled("/work/alpha"));
    }

    #[test]
    fn roots_are_added_and_removed_once_each() {
        let mut list = RepoWatchlist::default();
        assert!(list.add_root("/work"));
        assert!(!list.add_root("/work"));
        assert!(list.remove_root("/work"));
        assert!(!list.remove_root("/work"));
    }

    /// The watchlist survives a round trip through the file.
    #[test]
    fn the_watchlist_round_trips_through_the_file() {
        let dir = temp_dir("store");
        let store = RepoStore::new(dir.join("repos.json"));

        let (empty, problem) = store.load();
        assert!(problem.is_none());
        assert!(empty.roots.is_empty());

        let mut list = RepoWatchlist::default();
        list.add_root("/work");
        list.set_favorite("/work/alpha", true);
        list.dismiss("/work/beta");
        store.save(&list).expect("save the watchlist");

        let (loaded, problem) = store.load();
        assert!(problem.is_none());
        assert_eq!(loaded.roots, vec!["/work".to_string()]);
        assert!(loaded.is_favorite("/work/alpha"));
        assert!(loaded.is_handled("/work/beta"));
    }

    /// A corrupt file is reported rather than silently discarded.
    #[test]
    fn a_corrupt_watchlist_is_reported() {
        let dir = temp_dir("corrupt");
        let path = dir.join("repos.json");
        std::fs::write(&path, "{not json").expect("write the broken file");

        let (list, problem) = RepoStore::new(path).load();
        assert!(list.roots.is_empty());
        assert!(problem.is_some(), "a corrupt file must be reported");
    }

    /// A root that does not exist is skipped, not an error: a volume that
    /// is not mounted is a normal state, not a failure.
    #[test]
    fn a_root_that_does_not_exist_is_skipped() {
        let (found, truncated) = scan_roots(&["/definitely/not/here".to_string()]);
        assert!(found.is_empty());
        assert!(!truncated);
    }
}
