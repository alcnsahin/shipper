# Shipper — Implementation Progress

> Generated: 2026-04-18
> Test suite: 41 passing (38 unit + 3 CLI), clippy `-D warnings` clean

---

## Completed Phases

### Faz 0 — Foundations

| Task | Details |
|------|---------|
| CLAUDE.md | Project conventions, layout, build commands, external tools reference |
| Test harness | `assert_cmd` + `predicates` + `tempfile` dev-deps; CLI smoke tests (`tests/cli.rs`); unit tests for `version`, `config`, `secret`, `init` |
| CI pipeline | `.github/workflows/ci.yml` — rustfmt, clippy (`-D warnings`), test matrix (macOS + Linux), `cargo-audit`, `cargo-deny` |
| Dead code cleanup | Removed unused `base64`, `tokio-util` deps; removed `patch_gradle_properties`, `run_command`, `AppVersion::bump_patch`, `progress::failure` helpers; removed unused `version` param from `poll_build_processing` |
| CHANGELOG.md | Keep-a-Changelog format, SemVer |
| Simplify pass | Dep cleanup, `pub` → `pub(crate)` demotion, `tokio` features narrowed to `macros`/`rt-multi-thread`/`process`/`time`, `chrono` default features dropped |

### Faz 1 — Secret Management & Security

| Task | Details |
|------|---------|
| 1.1 Secret newtype | `src/utils/secret.rs` — opaque `Secret` wrapper; `Debug`/`Display` print `[redacted]`; explicit `expose()` for subprocess/HTTP boundaries; `read_from_file(path)` reads + trims |
| 1.2 Permission guard | `enforce_owner_only_mode()` rejects files where `mode & 0o077 != 0` with `chmod 600` remediation message |
| 1.3 .gitignore guard | `shipper init` appends missing credential patterns (`*.keystore`, `*.jks`, `*.p8`, `*.p12`, `credentials.json`, `google-services.json`, `GoogleService-Info.plist`, `play-store-sa.json`, `*-service-account.json`) under `# shipper: credential guards` block |
| Callers migrated | `config::read_secret` → `Result<Secret>`; `notifier.rs` uses `token.expose()`; `android.rs` uses `ks_secret`/`key_secret` + shadowed `.expose()` |
| Tests | 3 secret unit tests + 4 gitignore-append unit tests |

### Faz 2 — Error Model & Observability

| Task | Details |
|------|---------|
| 2.1 Logger config | `logger::init(verbose, config_level)` reads `global.log_level` from `~/.shipper/config.toml`; precedence: `RUST_LOG` > `--verbose` > config > `"info"` |
| 2.2 ToolNotFound | `ShipperError::ToolNotFound { tool, hint }` with remediation strings at `xcodebuild`, `xcrun`, `apksigner`/`jarsigner` lookup sites |
| 2.3 Tracing spans | `deploy_ios`/`deploy_android` annotated with `#[tracing::instrument]`; emit `elapsed_ms`, `version`, `build`, `track` on completion |
| Simplify pass | `Option<&str>` → `&str` on logger; `#[tracing::instrument]` replaced manual `info_span!` duplication; global config threaded from `main` → `Config::with_global()`; `ShipperError` trimmed from 8 → 5 variants |

### Faz 3 — Store API Hardening (in progress)

| Task | Status | Details |
|------|--------|---------|
| 3.1 Shared HTTP helper | Done | `src/stores/http.rs` — `send_with_retry` with exponential backoff (3 attempts, 500ms → 5s cap); retries on network errors, `408`, `429`, `5xx` |
| 3.2 Retry classifier | Done | Pure `classify_status()` → `RetryDecision` enum; 6 unit tests covering success/retry/fail ranges |
| 3.3 Error mapping | Done | `map_status_to_error` (401/403 → `AuthError`, else `ApiError`), `map_upload_failure` (401/403 → `AuthError`, else `UploadFailed`); 3 unit tests |
| 3.4 Rewire appstore.rs | Done | `poll_build_processing` GET → `send_with_retry`; `submit_to_testflight` → `map_status_to_error` (409 still treated as success) |
| 3.5 Rewire playstore.rs | Done | Token exchange → `send_with_retry` + 4xx→`AuthError` promotion; `create_edit` → retry; `assign_to_track` PUT → retry; `commit_edit` → single-shot + typed error; `upload_bundle` → single-shot + `UploadFailed` |
| 3.6 Activate error variants | Done | `#[allow(dead_code)]` removed from `ApiError`, `AuthError`, `UploadFailed` |
| 3.7 Simplify pass | Done | `pub` → `pub(crate)` on `appstore`/`playstore` modules; `generate_jwt` and `ProcessingState` demoted to private; removed redundant type annotation in `playstore.rs`; fixed stale phase reference in `BuildAttributes` comment |

