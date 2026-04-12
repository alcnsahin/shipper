# Shipper — Setup Guide

This document explains every value asked during `shipper init` and how to obtain it.

---

## `shipper init` — Field Reference

### Platform

The first question after the project name. Controls which sections are generated in `shipper.toml` and which credentials are written to `~/.shipper/config.toml`.

| Input | Effect |
|-------|--------|
| `ios` | Only iOS questions are asked. `[ios]` + `[credentials.apple]` generated. |
| `android` | Only Android questions are asked. `[android]` + `[credentials.google]` generated. |
| `all` or Enter | Both platforms configured. |

---

### Project name

Any display name for your project. Used in notification messages.

- Expo: auto-filled from `app.json → expo.name`
- Otherwise: defaults to the current directory name

---

### iOS

#### Workspace path

Path to your Xcode workspace file, relative to the project root.

- Expo / React Native: auto-detected by scanning the `ios/` directory for `.xcworkspace`
- Typical value: `ios/MyApp.xcworkspace`
- If you use a plain Xcode project (no CocoaPods): use `project = "ios/MyApp.xcodeproj"` in `shipper.toml` instead

#### Scheme

The Xcode build scheme to archive.

- Expo: auto-filled from `app.json → expo.scheme` (or `slug`, or `name`)
- Native: open Xcode → **Product → Scheme → Manage Schemes** — use the scheme marked as "Shared"

#### Bundle ID

The unique identifier for your iOS app (reverse-domain format).

- Expo: auto-filled from `app.json → expo.ios.bundleIdentifier`
- Native: Xcode → select your target → **Signing & Capabilities** → Bundle Identifier
- Must match exactly what is registered in App Store Connect

#### App Store Connect App ID

The numeric ID of your app in App Store Connect. This is **not** the Bundle ID.

**This field is optional.** Leave it empty if your app doesn't exist on App Store Connect yet.

**If the app already exists:**

