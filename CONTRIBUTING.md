# Contributing to Litecord 🚀

First off, thank you for considering contributing to **Litecord**! Open-source collaboration is what makes projects fast, secure, and innovative.

---

## 🎯 Core Project Vision & Philosophy

Litecord was created with a clear and unwavering mission: **to provide an ultra-lightweight, simple, zero-lag, and low-resource desktop client for gamers to communicate on Discord without sacrificing gaming FPS or system memory**.

### ⚖️ Guidelines on Features & Alternative Releases:
- **Main Branch (`main`)**: Exclusively reserved for performance optimizations, voice/audio improvements, essential messaging, and lightweight enhancements that preserve our sub-0.1% CPU and sub-35 MB RAM footprint.
- **Alternative Branches & Extended Releases**: Any contribution or proposed modification that adds complex, heavy, or non-minimal features (e.g. video rendering, rich browser integrations, heavy canvas animations) **will still be warmly reviewed and considered!** However, rather than bloating the main lightweight core, they will be merged and maintained on **alternative branches (e.g. `extended` or `feature/xyz`)** and published as **alternative release builds**.

---

## 📜 Contribution Policy & Guidelines

Litecord is an open-source project. You are welcome to **clone, fork, inspect, and modify** the codebase. 

### 💡 The Golden Rule:
If you build improvements, bug fixes, performance optimizations, or new features based on this project:
- We ask that you submit your changes back to the main repository via a **Pull Request (PR)**!
- Every accepted contribution will be **officially acknowledged and credited in the [`README.md`](README.md)** and in Release Notes, highlighting your proportional contribution and impact on the project.

---

## 🛠️ How to Submit a Pull Request (PR)

### 1. Fork & Clone
1. Fork the repository to your GitHub account by clicking the **Fork** button at [github.com/Ak4ai/Litecord](https://github.com/Ak4ai/Litecord).
2. Clone your fork locally:
   ```bash
   git clone https://github.com/YOUR_USERNAME/Litecord.git
   cd Litecord
   ```

### 2. Create a Feature Branch
Create a new branch for your specific fix or feature:
```bash
git checkout -b feat/amazing-feature
# or for bug fixes:
git checkout -b fix/audio-underrun-issue
```

### 3. Make Your Changes & Test
Make sure your changes adhere to project standards:
- **Zero Warnings**: Verify with `cargo check --bin litecord` that there are 0 errors and 0 warnings.
- **Code Style**: Run `cargo fmt` to keep Rust formatting clean and consistent.
- **Slint UI**: Ensure UI additions match the clean, native dark aesthetic without adding CPU bloat.

### 4. Commit and Push
Write clear, concise commit messages:
```bash
git add .
git commit -m "feat(audio): add new dynamic noise suppression filter"
git push origin feat/amazing-feature
```

### 5. Open a Pull Request (PR)
1. Navigate to [github.com/Ak4ai/Litecord/pulls](https://github.com/Ak4ai/Litecord/pulls).
2. Click **New Pull Request** and select your feature branch.
3. Provide a clear description of:
   - What problem was solved or what feature was added.
   - How you tested it.
   - Any UI or audio behavioral changes.

---

## 🚀 Releasing New Versions (Maintainers)

Litecord uses automated GitHub Actions CI/CD to build and publish releases. To release a new version cleanly without duplicate assets or changelogs:

1. Update the version in `Cargo.toml`:
   ```toml
   [package]
   version = "0.x.y"
   ```
2. Run `cargo check` to update `Cargo.lock`.
3. Commit and push to `dev`, then merge to `main`.
4. Create and push the version tag:
   ```bash
   git tag v0.x.y
   git push origin v0.x.y
   ```
5. **GitHub Actions will automatically**:
   - Compile the Windows Release Binary (`cargo build --release`)
   - Generate `Litecord-Setup-x64.exe` (Inno Setup)
   - Generate `litecord-windows-x64-portable.zip`
   - Compile and package `litecord-linux-x64.tar.gz`
   - Publish the official release attaching exactly those 3 standardized files.

---

## 👑 Recognition & Credits

Once your Pull Request is reviewed and merged:
- Your name and GitHub profile will be added to the **Contributors Hall of Fame** in the main `README.md`.
- Your specific contributions and their percentage/proportional impact will be highlighted in the release notes.

Thank you for helping make Litecord the lightest and fastest Discord client in existence! ⚡
