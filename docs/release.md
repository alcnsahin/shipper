# Shipper — Release & Installation Guide

## Release Workflow Overview

```
git tag v1.2.3
git push origin v1.2.3
        │
        ▼
GitHub Actions (.github/workflows/release.yml)
        │
        ├─ Build: macos-arm64
        ├─ Build: macos-x86_64 (cross-compiled)
        ├─ Build: linux-x86_64 (musl, static)
        └─ Build: windows-x86_64
        │
        ▼
GitHub Release (draft: false)
  shipper-macos-arm64
  shipper-macos-x86_64
  shipper-linux-x86_64
  shipper-windows-x86_64.exe
        │
        ▼
homebrew-tap/Formula/shipper.rb (auto-updated with SHA256)
```

---

## Tag Management

### Create a release tag

```bash
git tag v1.2.3
git push origin v1.2.3
```

Pushing a tag triggers the GitHub Actions release workflow. The tag name must start with `v` (`v*` pattern).

### Pre-release tag

If the tag name contains `-`, the GitHub Release is automatically marked as **pre-release**:

```bash
git tag v1.2.3-beta
git tag v1.2.3-rc.1
git push origin v1.2.3-beta
```

### List tags

```bash
git tag                          # local tags
git tag --sort=-version:refname  # sorted descending
git ls-remote --tags origin      # remote tags
```

### Delete a tag

```bash
# Local only
git tag -d v1.2.3

# Remote only
git push origin --delete v1.2.3

# Both at once
git tag -d v1.2.3 && git push origin --delete v1.2.3
```

### Retag (overwrite an existing tag)

If you tagged the wrong commit:

```bash
# Delete the old tag
git tag -d v1.2.3
git push origin --delete v1.2.3

# Retag at current HEAD
git tag v1.2.3
git push origin v1.2.3
```

> **Note:** If the workflow already ran, a GitHub Release and Homebrew formula have already been created.
> Delete the corresponding GitHub Release before retagging.

---

## Re-running a Failed Workflow

If the workflow fails mid-build, you can re-trigger it from GitHub Actions instead of deleting and recreating the tag:

```
GitHub → alcnsahin/shipper → Actions → Release → Re-run all jobs
```

Or delete and recreate the tag:

```bash
git tag -d v1.2.3 && git push origin --delete v1.2.3
git tag v1.2.3 && git push origin v1.2.3
```

---

## Installation

### macOS — Homebrew (recommended)

```bash
brew tap alcnsahin/tap
brew install shipper
```

`brew tap alcnsahin/tap` adds the `alcnsahin/homebrew-tap` GitHub repository as a Homebrew tap.
This repository is automatically updated by the release workflow on every new tag.

### macOS / Linux — Direct download

```bash
# Apple Silicon (M1/M2/M3)
curl -Lo shipper https://github.com/alcnsahin/shipper/releases/latest/download/shipper-macos-arm64
chmod +x shipper && sudo mv shipper /usr/local/bin/

# Intel Mac
curl -Lo shipper https://github.com/alcnsahin/shipper/releases/latest/download/shipper-macos-x86_64
chmod +x shipper && sudo mv shipper /usr/local/bin/

# Linux x86_64
curl -Lo shipper https://github.com/alcnsahin/shipper/releases/latest/download/shipper-linux-x86_64
chmod +x shipper && sudo mv shipper /usr/local/bin/
```

### Windows

Download `shipper-windows-x86_64.exe` from the [latest release](https://github.com/alcnsahin/shipper/releases/latest),
rename it to `shipper.exe`, and place it in any directory on your `PATH`.

### Build from source

```bash
git clone https://github.com/alcnsahin/shipper
cd shipper
cargo build --release
sudo mv target/release/shipper /usr/local/bin/
```

Requires Rust 1.75+. Install via [rustup.rs](https://rustup.rs).

---

## Upgrade

```bash
# Homebrew — fetch latest formulas first, then upgrade
brew update && brew upgrade shipper

# Manual — re-run the same curl command
curl -Lo shipper https://github.com/alcnsahin/shipper/releases/latest/download/shipper-macos-arm64
chmod +x shipper && sudo mv shipper /usr/local/bin/
```

---

## Verify Installation

```bash
shipper --version
shipper --help
```

---

## How Homebrew Formula Works

When you run `brew install shipper`, Homebrew performs the following steps:

1. Reads `Formula/shipper.rb` from the `alcnsahin/homebrew-tap` repository
2. Detects CPU architecture and selects the correct binary URL (`on_arm` / `on_intel`)
3. Downloads the binary from GitHub Releases
4. Verifies the SHA256 hash against the value in the formula — aborts if they don't match
5. Installs the binary as `shipper` into `/opt/homebrew/bin/` (ARM) or `/usr/local/bin/` (Intel)

No source build. No Rust installation required.