1. Go to [App Store Connect](https://appstoreconnect.apple.com) → **Apps**
2. Select your app
3. The numeric ID is visible in the URL: `appstoreconnect.apple.com/apps/`**`6762051322`**`/...`
4. Also found under **App Information → Apple ID**

- Expo + EAS: auto-filled from `eas.json → submit.production.ios.ascAppId`

**If the app doesn't exist yet (first-time submission):**

1. Leave this field empty during `shipper init`
2. Run `shipper deploy ios` — the IPA will be uploaded but build status polling will be skipped
3. Create the app in App Store Connect: **Apps → +**
   - Use the same Bundle ID configured in `shipper.toml`
4. Copy the numeric App ID from the URL and add it to `shipper.toml`:

```toml
[ios]
asc_app_id = "6762051322"
```

From the next deploy onwards, shipper will poll App Store Connect and wait for the build to finish processing.

---

### Android

#### Android project dir

Directory containing the Android project (where `gradlew` lives).

- Expo / React Native: `android` (default)
- Native: typically the root of the Android project

#### Package name

The unique identifier for your Android app (reverse-domain format).

- Expo: auto-filled from `app.json → expo.android.package`
- Native: found in `android/app/build.gradle → applicationId`

#### Release track

The Google Play track to publish to.

| Track | Description |
|-------|-------------|
| `internal` | Internal testers only (up to 100 users). Publishes instantly. **Recommended for first uploads.** |
| `alpha` | Closed testing — invite specific users or groups |
| `beta` | Open testing — anyone can join |
| `production` | Public release on the Play Store |

- Expo + EAS: auto-filled from `eas.json → submit.production.android.track`
- Start with `internal` — your app must be approved for production before using other tracks

#### Build type

| Value | Output | When to use |
|-------|--------|-------------|
| `bundle` | `.aab` (Android App Bundle) | **Default.** Required for Play Store submissions |
| `apk` | `.apk` | Direct installs, internal distribution outside Play Store |

- Expo + EAS: auto-filled from `eas.json → build.production.android.buildType`

#### Keystore path

Path to your `.keystore` file used to sign the release build.

**If you already have one** (e.g. from a previous EAS or Fastlane setup):

```bash
# Copy it to shipper's key directory
cp /path/to/your/release.keystore ~/.shipper/keys/release.keystore
chmod 600 ~/.shipper/keys/release.keystore
```

**If you need to create one:**

```bash
keytool -genkey -v \
  -keystore ~/.shipper/keys/release.keystore \
  -alias release \
  -keyalg RSA -keysize 2048 \
  -validity 10000
```

> **Warning:** Keep your keystore safe. If you lose it, you cannot update your app on the Play Store.
> Back it up to a secure location (password manager, encrypted storage).

- Native: may be auto-detected from `android/app/build.gradle → signingConfigs.release.storeFile`

#### Keystore alias

The alias of the key inside the keystore.

- Native: auto-detected from `android/app/build.gradle → signingConfigs.release.keyAlias`
- If you created the keystore with the command above: `release`
- To list aliases in an existing keystore:

```bash
keytool -list -keystore ~/.shipper/keys/release.keystore
```

#### Keystore password

Not asked during `init` — stored in a separate file after setup:

```bash
echo "your-keystore-password" > ~/.shipper/keys/keystore-password
chmod 600 ~/.shipper/keys/keystore-password
```

Then set in `shipper.toml`:
```toml
keystore_password_path = "~/.shipper/keys/keystore-password"
```

---

## `~/.shipper/config.toml` — Credential Reference

These are filled in after `shipper init` creates the config file.

### Apple credentials

#### `team_id`

Your Apple Developer Team ID.

1. Go to [developer.apple.com](https://developer.apple.com) → **Account**
2. Scroll to **Membership details**
3. Copy the **Team ID** (e.g. `QC686RQ858`)

- Expo + EAS: auto-filled from `eas.json → submit.production.ios.appleTeamId`

#### `key_id`, `issuer_id`, `key_path` (.p8 file)

These three come from the same place — App Store Connect API keys.

1. Go to [App Store Connect](https://appstoreconnect.apple.com) → **Users and Access** → **Integrations** → **App Store Connect API**
2. Click **+** to generate a new key
   - Role: **Developer** (sufficient for uploads)
3. Note the **Key ID** (e.g. `W54D6Z8Y5M`) and **Issuer ID** shown on the page
4. Download `AuthKey_W54D6Z8Y5M.p8` — **this is the only time you can download it**
5. Move it to the shipper keys directory:

```bash
mv ~/Downloads/AuthKey_W54D6Z8Y5M.p8 ~/.shipper/keys/
chmod 600 ~/.shipper/keys/AuthKey_W54D6Z8Y5M.p8
```

6. Update `~/.shipper/config.toml`:

```toml
[credentials.apple]
team_id   = "QC686RQ858"
key_id    = "W54D6Z8Y5M"
issuer_id = "xxxxxxxx-xxxx-xxxx-xxxx-xxxxxxxxxxxx"
key_path  = "~/.shipper/keys/AuthKey_W54D6Z8Y5M.p8"
```

---

### Google credentials

#### `service_account` (JSON key file)

A Google service account with access to the Google Play Developer API.

1. Go to [Google Play Console](https://play.google.com/console) → **Setup** → **API access**
2. Click **Link to a Google Cloud project** (create one if needed)
3. Under **Service accounts**, click **Create new service account**
4. In the Google Cloud Console that opens:
   - Click **+ Create Service Account**
   - Name it (e.g. `shipper-deploy`)
   - Grant the role **Service Account User**
   - Click **Done**
5. Back in Play Console → **Grant access** next to the new service account
   - Set permissions to **Release Manager** (or **Admin** for full access)
6. In Google Cloud Console → select the service account → **Keys** → **Add key** → **Create new key** → **JSON**
7. Download the JSON file and move it:

```bash
mv ~/Downloads/service-account-key.json ~/.shipper/keys/play-store-sa.json
chmod 600 ~/.shipper/keys/play-store-sa.json
```

8. Update `~/.shipper/config.toml`:

```toml
[credentials.google]
service_account = "~/.shipper/keys/play-store-sa.json"
```

- Expo + EAS: the path may be auto-filled from `eas.json → submit.production.android.serviceAccountKeyPath`

---

### Telegram notifications (optional)

#### `bot_token_path`

1. Open Telegram and message [@BotFather](https://t.me/BotFather)
2. Send `/newbot` and follow the prompts
3. Copy the bot token (e.g. `7123456789:AAF...`)
4. Save it:

```bash
echo "7123456789:AAF..." > ~/.shipper/keys/telegram-bot-token
chmod 600 ~/.shipper/keys/telegram-bot-token
```

#### `chat_id`

The Telegram chat or channel ID where notifications are sent.

- **Personal chat:** message [@userinfobot](https://t.me/userinfobot) — it replies with your chat ID
- **Group/channel:** add [@RawDataBot](https://t.me/RawDataBot) to the group, it will post the chat ID (negative number starting with `-100`)

Update `~/.shipper/config.toml`:

```toml
[notifications.telegram]
bot_token_path = "~/.shipper/keys/telegram-bot-token"
chat_id        = "-100xxxxxxxxxx"
```

And enable it in the global section:

```toml
[global]
notify = ["telegram"]
```
