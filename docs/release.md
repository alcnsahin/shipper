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

Tag push'u GitHub Actions workflow'unu tetikler. Tag adı `v` ile başlamak zorunda (`v*` pattern).

### Pre-release tag

Tag adında `-` içerirse GitHub Release otomatik olarak **pre-release** işaretlenir:

```bash
git tag v1.2.3-beta
git tag v1.2.3-rc.1
git push origin v1.2.3-beta
```

### List tags

```bash
git tag                        # local tags
git tag --sort=-version:refname  # sorted descending
git ls-remote --tags origin    # remote tags
```

### Delete a tag

```bash
# Local
git tag -d v1.2.3

# Remote
git push origin --delete v1.2.3

# Both at once
git tag -d v1.2.3 && git push origin --delete v1.2.3
```

### Retag (overwrite an existing tag)

Yanlış commit'e tag attıysan:

```bash
# Delete old
git tag -d v1.2.3
git push origin --delete v1.2.3

# Retag at current HEAD
git tag v1.2.3
git push origin v1.2.3
```

> **Not:** Workflow daha önce çalıştıysa GitHub Release ve homebrew formula zaten oluşmuştur.
> Retaglemeden önce GitHub'da ilgili Release'i de silmen gerekir.

---

## Re-running a Failed Workflow

Workflow build sırasında hata aldıysa tag'i silip yeniden oluşturmak yerine
GitHub Actions üzerinden yeniden tetikleyebilirsin:

```
GitHub → alcnsahin/shipper → Actions → Release → Re-run all jobs
```

Veya tag'i sil/yeniden oluştur:

```bash
git tag -d v1.2.3 && git push origin --delete v1.2.3
git tag v1.2.3 && git push origin v1.2.3
```

---

## Installation

### macOS — Homebrew (önerilen)

```bash
brew tap alcnsahin/tap
brew install shipper
```

`brew tap alcnsahin/tap` → GitHub'da `alcnsahin/homebrew-tap` reposunu ekler.
Bu repo, her release'de workflow tarafından otomatik güncellenir.

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

[Releases sayfasından](https://github.com/alcnsahin/shipper/releases/latest) `shipper-windows-x86_64.exe`'yi indir,
`shipper.exe` olarak yeniden adlandır ve PATH'te olan bir dizine koy.

### Build from source

```bash
git clone https://github.com/alcnsahin/shipper
cd shipper
cargo build --release
sudo mv target/release/shipper /usr/local/bin/
```

Rust 1.75+ gerektirir. → [rustup.rs](https://rustup.rs)

---

## Upgrade

```bash
# Homebrew
brew upgrade shipper

# Manuel — aynı curl komutunu tekrar çalıştır
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

`brew install shipper` çalıştırıldığında Homebrew şu adımları izler:

1. `alcnsahin/homebrew-tap` reposundaki `Formula/shipper.rb` dosyasını okur
2. CPU mimarisine göre (`on_arm` / `on_intel`) doğru binary URL'ini seçer
3. Binary'yi GitHub Releases'ten indirir
4. SHA256 hash'ini formüldeki değerle karşılaştırır — eşleşmezse hata verir
5. Binary'yi `/opt/homebrew/bin/shipper` (ARM) veya `/usr/local/bin/shipper` (Intel) olarak kurar

Kaynak build yoktur, Rust kurulumu gerekmez.
