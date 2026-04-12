# Shipper — iOS Code Signing Setup

This document covers how to set up iOS distribution credentials so that
`shipper deploy ios` can archive and upload your app to App Store Connect.

---

## Prerequisites

- macOS with Xcode installed
- Apple Developer account with an active membership
- EAS CLI installed (`npm install -g eas-cli`) — optional but easiest path

---

## What is needed

To archive and upload an iOS app, two things must be present on your Mac:

| Credential | Where it lives | What it is |
|---|---|---|
| **Distribution Certificate** | macOS Keychain | Proves you are an authorized Apple developer |
| **Provisioning Profile** | `~/Library/MobileDevice/Provisioning Profiles/` | Authorizes your app (bundle ID) for App Store distribution |

Both must be installed locally. EAS cloud credentials are not enough — `xcodebuild` reads from Keychain and the local profiles directory.

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

This creates two files in your project:
```
credentials/
  ios/
    dist-cert.p12       ← Distribution certificate + private key
    profile.mobileprovision
    credentials.json    ← Contains the p12 password
```

> **Important:** Add `credentials/` to `.gitignore`. It contains private keys.

### Step 2 — Install the distribution certificate

```bash
# Get the p12 password
cat credentials/ios/credentials.json | grep -i password

# Open the p12 (Keychain Access will prompt for the password)
open credentials/ios/dist-cert.p12
```

When Keychain Access opens, enter the password from `credentials.json` and click **Add**.

Verify it was installed:
```bash
security find-identity -v -p codesigning
# Should show: "iPhone Distribution: Your Company (TEAMID)"
```

### Step 3 — Install the provisioning profile

```bash
open credentials/ios/profile.mobileprovision
```

Xcode registers the profile automatically.

### Step 4 — Deploy

```bash
shipper deploy ios
```

Shipper will automatically detect the certificate and provisioning profile — no manual configuration needed.

---

## Setup via Apple Developer Portal (manual)

Use this if you don't have EAS or prefer full manual control.

### Distribution Certificate

1. Go to [developer.apple.com](https://developer.apple.com) → Certificates, IDs & Profiles
2. Click **+** → **Apple Distribution**
3. Follow the CSR steps (Keychain Access → Certificate Assistant → Request a Certificate)
4. Download the `.cer` file and double-click to install in Keychain

### Provisioning Profile

1. Go to [developer.apple.com](https://developer.apple.com) → Profiles
2. Click **+** → **App Store Connect**
3. Select your App ID (bundle ID)
4. Select the Distribution certificate you just created
5. Download the `.mobileprovision` file and double-click to install

### shipper.toml configuration

After manual setup, you can explicitly set the values in `shipper.toml` to skip auto-detection:

```toml
[ios]
provisioning_profile = "CyberChan AppStore"
code_sign_identity = "iPhone Distribution: STELIKON OU (QC686RQ858)"
```

---

## How shipper handles signing

On every `shipper deploy ios` run, if `provisioning_profile` or
`code_sign_identity` are not set in `shipper.toml`, shipper will:

1. **Scan** `~/Library/MobileDevice/Provisioning Profiles/` for a distribution
   profile matching your bundle ID
2. **Check** the Keychain via `security find-identity -v -p codesigning` for
   an `Apple Distribution` or `iPhone Distribution` identity
3. **Prompt** you interactively if nothing is found

Once detected, shipper prints what it found:
```
  i Provisioning profile detected: CyberChan AppStore
  i Code sign identity detected: iPhone Distribution: STELIKON OU (QC686RQ858)
```

To avoid the detection step on every run, add these to `shipper.toml`:

```toml
[ios]
provisioning_profile = "CyberChan AppStore"
code_sign_identity = "iPhone Distribution: STELIKON OU (QC686RQ858)"
```

---

## Troubleshooting

| Error | Cause | Fix |
|---|---|---|
| `requires a provisioning profile` | No profile set and none detected | Follow setup steps above |
| `0 valid identities found` | Distribution cert not in Keychain | Install the `.p12` file |
| `xcodebuild archive failed` | Profile / cert mismatch | Ensure profile uses the same cert that is in Keychain |
| `Certificate has been revoked` | Old cert revoked on Apple portal | Generate a new cert via `eas credentials` or Developer Portal |

---

## Security notes

- `credentials/ios/dist-cert.p12` contains your private key — treat it like a password
- Add `credentials/` to `.gitignore` immediately after downloading
- `chmod 600` the files if storing long-term:

```bash
chmod 600 credentials/ios/dist-cert.p12
chmod 600 credentials/ios/credentials.json
```
