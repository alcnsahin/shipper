# Shipper — Architecture

## System Overview

```
┌─────────────────────────────────────────────────────────────────┐
│                          shipper CLI                            │
│                                                                 │
│   shipper init          shipper deploy ios/android/all          │
│        │                          │                             │
│   ┌────▼──────────────────────────▼──────────────────────────┐  │
│   │                  Command Router (clap)                   │  │
│   └────────────────────────────┬─────────────────────────────┘  │
│                                │                                │
│   ┌────────────────────────────▼─────────────────────────────┐  │
│   │                 Platform Orchestrator                    │  │
│   │                   commands/deploy.rs                     │  │
│   │                                                          │  │
│   │      ┌─────────────────┐        ┌────────────────────┐   │  │
│   │      │   iOS Pipeline  │        │  Android Pipeline  │   │  │
│   │      │ platforms/ios.rs│        │platforms/android.rs│   │  │
│   │      │                 │        │                    │   │  │
│   │      │ 0. signing setup│        │ 1. version bump    │   │  │
│   │      │ 1. version bump │        │ 2. expo prebuild   │   │  │
│   │      │ 2. expo prebuild│        │ 3. gradle build    │   │  │
│   │      │ 3. pod install  │        │ 4. apksigner       │   │  │
│   │      │ 4. xcodebuild   │        │ 5. play store API  │   │  │
│   │      │ 5. export IPA   │        │                    │   │  │
│   │      │ 6. altool upload│        │                    │   │  │
│   │      │ 7. asc poll     │        │                    │   │  │
│   │      └────────┬────────┘        └─────────┬──────────┘   │  │
│   └───────────────┴─────────────────────────── ┴─────────────┘  │
│                   │                             │               │
│   ┌───────────────▼─────────────────────────────▼─────────────┐  │
│   │                    Store Connectors                       │  │
│   │                                                           │  │
│   │   ┌──────────────────────────┐  ┌──────────────────────┐  │  │
│   │   │   App Store Connect API  │  │  Google Play API v3  │  │  │
│   │   │   stores/appstore.rs     │  │  stores/playstore.rs │  │  │
│   │   │                          │  │                      │  │  │
│   │   │  JWT (ES256, .p8 key)    │  │  OAuth2 (RS256,      │  │  │
│   │   │  Build polling           │  │  service account)    │  │  │
│   │   └──────────────────────────┘  └──────────────────────┘  │  │
│   └───────────────────────────────────────────────────────────┘  │
│                                │                                │
│   ┌────────────────────────────▼─────────────────────────────┐  │
│   │                   Cross-cutting Concerns                 │  │
│   │                                                          │  │
│   │  config.rs     utils/version.rs    utils/notifier.rs    │  │
│   │  TOML parsing  semver bump         Telegram / Slack      │  │
│   │                                                          │  │
│   │  utils/progress.rs                 utils/logger.rs       │  │
│   │  Spinner / step output             tracing subscriber    │  │
│   └──────────────────────────────────────────────────────────┘  │
└─────────────────────────────────────────────────────────────────┘
```

---

## Source Tree

```
shipper/
├── src/
│   ├── main.rs                  # CLI entry point (clap)
│   ├── config.rs                # Config parsing + credential helpers
│   ├── error.rs                 # Error types
│   ├── commands/
│   │   ├── deploy.rs            # Deploy orchestrator + notification dispatch
│   │   └── init.rs              # Interactive init, Expo auto-detect
│   ├── platforms/
│   │   ├── ios.rs               # iOS build pipeline (7 steps)
│   │   └── android.rs           # Android build pipeline (5 steps)
│   ├── stores/
│   │   ├── appstore.rs          # App Store Connect API (JWT, build polling)
│   │   └── playstore.rs         # Google Play Developer API v3 (OAuth2)
│   └── utils/
│       ├── version.rs           # Version bump (Info.plist, build.gradle, app.json)
│       ├── notifier.rs          # Telegram / Slack notifications
│       ├── progress.rs          # Terminal output (spinner, steps)
│       └── logger.rs            # tracing-subscriber init
├── .github/
│   └── workflows/
│       └── release.yml          # CI: build + GitHub Release + Homebrew update
├── Cargo.toml
└── docs/
    ├── architecture.md
    ├── ios-pipeline.md
    ├── ios-code-signing.md
    ├── setup.md
    └── release.md
```

---

## iOS Pipeline — Step by Step

```
shipper deploy ios
│
├─ 0. Auto-install signing credentials
│      Check Keychain for dist cert, check disk for provisioning profile
│      If missing: search ~/.shipper/keys/<bundle_id>/ → install automatically
│
├─ 1. Preflight checks
│      xcodebuild, xcrun — exits early if missing
│
├─ 2. Version bump
│      Expo:   app.json → expo.ios.buildNumber += 1
│      Native: ios/<App>/Info.plist → CFBundleVersion += 1
│
├─ 3. Expo prebuild  (if app.json contains "expo")
│      npx expo prebuild --platform ios --clean
│      Workspace/scheme names derived from expo.name (not expo.scheme)
│
├─ 4. CocoaPods  (if Podfile exists)
│      pod install --repo-update
│
├─ 5. xcodebuild archive
│      xcodebuild archive -workspace ... -scheme ...
│      DEVELOPMENT_TEAM=<team_id> CODE_SIGN_STYLE=Manual
│      → build/shipper/<Scheme>.xcarchive
│
├─ 6. Export IPA
│      ExportOptions.plist: method=app-store-connect, destination=export
│      xcodebuild -exportArchive → build/shipper/ipa/<App>.ipa
│
├─ 7. Upload
│      Copies .p8 key to ~/.appstoreconnect/private_keys/ if needed
│      xcrun altool --upload-app --apiKey ... --apiIssuer ...
│
├─ 8. Poll App Store Connect  [skipped if asc_app_id not set]
│      GET /v1/builds?filter[app]=...&filter[version]=<build_number>
│      Polls every 30s until processingState == VALID (max 20 min)
│
└─ 9. Notify
       Telegram / Slack → "AppName v1.0.1 (42) → TestFlight ✅"
```

