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
4. **If still not found**, runs `eas credentials --platform ios` automatically so you can authenticate and download credentials interactively
5. **Copies** any downloaded files to `~/.shipper/keys/<bundle_id>/` so future runs skip the EAS step entirely
6. **Installs** the credentials — certificate into Keychain, profile into the Xcode profiles directory

No separate setup command is needed. On first deploy, shipper will launch EAS if credentials are missing.

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

If your project uses Expo / EAS, no manual setup is needed. Shipper handles everything automatically on first deploy.

### Just run deploy

```bash
shipper deploy ios
```

If credentials are missing, shipper launches `eas credentials --platform ios` interactively:

```
  i Signing credentials not found — launching EAS to download them...
```

When the EAS prompt appears, navigate the menus as follows:

```
? Select a build profile  →  production

? What do you want to do?
  → Manage your iOS credentials
    (logs in to Apple if needed)

? What do you want to do?
  → credentials.json: Upload/Download credentials between EAS servers and your local json

? What do you want to do?
  → Download credentials from EAS to credentials.json
```

EAS downloads the files to `./credentials/ios/`. Shipper then **moves** them to
`~/.shipper/keys/<bundle_id>/` (the originals in `./credentials/ios/` are deleted) and
installs them automatically:

```
  ✓ Distribution certificate installed
  ✓ Provisioning profile installed
```

Subsequent deploys are fully automatic — no EAS prompt, credentials come from `~/.shipper/keys/<bundle_id>/`.

### Manual download (alternative)

If you prefer to download credentials separately before deploying:

```bash
eas credentials --platform ios
# Follow the same menu steps above
# → downloads to ./credentials/ios/
```

Then run `shipper deploy ios` — it picks them up from `./credentials/ios/`, moves them to
`~/.shipper/keys/<bundle_id>/`, and deletes the originals from `./credentials/ios/`.

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
| `eas credentials failed` | EAS CLI not installed or auth failed | Run `npm i -g eas-cli` then `eas login` |
| `not found after eas credentials` | EAS didn't download expected files | Check EAS project config; run `eas credentials` manually and verify `credentials/ios/` |
| `Failed to import certificate` | Wrong p12 password | Check `certPassword` in `credentials.json` |
| `xcodebuild archive failed` | Profile / cert mismatch | Ensure profile uses the same cert that is in Keychain |
| `Certificate has been revoked` | Old cert revoked on Apple portal | Delete `~/.shipper/keys/<bundle_id>/dist-cert.p12` and redeploy (triggers EAS re-download) |

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
