//! The repository screen, end to end.
//!
//! The commands are driven in-process rather than over the IPC layer. The
//! functions called here are the very ones the commands call — the command
//! bodies are one-line shells over them — so the path being tested is the
//! one a user's clicks take: add a root, scan, follow a repository, close
//! the app, open it again.
//!
//! The repositories are real directories with real `.git` directories. A
//! fake repository would let a broken scanner pass, which is the one thing
//! this test exists to catch.
use repotory_agent_lib::repos::{
    RepoStore, add_root_and_scan, dismiss_and_scan, remove_root_and_scan, scan_now,
    set_favorite_and_scan,
};
use std::path::{Path, PathBuf};

/// A temporary directory that no other test shares.
fn temp_dir(name: &str) -> PathBuf {
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let dir = std::env::temp_dir().join(format!(
        "repotory-agent-e2e-{name}-{}-{stamp}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("create the temp dir");
    dir
}

/// Makes a directory that looks like a git repository.
fn make_repo(dir: &Path) {
    std::fs::create_dir_all(dir.join(".git")).expect("create the .git dir");
    std::fs::write(dir.join(".git").join("HEAD"), "ref: refs/heads/main\n").expect("write HEAD");
}

/// The store the commands read and write.
fn store(dir: &Path) -> RepoStore {
    RepoStore::new(dir.join("repos.json"))
}

#[test]
fn a_root_is_added_and_the_repositories_below_it_are_found() {
    let root = temp_dir("add-root");
    make_repo(&root.join("alpha"));
    make_repo(&root.join("nested").join("beta"));

    let store = store(&root);

    let scan =
        add_root_and_scan(&store, &root.to_string_lossy()).expect("the root should be accepted");

    assert_eq!(scan.roots.len(), 1);
    assert_eq!(scan.repos.len(), 2, "{:?}", scan.repos);

    let names: Vec<&str> = scan.repos.iter().map(|r| r.name.as_str()).collect();
    assert!(names.contains(&"alpha"), "{names:?}");
    assert!(names.contains(&"beta"), "{names:?}");

    // A repository the user has not answered for is announced.
    assert_eq!(scan.new_repos.len(), 2, "{:?}", scan.new_repos);
}

#[test]
fn following_a_repository_survives_a_restart() {
    let root = temp_dir("restart");
    make_repo(&root.join("alpha"));

    // The first run: add the root and follow the repository.
    {
        let store = store(&root);

        let scan = add_root_and_scan(&store, &root.to_string_lossy())
            .expect("the root should be accepted");
        let path = scan.repos[0].path.clone();

        let scan = set_favorite_and_scan(&store, &path, true).expect("follow the repository");
        assert!(scan.repos[0].favorite);
        assert!(scan.new_repos.is_empty());
    }

    // The second run: a fresh store reads the same file.
    {
        let store = store(&root);

        let scan = scan_now(&store);
        assert_eq!(scan.roots.len(), 1, "{:?}", scan.roots);
        assert_eq!(scan.repos.len(), 1, "{:?}", scan.repos);
        assert!(scan.repos[0].favorite, "the follow survived the restart");
        assert!(scan.new_repos.is_empty());
    }
}

#[test]
fn a_dismissed_repository_is_not_announced_again() {
    let root = temp_dir("dismiss");
    make_repo(&root.join("noise"));
    make_repo(&root.join("other"));

    let store = store(&root);

    let scan =
        add_root_and_scan(&store, &root.to_string_lossy()).expect("the root should be accepted");
    let path = scan.new_repos[0].path.clone();

    let scan = dismiss_and_scan(&store, &path).expect("dismiss the repository");
    assert_eq!(scan.new_repos.len(), 1, "the other one is still unhandled");

    // The dismissed one is not announced again, but it is still in the list.
    let scan = scan_now(&store);
    assert_eq!(scan.new_repos.len(), 1, "{:?}", scan.new_repos);
    assert!(!scan.new_repos[0].path.contains("noise"));
    assert_eq!(scan.repos.len(), 2);
}

#[test]
fn a_root_that_is_not_a_directory_is_refused() {
    let root = temp_dir("bad-root");
    let file = root.join("a-file.txt");
    std::fs::write(&file, "not a directory").expect("write the file");

    let store = store(&root);

    let outcome = add_root_and_scan(&store, &file.to_string_lossy());
    assert!(outcome.is_err(), "a file is not a scan root");

    let outcome = add_root_and_scan(&store, "/definitely/not/here");
    assert!(outcome.is_err(), "a missing directory is not a scan root");
}

#[test]
fn removing_a_root_removes_its_repositories_from_the_list() {
    let root = temp_dir("remove-root");
    make_repo(&root.join("alpha"));

    let store = store(&root);

    let scan =
        add_root_and_scan(&store, &root.to_string_lossy()).expect("the root should be accepted");
    assert_eq!(scan.repos.len(), 1);

    let scan =
        remove_root_and_scan(&store, &root.to_string_lossy()).expect("the root should be removed");
    assert!(scan.roots.is_empty(), "{:?}", scan.roots);
    assert!(scan.repos.is_empty(), "{:?}", scan.repos);

    // Removing it twice is an error, not a silent success.
    let outcome = remove_root_and_scan(&store, &root.to_string_lossy());
    assert!(outcome.is_err());
}

#[test]
fn unfollowing_a_repository_brings_it_back_to_the_announced_list() {
    let root = temp_dir("unfollow");
    make_repo(&root.join("alpha"));

    let store = store(&root);

    let scan =
        add_root_and_scan(&store, &root.to_string_lossy()).expect("the root should be accepted");
    let path = scan.repos[0].path.clone();

    let scan = set_favorite_and_scan(&store, &path, true).expect("follow");
    assert!(scan.new_repos.is_empty());

    let scan = set_favorite_and_scan(&store, &path, false).expect("unfollow");
    assert_eq!(scan.new_repos.len(), 1, "{:?}", scan.new_repos);
}