## Android Pipeline — Step by Step

```
shipper deploy android
│
├─ 1. Preflight checks
│      gradlew exists, keystore exists, apksigner/jarsigner in PATH
│
├─ 2. Version bump
│      Expo:   app.json → expo.android.versionCode += 1
│      Native: android/app/build.gradle → versionCode += 1
│
├─ 3. Expo prebuild  (if Expo project)
│      npx expo prebuild --platform android --clean
│
├─ 4. Gradle build
│      ./gradlew bundleRelease  → app/build/outputs/bundle/release/app-release.aab
│      ./gradlew assembleRelease  (if build_type = "apk")
│
├─ 5. Sign
│      apksigner sign --ks ... --out app-release-signed.aab ...
│      (falls back to jarsigner if apksigner not found)
│
├─ 6. Google Play API v3
│      POST /edits              → create edit
│      POST /edits/{id}/bundles → upload AAB
│      PUT  /edits/{id}/tracks  → assign to track (internal/alpha/beta/production)
│      POST /edits/{id}:commit  → publish
│
└─ 7. Notify
```

---

## Config Model

```
~/.shipper/
├── config.toml              ← global credentials & notification settings
└── keys/
    ├── AuthKey_XXXX.p8      ← Apple App Store Connect API key (ES256)
    ├── play-store.json      ← Google service account JSON (RS256)
    ├── keystore-password    ← Android keystore password (plain text, chmod 600)
    ├── telegram-token       ← Telegram bot token (optional)
    └── <bundle_id>/         ← per-app iOS signing credentials
        ├── dist-cert.p12
        ├── profile.mobileprovision
        └── credentials.json ← { "certPassword": "..." }

./shipper.toml               ← per-project: platform config, bundle IDs, schemes
```

Config loading order:

```rust
Config::load()
  └─ load_global_config()   // ~/.shipper/config.toml  (missing = defaults)
  └─ load_project_config()  // ./shipper.toml          (missing = hard error)
```

---

## Authentication

### Apple — App Store Connect API

```
.p8 file (EC private key, P-256)
    │
    ▼
ES256 JWT
  header:  { alg: ES256, kid: KEY_ID }
  payload: { iss: ISSUER_ID, iat: now, exp: now+1200, aud: appstoreconnect-v1 }
    │
    ▼
Authorization: Bearer <jwt>   →   api.appstoreconnect.apple.com
```

Token TTL: 20 minutes (Apple limit). Regenerated before each API call.

### Google — Play Store API

```
service-account.json (RSA private key)
    │
    ▼
RS256 JWT  →  POST oauth2.googleapis.com/token
    │
    ▼
access_token (TTL: 1h)
    │
    ▼
Authorization: Bearer <token>  →  androidpublisher.googleapis.com
```

---

## Version Bump Strategy

| Project type   | File                              | Field                                          |
|----------------|-----------------------------------|------------------------------------------------|
| Expo / iOS     | `app.json`                        | `expo.ios.buildNumber`                         |
| Expo / Android | `app.json`                        | `expo.android.versionCode`                     |
| Native iOS     | `ios/*/Info.plist`                | `CFBundleVersion`, `CFBundleShortVersionString` |
| Native Android | `android/app/build.gradle`        | `versionCode`, `versionName`                   |

`auto_increment = true` (default) increments the build number by 1 on every deploy.

---

## Error Handling

| Layer        | Strategy                                                        |
|--------------|-----------------------------------------------------------------|
| Preflight    | Exits before starting if required tools are missing             |
| Build errors | First 10–15 lines of `xcodebuild` / `gradle` stderr are shown  |
| API errors   | HTTP status + response body are surfaced                        |
| ASC processing | 20-minute timeout, 30-second polling interval                |
| Notification | Non-fatal — a failed notification never aborts a deploy         |

---

## CI / Release Pipeline

```
git tag v1.2.3
git push origin v1.2.3
        │
        ▼
.github/workflows/release.yml
        │
        ├─ [macos-14] cargo build --target aarch64-apple-darwin
        ├─ [macos-14] cargo build --target x86_64-apple-darwin  (cross)
        ├─ [ubuntu]   cargo build --target x86_64-unknown-linux-musl
        └─ [windows]  cargo build --target x86_64-pc-windows-msvc
        │
        ▼
GitHub Release  (marked prerelease if tag contains '-')
  + compute SHA256 of macOS binaries
        │
        ▼
alcnsahin/homebrew-tap → Formula/shipper.rb updated
        │
        ▼
brew upgrade shipper   ← user side
```

---

## Key Design Decisions

| Decision | Reason |
|----------|--------|
| Rust | Single binary, zero runtime dependencies, fast startup |
| `rustls` instead of `native-tls` | Compatible with musl static builds — no OpenSSL dependency |
| `xcrun altool` | Single subprocess call, included with Apple's Xcode toolchain |
| Google Play edit/commit model | Atomic upload: changes are not published until `commit` is called |
| Secrets read from files | Explicit `chmod 600` files instead of env vars or `.env` — harder to accidentally leak |
| Expo auto-detect | Detects `app.json` presence and runs `prebuild` automatically |
