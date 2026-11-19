//! Sets `RUSTHOUR_GIT_COMMIT` to the checkout's commit for the build label of
//! local builds (CI sets `GITHUB_SHA`). Without git, it sets nothing.

use std::path::Path;
use std::process::Command;

fn git(args: &[&str]) -> Option<String> {
    let out = Command::new("git").args(args).output().ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).trim().to_owned())
}

fn main() {
    if let Some(sha) = git(&["rev-parse", "--short=7", "HEAD"]) {
        println!("cargo::rustc-env=RUSTHOUR_GIT_COMMIT={sha}");
    }
    // Rebuild on a new commit or checkout. Only files that exist: a missing
    // one would rerun this, and rebuild the viewer, on every build.
    // shortcut: a branch whose ref is only in packed-refs isn't watched; its
    // label updates on the next change in this crate.
    let branch = git(&["symbolic-ref", "-q", "HEAD"]);
    for name in ["HEAD"].into_iter().chain(branch.as_deref()) {
        if let Some(path) = git(&["rev-parse", "--git-path", name])
            && Path::new(&path).exists()
        {
            println!("cargo::rerun-if-changed={path}");
        }
    }
}
