use anyhow::{Context, Result};
use chrono::Utc;
use jsonwebtoken::{encode, Algorithm, EncodingKey, Header};
use serde::{Deserialize, Serialize};
use std::time::Duration;

use crate::config::{AppleCredentials, expand_path};

const ASC_BASE: &str = "https://api.appstoreconnect.apple.com/v1";

// ─── JWT ─────────────────────────────────────────────────────────────────────

#[derive(Debug, Serialize)]
struct AscClaims {
    iss: String,
    iat: i64,
    exp: i64,
    aud: String,
}

pub fn generate_jwt(creds: &AppleCredentials) -> Result<String> {
    let key_path = expand_path(&creds.key_path);
    let pem = std::fs::read(&key_path)
        .with_context(|| format!("Failed to read App Store key: {}", key_path.display()))?;

    let now = Utc::now().timestamp();
    let claims = AscClaims {
        iss: creds.issuer_id.clone(),
        iat: now,
        exp: now + 1200, // 20 minutes max
        aud: "appstoreconnect-v1".to_string(),
    };

    let mut header = Header::new(Algorithm::ES256);
    header.kid = Some(creds.key_id.clone());

    let key = EncodingKey::from_ec_pem(&pem)
        .context("Failed to load EC key from .p8 file")?;

    encode(&header, &claims, &key).context("Failed to generate JWT")
}

fn asc_client(jwt: &str) -> reqwest::Client {
    let mut headers = reqwest::header::HeaderMap::new();
    headers.insert(
        reqwest::header::AUTHORIZATION,
        format!("Bearer {}", jwt).parse().unwrap(),
    );
    reqwest::Client::builder()
        .default_headers(headers)
        .timeout(Duration::from_secs(30))
        .build()
        .unwrap()
}

// ─── Build polling ────────────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
struct BuildsResponse {
    data: Vec<BuildData>,
}

#[derive(Debug, Deserialize)]
struct BuildData {
    id: String,
    attributes: BuildAttributes,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct BuildAttributes {
    processing_state: String,
    version: String,
    uploaded_date: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ProcessingState {
    Processing,
    Valid,
    Invalid(String),
    Unknown(String),
}

pub async fn poll_build_processing(
    creds: &AppleCredentials,
    app_id: &str,
    version: &str,
    build_number: &str,
) -> Result<String> {
    use crate::utils::progress;

    let spinner = progress::spinner("Waiting for App Store Connect to process build...");
    let max_attempts = 40; // ~20 minutes
    let poll_interval = Duration::from_secs(30);

    for attempt in 0..max_attempts {
        let jwt = generate_jwt(creds)?;
        let client = asc_client(&jwt);

        let url = format!(
            "{}/builds?filter[app]={}&filter[version]={}&sort=-uploadedDate&limit=5",
            ASC_BASE, app_id, version
        );

        let res = client.get(&url).send().await?;
        if !res.status().is_success() {
            let status = res.status().as_u16();
            let body = res.text().await.unwrap_or_default();
            anyhow::bail!("App Store Connect API error ({}): {}", status, body);
        }

        let builds: BuildsResponse = res.json().await?;

        // Find our specific build by build number
        if let Some(build) = builds.data.iter().find(|b| {
            // Build number is in a separate field; check version match
            b.attributes.version == version
        }) {
            let state = match build.attributes.processing_state.as_str() {
                "PROCESSING" | "RECEIVE" => ProcessingState::Processing,
                "VALID" => ProcessingState::Valid,
                "INVALID" => ProcessingState::Invalid(build.id.clone()),
                other => ProcessingState::Unknown(other.to_string()),
            };

            match state {
                ProcessingState::Valid => {
                    spinner.finish_with_message(format!(
                        "Build {} processed successfully",
                        build_number
                    ));
                    return Ok(build.id.clone());
                }
                ProcessingState::Invalid(ref id) => {
                    spinner.finish_with_message("Build marked INVALID by App Store Connect");
                    anyhow::bail!(
                        "Build {} was marked INVALID by App Store Connect. \
                        Check App Store Connect for error details.",
                        id
                    );
                }
                ProcessingState::Processing => {
                    spinner.set_message(format!(
                        "Processing... (attempt {}/{})",
                        attempt + 1,
                        max_attempts
                    ));
                }
                ProcessingState::Unknown(ref s) => {
                    tracing::debug!("Unknown processing state: {}", s);
                }
            }
        } else {
            spinner.set_message(format!(
                "Waiting for build to appear... (attempt {}/{})",
                attempt + 1,
                max_attempts
            ));
        }

        tokio::time::sleep(poll_interval).await;
    }

    spinner.finish_with_message("Timed out waiting for build processing");
    anyhow::bail!("Timed out after 20 minutes waiting for build to be processed")
}

// ─── TestFlight beta submission ───────────────────────────────────────────────

pub async fn submit_to_testflight(
    creds: &AppleCredentials,
    build_id: &str,
) -> Result<()> {
    let jwt = generate_jwt(creds)?;
    let client = asc_client(&jwt);

    let url = format!("{}/betaAppReviewSubmissions", ASC_BASE);
    let body = serde_json::json!({
        "data": {
            "type": "betaAppReviewSubmissions",
            "relationships": {
                "build": {
                    "data": { "type": "builds", "id": build_id }
                }
            }
        }
    });

    let res = client.post(&url).json(&body).send().await?;
    let status = res.status();

    if status.as_u16() == 409 {
        // Already submitted — that's fine
        tracing::debug!("Build already submitted to TestFlight");
        return Ok(());
    }

    if !status.is_success() {
        let body = res.text().await.unwrap_or_default();
        anyhow::bail!("TestFlight submission failed ({}): {}", status, body);
    }

    Ok(())
}
