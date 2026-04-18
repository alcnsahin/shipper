# Shipper

Zero-dependency Rust CLI that builds and submits iOS and Android apps to the
App Store (TestFlight) and Google Play Store from a developer's Mac.

## Scope

- **iOS:** `xcodebuild` → IPA → `xcrun altool` → App Store Connect API
- **Android:** Gradle → AAB/APK → `apksigner`/`jarsigner` → Google Play Developer API v3
- **Expo-aware:** detects `app.json` and runs `expo prebuild` when needed
- **Notifications:** Telegram
- **Out of scope:** backend deployment, cloud build services, CI orchestration

Never propose backend, server, or cloud-build features. All work focuses on
mobile store submissions driven entirely from the local machine.

## Layout

- `src/main.rs` — clap CLI entry, `deploy {ios|android|all}` and `init`
- `src/commands/` — top-level command runners (`deploy`, `init`)
- `src/platforms/` — build pipelines (`ios.rs`, `android.rs`)
- `src/stores/` — upload clients (`appstore.rs`, `playstore.rs`)
- `src/utils/` — `credentials`, `logger`, `notifier`, `progress`, `version`
- `src/config.rs` — `~/.shipper/config.toml` + per-project `shipper.toml`
- `src/error.rs` — error types
- `docs/` — `architecture.md`, `ios-pipeline.md`, `ios-code-signing.md`, `setup.md`, `release.md`

## Conventions

- Rust 2021, async via `tokio`, errors via `anyhow` + `thiserror`
- HTTP via `reqwest` (rustls); TOML config via `serde`/`toml`
- Apple JWT via `jsonwebtoken`; no Ruby, no Fastlane, no Node runtime deps
- Logs via `tracing` / `tracing-subscriber`; progress via `indicatif` + `console`
- Distributed as a single binary (Homebrew tap `alcnsahin/tap` + GitHub releases)

## Build & Run

```bash
cargo build --release
cargo run -- deploy ios
cargo run -- deploy android
cargo run -- init
```

## Key External Tools

`xcodebuild`, `xcrun altool`, `pod`, `security` (Keychain), `keytool`,
`jarsigner`, `apksigner`, `gradle`. These are looked up via `which` at runtime.

## iOS Pipeline Notes

See `docs/ios-pipeline.md` and `docs/ios-code-signing.md`. Known upstream
gotchas documented in `README.md` troubleshooting: Reanimated + New
Architecture, and `fmt` pod C++20 `consteval` under Xcode 16+.