### Faz 4 — Build Pipeline Hardening

| Task | Status | Details |
|------|--------|---------|
| 4.1 Activate BuildFailed | Done | `#[allow(dead_code)]` removed; `anyhow::bail!` → `ShipperError::BuildFailed` at expo prebuild (iOS+Android), pod install, xcodebuild archive, xcodebuild exportArchive, gradlew bundleRelease, gradlew assembleRelease |
| 4.2 Structured build output | Done | `tail_lines` helper in both platform files; `build_apk` now extracts last 60 lines (was raw stderr); all build failures include structured tail output |
| 4.3 Subprocess timeout | Done | `tokio::time::timeout` wraps all build subprocesses: prebuild/pod (10 min), xcodebuild archive (30 min), export (10 min), Gradle builds (30 min), altool upload (15 min); timeout → `BuildFailed` with human-readable message |

### Faz 5 — Concurrency & Lock

| Task | Status | Details |
|------|--------|---------|
| 5.1 Deploy lock | Done | `src/utils/lock.rs` — `DeployLock` RAII guard writes PID to `~/.shipper/<project>/deploy.lock`; stale lock detection (dead PID); auto-cleanup on `Drop`; 3 unit tests |
| 5.2 Parallel deploy all | Done | `deploy all` bumps Expo versions sequentially (avoids app.json race), then runs iOS + Android via `tokio::join!`; both results reported even if one fails; native projects bump in parallel (separate files); `pre_bumped: Option<AppVersion>` threaded into platform deploy fns |

### Faz 6 — Config Validation

| Task | Status | Details |
|------|--------|---------|
| 6.1 Schema validation | Done | `#[serde(deny_unknown_fields)]` on all project config structs; `ProjectConfig::validate()` checks empty names, valid enum values (track, build_type, export_method, strategy); wired into `load_project_config`; 5 unit tests |
| 6.2 Validate subcommand | Done | `shipper validate` — parses + validates both `shipper.toml` and `~/.shipper/config.toml`; checks credential file accessibility; reports iOS/Android section presence |

### Faz 7 — Subprocess Hardening

| Task | Status | Details |
|------|--------|---------|
| 7.1 Env-piped passwords | Done | `jarsigner` uses `-storepass:env`/`-keypass:env`; `apksigner` uses `env:SHIPPER_KS_PASS`/`env:SHIPPER_KEY_PASS`; passwords injected as child-process env vars only — invisible to `ps aux` |
| 7.2 Structured exit codes | Done | All `BuildFailed` messages include exit code; signing failures (`apksigner`, `jarsigner`) now capture + display stderr; `exit_code_str` helper in both platform files |

### Faz 8 — UX Polish

| Task | Status | Details |
|------|--------|---------|
| 8.1 `--dry-run` flag | Done | `shipper deploy ios --dry-run` — validates config, shows version/scheme/track/destination without building or uploading; works for all targets (ios, android, all) |
| 8.2 Summary table | Done | Box-drawn table at end of each successful deploy showing platform, version (build), destination, elapsed time; `format_duration` helper (Xm Ys) |
| 8.3 Timed spinners | Done | `timed_spinner` shows elapsed time on long-running steps: pod install, xcodebuild archive/export, gradlew bundle/assemble, altool upload |

### Faz 9 — Store Feature Completion

