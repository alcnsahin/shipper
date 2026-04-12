# Shipper — Architecture

## System Overview

```
┌──────────────────────────────────────────────────────────┐
│                     shipper CLI                          │
│                                                          │
│  ┌─────────┐  ┌──────────┐                              │
│  │  init   │  │  deploy  │                              │
│  └────┬────┘  └────┬─────┘                              │
│       │            │                                     │
│  ┌────┴────────────┴────────────────────────────────┐   │
│  │              Command Router (clap)               │   │
│  └─────────────────────┬────────────────────────────┘   │
│                        │                                 │
│  ┌─────────────────────┴────────────────────────────┐   │
│  │              Platform Orchestrator               │   │
│  │                                                  │   │
│  │  ┌───────────────────┐  ┌──────────────────────┐ │   │
│  │  │       iOS         │  │      Android         │ │   │
│  │  │                   │  │                      │ │   │
│  │  │ expo prebuild     │  │ gradle bundleRelease  │ │   │
│  │  │ pod install       │  │ apksigner            │ │   │
│  │  │ xcodebuild archive│  │ play store upload    │ │   │
│  │  │ export IPA        │  │                      │ │   │
│  │  │ altool upload     │  │                      │ │   │
│  │  │ asc poll          │  │                      │ │   │
│  │  └────────┬──────────┘  └──────────┬───────────┘ │   │
│  └───────────┴─────────────────────────┴─────────────┘   │
│                        │                                 │
│  ┌─────────────────────┴────────────────────────────┐   │
│  │              Store Connectors                    │   │
│  │                                                  │   │
│  │  ┌──────────────────────┐  ┌───────────────────┐ │   │
│  │  │  App Store Connect   │  │   Play Store      │ │   │
│  │  │  API v1 (ES256 JWT)  │  │  API v3 (OAuth2)  │ │   │
│  │  └──────────────────────┘  └───────────────────┘ │   │
│  └──────────────────────────────────────────────────┘   │
│                        │                                 │
│  ┌─────────────────────┴────────────────────────────┐   │
│  │         Config · Version · Log · Notify          │   │
│  └──────────────────────────────────────────────────┘   │
└──────────────────────────────────────────────────────────┘
```

## Data Flow

```
User runs: shipper deploy ios

1. Load ~/.shipper/config.toml     → Global credentials
2. Load ./shipper.toml             → Project config
3. Detect project type             → Expo / Native
4. Bump version                    → app.json / Info.plist
5. Expo prebuild (if Expo)         → generates ios/ dir
6. pod install                     → CocoaPods dependencies
7. xcodebuild archive              → .xcarchive
8. xcodebuild -exportArchive       → .ipa
9. xcrun altool upload             → App Store Connect
10. Poll ASC API                   → wait for VALID state
11. Send notification              → Telegram / Slack
```

```
User runs: shipper deploy android

1. Load config files
2. Bump versionCode / versionName  → app.json / build.gradle
3. Expo prebuild (if Expo)         → generates android/ dir
4. ./gradlew bundleRelease         → app-release.aab
5. apksigner                       → signed .aab
6. Play Store API v3               → create edit → upload → assign track → commit
7. Send notification
```

## Config Hierarchy

```
~/.shipper/
├── config.toml          # Global: credentials, notification settings
└── keys/
    ├── AuthKey_XXX.p8   # Apple App Store Connect API key
    ├── play-store.json  # Google service account
    └── telegram-token   # Notification bot token (optional)

./shipper.toml           # Per-project: platforms, bundle IDs, schemes
```

## Credential Model

- All secrets live in `~/.shipper/keys/` — never in the project repo
- Apple: ES256 JWT generated on-the-fly from `.p8` key (20 min TTL)
- Google: RS256 JWT → OAuth2 access token from service account JSON
- Passwords for keystores are read from plain text files (set `chmod 600`)

## Error Handling

1. **Pre-flight checks** — verify tools exist (`xcodebuild`, `apksigner`, etc.) before starting
2. **Fail fast** — stop on first error with a clear message
3. **Non-fatal notifications** — a failed Telegram message never aborts a deploy
