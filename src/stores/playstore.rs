use anyhow::{Context, Result};
use chrono::Utc;
use jsonwebtoken::{encode, Algorithm, EncodingKey, Header};
use serde::{Deserialize, Serialize};
use std::path::Path;
use std::time::Duration;

use crate::config::{expand_path, GoogleCredentials};

const PLAY_BASE: &str = "https://androidpublisher.googleapis.com/androidpublisher/v3/applications";

// ─── Service Account JWT → OAuth2 token ──────────────────────────────────────

#[derive(Debug, Deserialize)]
struct ServiceAccount {
    private_key: String,
    client_email: String,
    token_uri: String,
}

#[derive(Debug, Serialize)]
struct GoogleClaims {
    iss: String,
    scope: String,
    aud: String,
    iat: i64,
    exp: i64,
}

#[derive(Debug, Deserialize)]
struct TokenResponse {
    access_token: String,
}

async fn get_access_token(creds: &GoogleCredentials) -> Result<String> {
    let sa_path = expand_path(&creds.service_account);
    let sa_json = std::fs::read_to_string(&sa_path)
        .with_context(|| format!("Failed to read service account: {}", sa_path.display()))?;
    let sa: ServiceAccount =
        serde_json::from_str(&sa_json).context("Failed to parse service account JSON")?;

    let now = Utc::now().timestamp();
    let claims = GoogleClaims {
        iss: sa.client_email.clone(),
        scope: "https://www.googleapis.com/auth/androidpublisher".to_string(),
        aud: sa.token_uri.clone(),
        iat: now,
        exp: now + 3600,
    };

    let mut header = Header::new(Algorithm::RS256);
    header.typ = Some("JWT".to_string());

    let key = EncodingKey::from_rsa_pem(sa.private_key.as_bytes())
        .context("Failed to load RSA private key from service account")?;

    let jwt = encode(&header, &claims, &key)?;

    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(15))
        .build()?;

    let res = client
        .post(&sa.token_uri)
        .form(&[
            ("grant_type", "urn:ietf:params:oauth:grant-type:jwt-bearer"),
            ("assertion", &jwt),
        ])
        .send()
        .await?;

    if !res.status().is_success() {
        let body = res.text().await.unwrap_or_default();
        anyhow::bail!("Failed to get Google access token: {}", body);
    }

    let token: TokenResponse = res.json().await?;
    Ok(token.access_token)
}

fn play_client(token: &str) -> reqwest::Client {
    let mut headers = reqwest::header::HeaderMap::new();
    headers.insert(
        reqwest::header::AUTHORIZATION,
        format!("Bearer {}", token).parse().unwrap(),
    );
    reqwest::Client::builder()
        .default_headers(headers)
        .timeout(Duration::from_secs(300)) // uploads can be slow
        .build()
        .unwrap()
}

// ─── Edit lifecycle ───────────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
struct EditResponse {
    id: String,
}

async fn create_edit(client: &reqwest::Client, package: &str) -> Result<String> {
    let url = format!("{}/{}/edits", PLAY_BASE, package);
    let res = client.post(&url).json(&serde_json::json!({})).send().await?;

    if !res.status().is_success() {
        let body = res.text().await.unwrap_or_default();
        anyhow::bail!("Failed to create Play Store edit: {}", body);
    }

    let edit: EditResponse = res.json().await?;
    Ok(edit.id)
}

async fn commit_edit(client: &reqwest::Client, package: &str, edit_id: &str) -> Result<()> {
    let url = format!("{}/{}/edits/{}:commit", PLAY_BASE, package, edit_id);
    let res = client.post(&url).send().await?;

    if !res.status().is_success() {
        let body = res.text().await.unwrap_or_default();
        anyhow::bail!("Failed to commit Play Store edit: {}", body);
    }

    Ok(())
}

// ─── Bundle upload ────────────────────────────────────────────────────────────

async fn upload_bundle(
    client: &reqwest::Client,
    package: &str,
    edit_id: &str,
    aab_path: &Path,
) -> Result<u32> {
    let file_size = std::fs::metadata(aab_path)?.len();
    let file_bytes = std::fs::read(aab_path)
        .with_context(|| format!("Failed to read AAB: {}", aab_path.display()))?;

    let url = format!(
        "https://androidpublisher.googleapis.com/upload/androidpublisher/v3/applications/{}/edits/{}/bundles?uploadType=media",
        package, edit_id
    );

    let res = client
        .post(&url)
        .header(reqwest::header::CONTENT_TYPE, "application/octet-stream")
        .header(reqwest::header::CONTENT_LENGTH, file_size)
        .body(file_bytes)
        .send()
        .await?;

    if !res.status().is_success() {
        let body = res.text().await.unwrap_or_default();
        anyhow::bail!("Bundle upload failed: {}", body);
    }

    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct BundleResponse {
        version_code: u32,
    }

    let bundle: BundleResponse = res.json().await?;
    Ok(bundle.version_code)
}

// ─── Track assignment ─────────────────────────────────────────────────────────

async fn assign_to_track(
    client: &reqwest::Client,
    package: &str,
    edit_id: &str,
    track: &str,
    version_code: u32,
) -> Result<()> {
    let url = format!("{}/{}/edits/{}/tracks/{}", PLAY_BASE, package, edit_id, track);

    let body = serde_json::json!({
        "track": track,
        "releases": [{
            "status": "completed",
            "versionCodes": [version_code]
        }]
    });

    let res = client.put(&url).json(&body).send().await?;

    if !res.status().is_success() {
        let body = res.text().await.unwrap_or_default();
        anyhow::bail!("Failed to assign bundle to track '{}': {}", track, body);
    }

    Ok(())
}

// ─── Public API ───────────────────────────────────────────────────────────────

pub async fn upload_aab(
    google_creds: &GoogleCredentials,
    package_name: &str,
    track: &str,
    aab_path: &Path,
) -> Result<u32> {
    let token = get_access_token(google_creds).await?;
    let client = play_client(&token);

    let edit_id = create_edit(&client, package_name).await?;
    tracing::debug!("Created Play Store edit: {}", edit_id);

    let version_code = upload_bundle(&client, package_name, &edit_id, aab_path).await?;
    tracing::debug!("Uploaded bundle, version code: {}", version_code);

    assign_to_track(&client, package_name, &edit_id, track, version_code).await?;
    tracing::debug!("Assigned to track: {}", track);

    commit_edit(&client, package_name, &edit_id).await?;
    tracing::debug!("Committed edit");

    Ok(version_code)
}
