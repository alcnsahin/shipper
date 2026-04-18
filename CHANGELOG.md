# Changelog

All notable changes to this project are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Security
- Introduced opaque `Secret` wrapper (`src/utils/secret.rs`) that redacts its
  value in `Debug`/`Display` and only reveals raw bytes through an explicit
  `expose()` call at the subprocess/HTTP boundary.
- `config::read_secret` now returns `Secret` and, on Unix, refuses to read
  secret files whose mode grants group/world bits — the error prints the
  exact `chmod 600 <path>` remediation.
- `shipper init` now appends missing credential patterns
  (`*.keystore`, `*.jks`, `*.p8`, `*.p12`, `credentials.json`,
  `google-services.json`, `GoogleService-Info.plist`, `play-store-sa.json`,
  `*-service-account.json`) to an existing `.gitignore` under a
  `# shipper: credential guards` block; skipped silently when `.gitignore`
  is absent.

### Changed
- Store API calls now go through `stores::http::send_with_retry`, a
  bounded exponential-backoff helper (3 attempts, 500ms → 5s cap) that
  retries on network errors, `408`, `429`, and `5xx`, and maps terminal
  failures to typed `ShipperError` variants. Idempotent endpoints
  (ASC builds GET, Google OAuth token exchange, Play Store
  `create_edit`, `assign_to_track`) are wrapped; state-changing ones
  (bundle upload, edit commit, TestFlight submit) bypass retry and use
  `map_status_to_error` / `map_upload_failure` directly. Auth-class
  failures at the Google token endpoint collapse to
  `ShipperError::AuthError`.
- `ShipperError::ApiError`, `AuthError`, and `UploadFailed` are now
  live variants (no more `#[allow(dead_code)]`). Their doc comments
  describe when each fires so future call-sites pick the right shape.
- Narrowed `tokio` feature set to `macros`, `rt-multi-thread`, `process`, `time` (was `full`).
- Dropped `chrono` default features; only `clock` is enabled.
- `utils::logger::init` now reads the default level from
  `global.log_level` in `~/.shipper/config.toml`. Precedence:
  `RUST_LOG` > `--verbose` > config > `"info"`.
- Tool-lookup failures (`xcodebuild`, `xcrun`, `apksigner`/`jarsigner`)
  now return a typed `ShipperError::ToolNotFound { tool, hint }` with a
  concrete remediation string instead of an ad-hoc `anyhow!` message.
- `deploy_ios` / `deploy_android` are annotated with
  `#[tracing::instrument(name = "deploy", fields(platform, app))]` and
  emit an `info!`/`error!` line on completion with `elapsed_ms` (and
  `version`/`build`/`track` on success).
- `main` loads the global config once and threads it through
  `commands::deploy::run` / `Config::with_global`, removing a redundant
  read+parse of `~/.shipper/config.toml` on every `deploy` invocation.
- `logger::init` takes `config_level: &str` directly (the `Option` was
  dead shape — the caller always has a value, since
  `GlobalSection::log_level` has a serde default).
- `ShipperError` trimmed to variants that are either live or scheduled
  for the next two phases (`ToolNotFound`, `ApiError`, `AuthError`,
  `UploadFailed`, `BuildFailed`); speculative shapes removed.

### Removed
- Unused `base64` and `tokio-util` dependencies.
- Empty `patch_gradle_properties` Android helper and its no-op call site.
- Unused `run_command` helper in `platforms/ios.rs`.
- Unused `AppVersion::bump_patch` and `progress::failure` helpers.
- Unused `version` (marketing) parameter on `appstore::poll_build_processing`.

### Added
- `stores/http.rs`: shared retry + error-mapping boundary for store
  API clients. Pure `classify_status` + `map_status_to_error` +
  `map_upload_failure` helpers with unit tests for the success /
  retry / fail classification and for the auth-vs-api-vs-upload
  routing.
- `CHANGELOG.md` (this file).
- `[dev-dependencies]`: `assert_cmd`, `predicates` (moved `tempfile` here
  from runtime deps since it is only used in `#[cfg(test)]` blocks).
- Unit tests for `AppVersion::bump_build`, plist extract/replace, Expo
  iOS/Android version round-trips, Gradle version round-trip, and
  `expand_path` / `read_secret` in `config`.
- CLI smoke tests under `tests/cli.rs` (`--version`, `--help`,
  `deploy --help`).
- GitHub Actions CI workflow (`.github/workflows/ci.yml`):
  rustfmt, clippy (`-D warnings`) and test matrix on macOS + Linux,
  `cargo-audit`, and `cargo-deny`. Uses `Swatinem/rust-cache` and a
  `concurrency` group to cancel superseded runs.
- `deny.toml` baseline configuration (permissive licenses, deny unknown
  registries, warn on duplicate versions).

### Fixed
- `clippy::manual_split_once` in `read_mobileprovision` bundle-id parsing.
- `clippy::manual_strip` in `config::expand_path`.
- `clippy::doc_lazy_continuation` in `ensure_signing_setup` docs.
- Applied `cargo fmt` across the tree so `--check` is clean in CI.

## [0.1.0] - 2025-04-12

Initial release.
