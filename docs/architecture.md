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
│   │      │ 1. version bump │        │ 1. version bump    │   │  │
│   │      │ 2. expo prebuild│        │ 2. expo prebuild   │   │  │
│   │      │ 3. pod install  │        │ 3. gradle build    │   │  │
│   │      │ 4. xcodebuild   │        │ 4. apksigner       │   │  │
│   │      │ 5. export IPA   │        │ 5. play store API  │   │  │
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
│       └── release.yml          # CI: build + GitHub Release + homebrew update
├── Cargo.toml
└── docs/
    ├── architecture.md
    ├── ios-pipeline.md
    └── release.md
```

---

## iOS Pipeline — Step by Step

```
shipper deploy ios
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
│
├─ 4. CocoaPods  (if Podfile exists)
│      pod install --repo-update
│
├─ 5. xcodebuild archive
│      xcodebuild archive -workspace ... -scheme ... -archivePath ...
│      → build/shipper/<Scheme>.xcarchive
│
├─ 6. Export IPA
│      Generates ExportOptions.plist from shipper.toml
│      xcodebuild -exportArchive → build/shipper/ipa/<App>.ipa
│
├─ 7. Upload
│      xcrun altool --upload-app --apiKey ... --apiIssuer ...
│
├─ 8. Poll App Store Connect
│      GET /v1/builds?filter[app]=...
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
├── config.toml          ← global credentials & notification settings
└── keys/
    ├── AuthKey_XXXX.p8  ← Apple App Store Connect API key (ES256)
    ├── play-store.json  ← Google service account JSON (RS256)
    ├── keystore-password← Android keystore password (plain text, chmod 600)
    └── telegram-token   ← Telegram bot token (optional)

./shipper.toml           ← per-project: platform config, bundle IDs, schemes
```

Config yükleme sırası:

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
  header: { alg: ES256, kid: KEY_ID }
  payload: { iss: ISSUER_ID, iat: now, exp: now+1200, aud: appstoreconnect-v1 }
    │
    ▼
Authorization: Bearer <jwt>   →   api.appstoreconnect.apple.com
```

Token TTL: 20 dakika (Apple limiti). Her API çağrısından önce yeniden üretilir.

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

| Proje tipi | Dosya | Alan |
|------------|-------|------|
| Expo / iOS | `app.json` | `expo.ios.buildNumber` |
| Expo / Android | `app.json` | `expo.android.versionCode` |
| Native iOS | `ios/*/Info.plist` | `CFBundleVersion`, `CFBundleShortVersionString` |
| Native Android | `android/app/build.gradle` | `versionCode`, `versionName` |

`auto_increment = true` (default) her deploy'da build number'ı 1 artırır.

---

## Error Handling

| Katman | Strateji |
|--------|----------|
| Preflight | Araçlar yoksa deploy başlamadan çıkar |
| Build hataları | `xcodebuild` / `gradle` stderr'den ilk 10-15 satır alınır |
| API hataları | HTTP status + response body gösterilir |
| ASC processing | 20 dk timeout, 30s polling interval |
| Notification | Non-fatal — bildirim hatası deploy'u durdurmaz |

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
GitHub Release  (tag adında '-' varsa: prerelease)
  + SHA256 hesapla
        │
        ▼
alcnsahin/homebrew-tap → Formula/shipper.rb güncelle
        │
        ▼
brew upgrade shipper   ← kullanıcı tarafında
```

---

## Key Design Decisions

| Karar | Neden |
|-------|-------|
| Rust | Single binary, zero runtime deps, fast startup |
| `rustls` yerine `native-tls` yok | musl static build ile uyumluluk |
| `xcrun altool` | Tek subprocess çağrısı, Apple toolchain'e dahil |
| Google Play edit/commit modeli | Atomik upload: commit çağrılmadıkça store'a yansımaz |
| Secrets dosyadan okunur | `.env` veya env var yerine `chmod 600` dosyalar — daha explicit |
| Expo auto-detect | `app.json` varlığına bakılır, prebuild otomatik çalışır |