| Task | Status | Details |
|------|--------|---------|
| 9.1 TestFlight submission | Done | `submit_to_testflight` wired into deploy pipeline; `#[allow(dead_code)]` removed; automatic beta review submission when `testflight_groups` is configured |
| 9.1 Group assignment | Done | `add_build_to_beta_group(creds, app_id, build_id, group_name)` — looks up group by name via ASC API, adds build; 409 treated as success |
| 9.1 Config | Done | `testflight_groups: Vec<String>` on `IosConfig` (default empty); 2 deserialization tests |
| 9.2 Staged rollout | Done | `rollout_fraction: Option<f64>` on `AndroidConfig`; `assign_to_track` conditionally uses `"inProgress"` + `userFraction` on production track; non-production tracks always 100% |
| 9.2 Validation | Done | Rejects fraction outside (0.0, 1.0] and fraction < 1.0 on non-production tracks; 3 validation tests |
| 9.2 UX | Done | Dry-run shows `Rollout: X% staged`; summary table shows `Play Store (production, X% staged rollout)` |
| 9.3 ASC diagnostics | Done | `ProcessedBuild` struct exposes `id`, `version`, `uploaded_date`; polling shows version and upload timestamp; `#[allow(dead_code)]` removed from `BuildAttributes` |
| 9.4 Simplify pass | Done | Removed `SlackConfig` dead code; removed resolved phase-reference comments; `pub` → `pub(crate)` on `upload_aab`, `poll_build_processing`; `cargo fmt` + `cargo clippy -D warnings` clean |

---

## Key Metrics

| Metric | Value |
|--------|-------|
| Unit tests | 38 |
| CLI tests | 3 |
| Total tests | 41 |
| `ShipperError` variants | 5 (all live) |
| Clippy | Clean (`-D warnings`) |
| Fmt | Clean |
| Dependencies (runtime) | 14 |
| Dependencies (dev) | 3 |

## File Map (modified/created since v0.1.0)

```
src/
├── commands/
│   ├── deploy.rs        (Faz 2+5+9: tracing::instrument, deploy lock, parallel deploy all, rollout destination)
│   ├── init.rs          (Faz 1: .gitignore guards)
│   └── validate.rs      (Faz 6: NEW — dry-run config checks)
├── config.rs            (Faz 1+2+6+9: Secret return, load_global_or_default, deny_unknown_fields, validate(), testflight_groups, rollout_fraction, SlackConfig removed)
├── error.rs             (Faz 2+3: trimmed to 5 variants, all live)
├── main.rs              (Faz 2: global config bootstrap before logger)
├── platforms/
│   ├── android.rs       (Faz 1+2+4+5+9: Secret, ToolNotFound, BuildFailed, timeouts, pre_bumped, rollout_fraction)
│   └── ios.rs           (Faz 2+4+5+9: ToolNotFound, BuildFailed, timeouts, pre_bumped, TestFlight groups, ProcessedBuild)
├── stores/
│   ├── appstore.rs      (Faz 3+9: send_with_retry, typed errors, ProcessedBuild, submit_to_testflight wired, add_build_to_beta_group)
│   ├── http.rs          (Faz 3: NEW — retry + error boundary)
│   ├── mod.rs           (Faz 3: registered http module)
│   └── playstore.rs     (Faz 3+9: retry on idempotent, typed errors, staged rollout via rollout_fraction)
└── utils/
    ├── credentials.rs
    ├── lock.rs          (Faz 5: NEW — file-based deploy lock)
    ├── logger.rs        (Faz 2: config_level param)
    ├── mod.rs           (Faz 1+5: registered secret, lock modules)
    ├── notifier.rs      (Faz 1: token.expose())
    ├── progress.rs
    ├── secret.rs        (Faz 1: NEW — Secret newtype)
    └── version.rs

tests/
└── cli.rs               (Faz 0: NEW — CLI smoke tests)

.github/workflows/ci.yml (Faz 0: NEW)
CHANGELOG.md             (Faz 0: NEW)
deny.toml                (Faz 0: NEW)
```

