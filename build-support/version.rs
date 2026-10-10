// Build script shared by tui and bridge (each crate's build.rs includes it).
//
// Sets IMSG_VERSION to "<version> (<commit>)", e.g. "0.1.0 (455dfb1)", with
// "-dirty" when tracked files had uncommitted changes, so a local build is
// never mistaken for a release. Where there's no git checkout (the copy
// rsynced to the Mac), the commit comes from IMSG_COMMIT, or is left out.

use std::process::Command;

fn main() {
    let version = std::env::var("CARGO_PKG_VERSION").unwrap();
    let commit = std::env::var("IMSG_COMMIT")
        .ok()
        .filter(|c| !c.is_empty())
        .or_else(git_commit);
    let full = match commit {
        Some(c) => format!("{version} ({c})"),
        None => version,
    };
    println!("cargo:rustc-env=IMSG_VERSION={full}");
    println!("cargo:rerun-if-env-changed=IMSG_COMMIT");
    for path in [
        "src",
        "../proto/src",
        "../.git/HEAD",
        "../.git/index",
        "../.git/refs",
    ] {
        println!("cargo:rerun-if-changed={path}");
    }
}

fn git(args: &[&str]) -> Option<String> {
    let out = Command::new("git").args(args).output().ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).trim().to_string())
}

fn git_commit() -> Option<String> {
    let hash = git(&["rev-parse", "--short=7", "HEAD"])?;
    let dirty = git(&["status", "--porcelain", "--untracked-files=no"])
        .is_some_and(|s| !s.is_empty());
    Some(if dirty { format!("{hash}-dirty") } else { hash })
}
