//! Scans a directory and prints the repositories it finds.
//!
//! `cargo run --example repo_probe -- /Users/you/work`
//!
//! This is the scanner from the running app, on the real disk, with no
//! window and no mock: the answer it prints is the answer the app gets.
fn main() {
    let Some(root) = std::env::args().nth(1) else {
        eprintln!("usage: repo_probe <directory>");
        std::process::exit(2);
    };

    let (repos, truncated) = repotory_agent_lib::repos::scan_roots(std::slice::from_ref(&root));
    for repo in &repos {
        println!("{}", repo.path);
    }
    println!(
        "-- {} repositories under {}{}",
        repos.len(),
        root,
        if truncated { " (truncated)" } else { "" }
    );
}
