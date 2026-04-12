# Shipper — iOS Deployment Pipeline

## Overview

The iOS pipeline handles the complete flow from source code to TestFlight/App Store, running entirely on the developer's local Mac.

## Prerequisites

- **macOS** with Xcode installed (`xcode-select --install`)
- **Apple Developer Account** with App Store Connect API key (.p8)
- **Distribution certificate + provisioning profile** — installed automatically by shipper (see [iOS Code Signing Setup](ios-code-signing.md))

## Apple Credentials Setup

### App Store Connect API Key

Generate at: https://appstoreconnect.apple.com/access/integrations/api

```
Key ID:     W54D6Z8Y5M
Issuer ID:  your-issuer-id (shown on the API keys page)
Key File:   AuthKey_W54D6Z8Y5M.p8 (downloaded once, save securely)
```

Store at `~/.shipper/keys/AuthKey_W54D6Z8Y5M.p8`

### JWT Token Generation

App Store Connect API uses short-lived JWTs:

```
Header:
{
  "alg": "ES256",
  "kid": "W54D6Z8Y5M",          // Key ID
  "typ": "JWT"
}

Payload:
{
  "iss": "your-issuer-id",       // Issuer ID
  "iat": <current_time>,
  "exp": <current_time + 1200>,  // 20 min max
  "aud": "appstoreconnect-v1"
}

Signed with: AuthKey_W54D6Z8Y5M.p8 (ES256/P-256)
```

Before calling `xcrun altool`, shipper copies the `.p8` key to
`~/.appstoreconnect/private_keys/` — the only path altool recognises.

## Build Steps

### Step 0: Auto-install Signing Credentials

Before the build starts, shipper verifies that a distribution certificate exists in
Keychain and a matching provisioning profile is installed. If either is missing, it
searches `~/.shipper/keys/<bundle_id>/` (then `./credentials/ios/`) and installs
automatically.

See [iOS Code Signing Setup](ios-code-signing.md) for the full credential layout.

### Step 1: Expo Prebuild (React Native/Expo only)

```bash
if [ -f "app.json" ] && grep -q "expo" app.json; then
    npx expo prebuild --platform ios --clean
fi
```

This generates the `ios/` directory with native Xcode project.

- Workspace name is derived from `expo.name` in `app.json` (e.g. `ios/MyApp.xcworkspace`)
- Xcode scheme name also comes from `expo.name` — not `expo.scheme` (which is the deep-link URI scheme, not the Xcode build scheme)

After prebuild, shipper rescans `ios/` to confirm the actual workspace and scheme names.

### Step 2: Install CocoaPods

```bash
cd ios && pod install --repo-update && cd ..
```

### Step 3: Archive

```bash
xcodebuild archive \
    -workspace ios/MyApp.xcworkspace \
    -scheme MyApp \
    -configuration Release \
    -archivePath build/shipper/MyApp.xcarchive \
    -destination "generic/platform=iOS" \
    CODE_SIGN_STYLE=Manual \
    DEVELOPMENT_TEAM=QC686RQ858 \
    PROVISIONING_PROFILE_SPECIFIER="MyApp AppStore" \
    CODE_SIGN_IDENTITY="iPhone Distribution: Company (TEAMID)"
```

`DEVELOPMENT_TEAM` is always passed — required for manual signing with Xcode 15+.

### Step 4: Export IPA

Shipper generates `ExportOptions.plist` from `shipper.toml`:

```xml
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>method</key>
    <string>app-store-connect</string>       <!-- Xcode 16+: was "app-store" -->
    <key>teamID</key>
    <string>QC686RQ858</string>
    <key>signingStyle</key>
    <string>manual</string>
    <key>signingCertificate</key>
    <string>Apple Distribution</string>
    <key>provisioningProfiles</key>
    <dict>
        <key>com.company.myapp</key>
        <string>MyApp AppStore</string>
    </dict>
    <key>destination</key>
    <string>export</string>                  <!-- "upload" would trigger ASC upload during export -->
</dict>
</plist>
```

```bash
xcodebuild -exportArchive \
    -archivePath build/shipper/MyApp.xcarchive \
    -exportPath build/shipper/ipa \
    -exportOptionsPlist /tmp/ExportOptions.plist
```

> **Note:** `destination: export` (not `upload`) is required to produce a local `.ipa` file.
> Using `upload` causes xcodebuild to attempt an App Store Connect connection during export,
> which produces confusing errors before the actual upload step.

### Step 5: Upload to App Store Connect

```bash
xcrun altool --upload-app \
    --type ios \
    --file build/shipper/ipa/MyApp.ipa \
    --apiKey W54D6Z8Y5M \
    --apiIssuer your-issuer-id
```

The `.p8` key must be in `~/.appstoreconnect/private_keys/` — shipper copies it there automatically if needed.

### Step 6: Poll Processing Status (requires `asc_app_id`)

> **Skipped on first submission** if `asc_app_id` is not set in `shipper.toml`.
> See the [Setup Guide](setup.md#app-store-connect-app-id) for how to obtain it after first upload.

```
GET https://api.appstoreconnect.apple.com/v1/builds
    ?filter[app]=6762051322
    &filter[version]=42          ← build number (CFBundleVersion), NOT marketing version
    &sort=-uploadedDate
    &limit=5

Headers:
    Authorization: Bearer <jwt>

Response:
{
  "data": [{
    "attributes": {
      "processingState": "PROCESSING" | "VALID" | "INVALID",
      "version": "42",
      "uploadedDate": "2025-01-01T00:00:00Z"
    }
  }]
}
```

Polls every 30 seconds until `processingState == "VALID"` (max 20 minutes).

> **Important:** `filter[version]` is the **build number** (e.g. `42`), not the marketing
> version (e.g. `1.0.0`). These are separate fields in Xcode / App Store Connect.

### Step 7: Notify

```
Telegram / Slack → "MyApp v1.0.1 (42) → TestFlight ✅"
```

## Error Recovery

| Error | Recovery |
|-------|----------|
| `requires a provisioning profile` | Place credential files in `~/.shipper/keys/<bundle_id>/` |
| `0 valid identities found` | Place `dist-cert.p12` + `credentials.json` in `~/.shipper/keys/<bundle_id>/` |
| `requires a development team` | Set `team_id` in `~/.shipper/config.toml → [credentials.apple]` |
| `App Store Connect Credentials Error` | Check that `destination: export` is used in ExportOptions |
| Upload 401 | JWT expired — regenerated automatically on next retry |
| Processing INVALID | Check App Store Connect for detailed error messages |
| Network timeout | Retry with exponential backoff |

## Config Reference

```toml
# shipper.toml
[ios]
workspace            = "ios/MyApp.xcworkspace"   # or: project = "ios/MyApp.xcodeproj"
scheme               = "MyApp"
bundle_id            = "com.company.app"
asc_app_id           = "1234567890"              # numeric ID from App Store Connect
export_method        = "app-store-connect"       # app-store-connect | ad-hoc | development
provisioning_profile = "MyApp AppStore"          # optional — auto-detected if omitted
code_sign_identity   = "Apple Distribution: Company (TEAMID)"  # optional — auto-detected
configuration        = "Release"
build_dir            = "build/shipper"           # default
```
