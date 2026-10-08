//! Opens a real pull request with `gh`, run on demand:
//!
//! ```sh
//! SCRIPTR_LIVE_REPO=/path/to/a/throwaway/clone \
//!   cargo test --test publish_live -- --ignored --nocapture
//! ```
//!
//! Ignored by default: it needs a GitHub repository and an authenticated gh.

use std::path::PathBuf;

#[test]
#[ignore = "needs SCRIPTR_LIVE_REPO and an authenticated gh"]
fn opens_a_real_pull_request() {
    let Ok(repo) = std::env::var("SCRIPTR_LIVE_REPO") else {
        panic!("set SCRIPTR_LIVE_REPO to a throwaway clone");
    };
    let repo = PathBuf::from(repo);
    let id = format!("live-{}", std::process::id());

    let wt = scriptr_lib::worktree::ensure(&repo, &id, "Prove the publish path works", Some("main")).unwrap();
    println!("worktree {} on {}", wt.path.display(), wt.branch);
    std::fs::write(wt.path.join("evidence.txt"), "written by the publish test\n").unwrap();

    let done = scriptr_lib::publish::publish(
        &wt.path,
        &wt.branch,
        "main",
        "Prove the publish path works",
        "Write a file and open a PR for it, to prove B2 end to end.",
        false,
    )
    .expect("publish");

    println!("committed {:?} pushed {} pr {:?} note {:?}", done.committed, done.pushed, done.pr, done.pr_note);
    assert!(done.committed.is_some(), "Scriptr should have committed the file");
    assert!(done.pushed);
    let pr = done.pr.expect("a pull request");
    assert!(pr.url.contains("/pull/"), "{}", pr.url);
    assert!(pr.number > 0);

    // Calling again must reuse the PR, not open a second one.
    std::fs::write(wt.path.join("evidence2.txt"), "second round\n").unwrap();
    let again = scriptr_lib::publish::publish(
        &wt.path,
        &wt.branch,
        "main",
        "Prove the publish path works",
        "goal",
        false,
    )
    .expect("second publish");
    assert_eq!(again.pr.as_ref().map(|p| p.number), Some(pr.number), "a second PR was opened");
    println!("reused PR #{}", pr.number);

    scriptr_lib::worktree::remove(&repo, &id, true).unwrap();
}
