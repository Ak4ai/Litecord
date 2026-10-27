fn main() {
    slint_build::compile("ui/appwindow.slint").expect("Slint build failed");

    // Extract Git commit hash and dirty status for accurate version tracking
    let git_hash = std::process::Command::new("git")
        .args(["rev-parse", "--short=7", "HEAD"])
        .output()
        .ok()
        .and_then(|out| String::from_utf8(out.stdout).ok())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "release".to_string());

    let git_dirty = std::process::Command::new("git")
        .args(["status", "--porcelain"])
        .output()
        .ok()
        .and_then(|out| String::from_utf8(out.stdout).ok())
        .map(|s| !s.trim().is_empty())
        .unwrap_or(false);

    let git_branch = std::process::Command::new("git")
        .args(["rev-parse", "--abbrev-ref", "HEAD"])
        .output()
        .ok()
        .and_then(|out| String::from_utf8(out.stdout).ok())
        .map(|s| s.trim().to_string())
        .unwrap_or_default();

    println!("cargo:rustc-env=LITECORD_GIT_HASH={}", git_hash);
    println!("cargo:rustc-env=LITECORD_GIT_DIRTY={}", if git_dirty { "true" } else { "false" });
    println!("cargo:rustc-env=LITECORD_GIT_BRANCH={}", git_branch);
    // Rerun only when the checked-out commit changes: HEAD (branch switch) and the
    // branch ref (new commit). Watching .git/index re-ran this script, and so
    // recompiled the crate, on every `git add`. The dirty flag is therefore taken
    // at the last rebuild, which is exact for builds made from a clean tree.
    // The git dir is resolved through git so this also holds in worktrees, where
    // `.git` is a file and watching a path under it would rerun on every build.
    for path in git_paths_to_watch() {
        println!("cargo:rerun-if-changed={}", path);
    }

    #[cfg(target_os = "windows")]
    {
        let mut res = winres::WindowsResource::new();
        res.set_icon("assets/app_icon.ico");
        res.set_language(0x0409); // English (US) / Neutral
        res.set("ProductName", "Litecord");
        res.set("FileDescription", "Litecord - Ultra-Lightweight Discord Client");
        res.set("LegalCopyright", "Litecord Project");
        if let Err(e) = res.compile() {
            println!("cargo:warning=winres icon compilation failed: {:?}", e);
        }
    }
}

fn git_output(args: &[&str]) -> Option<String> {
    let out = std::process::Command::new("git").args(args).output().ok()?;
    if !out.status.success() {
        return None;
    }
    let text = String::from_utf8(out.stdout).ok()?.trim().to_string();
    if text.is_empty() { None } else { Some(text) }
}

fn git_paths_to_watch() -> Vec<String> {
    let mut paths = Vec::new();
    if let Some(head) = git_output(&["rev-parse", "--path-format=absolute", "--git-path", "HEAD"]) {
        paths.push(head);
    }
    if let Some(branch_ref) = git_output(&["symbolic-ref", "-q", "HEAD"]) {
        if let Some(ref_file) = git_output(&["rev-parse", "--path-format=absolute", "--git-path", &branch_ref]) {
            paths.push(ref_file);
        }
        // Refs move into packed-refs after `git gc`.
        if let Some(packed) = git_output(&["rev-parse", "--path-format=absolute", "--git-path", "packed-refs"]) {
            paths.push(packed);
        }
    }
    paths
}
