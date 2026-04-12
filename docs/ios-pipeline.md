# Shipper — iOS Deployment Pipeline

## Overview

The iOS pipeline handles the complete flow from source code to TestFlight/App Store, running entirely on the developer's local Mac.

## Prerequisites

- **macOS** with Xcode installed (`xcode-select --install`)
- **Apple Developer Account** with App Store Connect API key (.p8)
- **Provisioning Profile** + Distribution Certificate installed

## Apple Credentials Setup

### 1. App Store Connect API Key

Generate at: https://appstoreconnect.apple.com/access/integrations/api

```
Key ID:     W54D6Z8Y5M
Issuer ID:  your-issuer-id (shown on the API keys page)
Key File:   AuthKey_W54D6Z8Y5M.p8 (downloaded once, save securely)
```

Store at `~/.shipper/keys/AuthKey_W54D6Z8Y5M.p8`

### 2. JWT Token Generation

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

### 3. Provisioning Profile

Option A: Download from Apple Developer Portal manually
Option B: Use App Store Connect API to manage programmatically

## Build Steps

### Step 1: Expo Prebuild (React Native/Expo only)

```bash
# Detect if expo project
if [ -f "app.json" ] && grep -q "expo" app.json; then
    npx expo prebuild --platform ios --clean
fi
```

This generates the `ios/` directory with native Xcode project.

### Step 2: Install CocoaPods

```bash
cd ios && pod install --repo-update && cd ..
```

### Step 3: Archive

```bash
xcodebuild archive \
    -workspace ios/CyberChan.xcworkspace \
    -scheme CyberChan \
    -configuration Release \
    -archivePath build/CyberChan.xcarchive \
    -destination "generic/platform=iOS" \
    CODE_SIGN_STYLE=Manual \
    PROVISIONING_PROFILE_SPECIFIER="your-profile-name" \
    CODE_SIGN_IDENTITY="Apple Distribution: STELIKON OU (QC686RQ858)"
```

### Step 4: Export IPA

Create `ExportOptions.plist`:
```xml
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "...">
<plist version="1.0">
<dict>
    <key>method</key>
    <string>app-store</string>
    <key>teamID</key>
    <string>QC686RQ858</string>
    <key>signingCertificate</key>
    <string>Apple Distribution</string>
    <key>provisioningProfiles</key>
    <dict>
        <key>app.cyberchan.mobile</key>
        <string>CyberChan AppStore</string>
    </dict>
    <key>destination</key>
    <string>upload</string>
</dict>
</plist>
```

```bash
xcodebuild -exportArchive \
    -archivePath build/CyberChan.xcarchive \
    -exportPath build/ipa \
    -exportOptionsPlist ExportOptions.plist
```

### Step 5: Upload to App Store Connect

**Option A: xcrun altool (legacy, simpler)**
```bash
xcrun altool --upload-app \
    --type ios \
    --file build/ipa/CyberChan.ipa \
    --apiKey W54D6Z8Y5M \
    --apiIssuer your-issuer-id
```

**Option B: App Store Connect API (modern, more control)**
```
POST https://is1-ssl.mzstatic.com/itms/api/v1
# Transporter protocol — chunked upload
```

**Option C: xcrun notarytool + iTMSTransporter**
```bash
# Apple's official upload tool
xcrun iTMSTransporter -m upload \
    -assetFile build/ipa/CyberChan.ipa \
    -apiKey ~/.shipper/keys/AuthKey_W54D6Z8Y5M.p8 \
    -apiIssuer your-issuer-id
```

### Step 6: Poll Processing Status

```
GET https://api.appstoreconnect.apple.com/v1/builds
    ?filter[app]=6762051322
    &filter[version]=1.0.1
    &sort=-uploadedDate
    &limit=1

Headers:
    Authorization: Bearer <jwt>

Response:
{
  "data": [{
    "attributes": {
      "processingState": "PROCESSING" | "VALID" | "INVALID",
      "version": "1.0.1",
      "buildNumber": "42"
    }
  }]
}
```

Poll every 30 seconds until `processingState == "VALID"`.

### Step 7: Auto-submit to TestFlight (optional)

```
# Create beta group submission
POST https://api.appstoreconnect.apple.com/v1/betaAppReviewSubmissions
{
  "data": {
    "type": "betaAppReviewSubmissions",
    "relationships": {
      "build": {
        "data": { "type": "builds", "id": "<build-id>" }
      }
    }
  }
}
```

## Error Recovery

| Error | Recovery |
|-------|----------|
| `xcodebuild` timeout | Retry with `-retry-tests-on-failure` |
| Code signing failed | Check `security find-identity -v -p codesigning` |
| Upload 401 | Regenerate JWT (expired after 20min) |
| Processing INVALID | Check App Store Connect for error details |
| Network timeout | Retry with exponential backoff |

## Config Reference

```toml
# shipper.toml
[ios]
workspace = "ios/MyApp.xcworkspace"   # or: project = "ios/MyApp.xcodeproj"
scheme = "MyApp"
bundle_id = "com.company.app"
asc_app_id = "1234567890"             # numeric ID from App Store Connect
export_method = "app-store"           # app-store | ad-hoc | development
provisioning_profile = "MyApp AppStore"   # optional, manual signing
code_sign_identity = "Apple Distribution: Company (TEAMID)"  # optional
configuration = "Release"
build_dir = "build/shipper"           # default
```
