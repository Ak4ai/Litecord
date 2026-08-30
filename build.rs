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
    println!("cargo:rerun-if-changed=.git/HEAD");
    println!("cargo:rerun-if-changed=.git/index");

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
