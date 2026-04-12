# Shipper

**Local deployment CLI for mobile apps. No EAS. No GitHub Actions. No cloud build services.**

Build, sign, and submit your iOS and Android apps to the stores from your Mac — with a single command.

```bash
shipper deploy ios        # Build → Sign → TestFlight
shipper deploy android    # Build → Sign → Play Store
shipper deploy all        # Both, sequentially
```

Expo-aware: detects `app.json` and runs `expo prebuild` automatically.

---

## Installation

### macOS (Homebrew) — recommended

```bash
brew install stelikon/tap/shipper
```

### Direct download

Download the binary for your platform from the [latest release](https://github.com/stelikon/shipper/releases/latest):

| Platform | Binary |
|----------|--------|
| macOS Apple Silicon (M1/M2/M3) | `shipper-macos-arm64` |
| macOS Intel | `shipper-macos-x86_64` |
| Linux x86_64 | `shipper-linux-x86_64` |
| Windows x86_64 | `shipper-windows-x86_64.exe` |

```bash
# macOS / Linux
curl -Lo shipper https://github.com/stelikon/shipper/releases/latest/download/shipper-macos-arm64
chmod +x shipper
sudo mv shipper /usr/local/bin/
```

### Build from source

```bash
git clone https://github.com/stelikon/shipper
cd shipper
cargo build --release
sudo mv target/release/shipper /usr/local/bin/
```

Requires Rust 1.75+. Install via [rustup.rs](https://rustup.rs).

---

## Quick Start

```bash
# 1. Initialize in your project root
cd your-app/
shipper init

# 2. Edit credentials (one-time setup)
nano ~/.shipper/config.toml

# 3. Ship
shipper deploy ios
```

### `shipper init`

Interactive setup that generates `shipper.toml` in your project root.

For Expo projects, `init` automatically reads `app.json` and `eas.json` and pre-fills:
- Bundle ID / Package name
- iOS scheme
- App Store Connect App ID
- Apple Team ID
- Google service account path

---

## Configuration

### Global credentials — `~/.shipper/config.toml`

```toml
[global]
notify = ["telegram"]
log_level = "info"

[credentials.apple]
team_id = "QC686RQ858"
key_id = "W54D6Z8Y5M"
issuer_id = "your-issuer-id"
key_path = "~/.shipper/keys/AuthKey_W54D6Z8Y5M.p8"

[credentials.google]
service_account = "~/.shipper/keys/play-store-sa.json"

[notifications.telegram]
bot_token_path = "~/.shipper/keys/telegram-bot-token"
chat_id = "-100xxxxxxxxxx"
```

### Per-project — `shipper.toml`

```toml
[project]
name = "cyberchan"

[ios]
workspace = "ios/CyberChan.xcworkspace"
scheme = "CyberChan"
bundle_id = "app.cyberchan.mobile"
asc_app_id = "6762051322"
export_method = "app-store"

[android]
project_dir = "android"
package_name = "app.cyberchan.mobile"
track = "internal"               # internal | alpha | beta | production
keystore_path = "~/.shipper/keys/cyberchan.keystore"
keystore_alias = "release"
keystore_password_path = "~/.shipper/keys/keystore-password"
build_type = "bundle"            # bundle (AAB) | apk

[versioning]
strategy = "semver"
auto_increment = true
```

---

## Credentials Setup

### Apple — App Store Connect API Key

1. Go to [App Store Connect → Users and Access → Integrations → App Store Connect API](https://appstoreconnect.apple.com/access/integrations/api)
2. Generate a key with **Developer** role
3. Download `AuthKey_XXXXXX.p8` — you can only download it once
4. Save to `~/.shipper/keys/AuthKey_XXXXXX.p8`
5. Note your **Key ID** and **Issuer ID**

```bash
chmod 600 ~/.shipper/keys/AuthKey_XXXXXX.p8
```

### Google — Play Store Service Account

1. Go to [Google Play Console → Setup → API access](https://play.google.com/console/developers/api-access)
2. Link to a Google Cloud project
3. Create a service account with **Release Manager** role
4. Download the JSON key
5. Save to `~/.shipper/keys/play-store-sa.json`

```bash
chmod 600 ~/.shipper/keys/play-store-sa.json
```

### Android Keystore

```bash
# Generate a new keystore (if you don't have one)
keytool -genkey -v \
  -keystore ~/.shipper/keys/release.keystore \
  -alias release \
  -keyalg RSA -keysize 2048 \
  -validity 10000

# Save the password to a file
echo "your-keystore-password" > ~/.shipper/keys/keystore-password
chmod 600 ~/.shipper/keys/keystore-password
chmod 600 ~/.shipper/keys/release.keystore
```

---

## iOS Pipeline

```
shipper deploy ios
│
├─ 1. Bump build number       app.json or Info.plist
├─ 2. expo prebuild           (Expo projects only)
├─ 3. pod install             (if Podfile exists)
├─ 4. xcodebuild archive      → build/shipper/*.xcarchive
├─ 5. xcodebuild -export      → build/shipper/ipa/*.ipa
├─ 6. xcrun altool upload     → App Store Connect
├─ 7. Poll processing state   → wait for VALID
└─ 8. Notify                  → Telegram / Slack
```

**Prerequisites:** macOS, Xcode, CocoaPods (for Expo/RN projects)

---

## Android Pipeline

```
shipper deploy android
│
├─ 1. Bump versionCode        app.json or build.gradle
├─ 2. expo prebuild           (Expo projects only)
├─ 3. ./gradlew bundleRelease → app-release.aab
├─ 4. apksigner               → signed .aab
├─ 5. Play Store API v3       → upload + assign track + commit
└─ 6. Notify
```

**Prerequisites:** Android SDK (`apksigner` in PATH), Java

---

## Why not EAS / Fastlane / GitHub Actions?

| Tool | Problem |
|------|---------|
| EAS Build | Cloud dependency, build credits, queue times |
| Fastlane | Ruby dependency hell, slow, needs maintenance |
| GitHub Actions | YAML complexity, secret management, runner costs |
| **Shipper** | Single binary, runs on your machine, no cloud needed |

---

## License

Private — Stelikon internal tooling.
