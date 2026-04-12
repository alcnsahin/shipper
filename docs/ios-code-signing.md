# Shipper — iOS Code Signing Setup

This document covers how to set up iOS distribution credentials so that
`shipper deploy ios` can archive and upload your app to App Store Connect.

---

## How shipper handles signing

Before every `shipper deploy ios` run, shipper automatically:

1. **Checks** whether a valid distribution certificate exists in your Keychain
2. **Checks** whether a matching provisioning profile is installed on disk
3. **If either is missing**, searches for credential files in:
   - `~/.shipper/keys/<bundle_id>/` (preferred persistent location)
   - `./credentials/ios/` (EAS CLI download location)
4. **Installs** whatever it finds — certificate into Keychain, profile into the Xcode profiles directory
5. **Copies** any files found in `./credentials/ios/` to `~/.shipper/keys/<bundle_id>/` so they work for future runs

No separate setup command is needed. Just place the files in the right location and run `shipper deploy ios`.

Once detected, shipper prints what it found:
```
  ✓ Distribution certificate installed
  ✓ Provisioning profile installed
  i Provisioning profile: CyberChan AppStore
  i Code sign identity:   iPhone Distribution: STELIKON OU (QC686RQ858)
```

---

## What is needed

To archive and upload an iOS app, two credential files must be available:

| File | What it is |
|---|---|
| `dist-cert.p12` | Distribution certificate + private key |
| `profile.mobileprovision` | Provisioning profile for your bundle ID |
| `credentials.json` | Contains the p12 password (`certPassword` field) |

---

## Setup via EAS CLI (recommended)

If your project uses Expo / EAS, this is the fastest path.

### Step 1 — Download credentials from EAS

```bash
eas credentials --platform ios
```

1. Select build profile: **production**
2. Log in to your Apple account when prompted
3. Choose: **credentials.json: Upload/Download credentials between EAS servers and your local json**
4. Choose: **Download credentials from EAS to credentials.json**

This creates files in your project:
```
credentials/
  ios/
    dist-cert.p12
    profile.mobileprovision
    credentials.json        ← contains the p12 password under "certPassword"
```

### Step 2 — Move credentials to shipper's key directory

Shipper reads credentials from `~/.shipper/keys/<bundle_id>/`. Move the files there
so they persist across projects and are never accidentally committed:

```bash
BUNDLE_ID="com.company.myapp"   # your actual bundle ID

mkdir -p ~/.shipper/keys/$BUNDLE_ID
mv credentials/ios/dist-cert.p12          ~/.shipper/keys/$BUNDLE_ID/
mv credentials/ios/profile.mobileprovision ~/.shipper/keys/$BUNDLE_ID/
mv credentials/ios/credentials.json        ~/.shipper/keys/$BUNDLE_ID/
chmod 600 ~/.shipper/keys/$BUNDLE_ID/*

# Remove the now-empty directory from your project
rmdir credentials/ios credentials 2>/dev/null || true
```

> **Important:** Do not keep `credentials/` in your project directory — it contains private keys.
> Add `credentials/` to `.gitignore` as a safety net.

### Step 3 — Deploy

```bash
shipper deploy ios
```

Shipper detects that the certificate and profile are not yet installed, finds the files in
`~/.shipper/keys/<bundle_id>/`, reads the password from `credentials.json`, and installs
everything automatically before proceeding with the build.

---

## Setup via Apple Developer Portal (manual)

Use this if you don't have EAS or prefer full manual control.

### Distribution Certificate

1. Go to [developer.apple.com](https://developer.apple.com) → Certificates, IDs & Profiles
2. Click **+** → **Apple Distribution**
3. Follow the CSR steps (Keychain Access → Certificate Assistant → Request a Certificate)
4. Download the `.cer` file and double-click to install in Keychain

Export as `.p12` for use across machines:

```
Keychain Access → My Certificates → right-click "Apple Distribution: ..." → Export
```

Save the exported file as `~/.shipper/keys/<bundle_id>/dist-cert.p12` and record the
export password in `~/.shipper/keys/<bundle_id>/credentials.json`:

```json
{ "certPassword": "your-export-password" }
```

### Provisioning Profile

1. Go to [developer.apple.com](https://developer.apple.com) → Profiles
2. Click **+** → **App Store Connect**
3. Select your App ID (bundle ID)
4. Select the Distribution certificate
5. Download the `.mobileprovision` file

Save it as `~/.shipper/keys/<bundle_id>/profile.mobileprovision`.

### Step 3 — Deploy

```bash
shipper deploy ios
```

Same as above — shipper installs everything automatically.

---

## Skip auto-detection (optional)

If you want to skip the auto-detection step on every run, add explicit values to `shipper.toml`:

```toml
[ios]
provisioning_profile = "CyberChan AppStore"
code_sign_identity   = "iPhone Distribution: STELIKON OU (QC686RQ858)"
```

Shipper will use these directly without scanning the filesystem.

---

## Credential file layout

```
~/.shipper/
└── keys/
    └── <bundle_id>/
        ├── dist-cert.p12          ← Distribution certificate + private key
        ├── profile.mobileprovision
        └── credentials.json       ← { "certPassword": "..." }
```

---

## Provisioning profile install location

Shipper installs profiles to the Xcode 15+ location:

```
~/Library/Developer/Xcode/UserData/Provisioning Profiles/<UUID>.mobileprovision
```

The UUID is read from the profile's embedded plist.

---

## Troubleshooting

| Error | Cause | Fix |
|---|---|---|
| `requires a provisioning profile` | No profile found in either search location | Place `profile.mobileprovision` in `~/.shipper/keys/<bundle_id>/` |
| `0 valid identities found` | Distribution cert not in Keychain and no `.p12` found | Place `dist-cert.p12` + `credentials.json` in `~/.shipper/keys/<bundle_id>/` |
| `Failed to import certificate` | Wrong p12 password | Check `certPassword` in `credentials.json` |
| `xcodebuild archive failed` | Profile / cert mismatch | Ensure profile uses the same cert that is in Keychain |
| `Certificate has been revoked` | Old cert revoked on Apple portal | Re-download via `eas credentials` or Developer Portal |

---

## Security notes

- `dist-cert.p12` contains your private key — treat it like a password
- Never commit `credentials/` or `~/.shipper/keys/` to version control
- Set strict permissions on all credential files:

```bash
chmod 600 ~/.shipper/keys/<bundle_id>/dist-cert.p12
chmod 600 ~/.shipper/keys/<bundle_id>/credentials.json
chmod 600 ~/.shipper/keys/<bundle_id>/profile.mobileprovision
```
