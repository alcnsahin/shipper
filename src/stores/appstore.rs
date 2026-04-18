use anyhow::{Context, Result};
use chrono::Utc;
use jsonwebtoken::{encode, Algorithm, EncodingKey, Header};
use serde::{Deserialize, Serialize};
use std::time::Duration;

use crate::config::{expand_path, AppleCredentials};
use crate::stores::http::{map_status_to_error, send_with_retry};

const ASC_BASE: &str = "https://api.appstoreconnect.apple.com/v1";

/// Successful build processing result with diagnostics.
#[derive(Debug, Clone)]
pub(crate) struct ProcessedBuild {
    pub id: String,
    pub version: String,
    pub uploaded_date: Option<String>,
}

// ─── JWT ─────────────────────────────────────────────────────────────────────

#[derive(Debug, Serialize)]
struct AscClaims {
    iss: String,
    iat: i64,
    exp: i64,
    aud: String,
}

fn generate_jwt(creds: &AppleCredentials) -> Result<String> {
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

    let key = EncodingKey::from_ec_pem(&pem).context("Failed to load EC key from .p8 file")?;

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
enum ProcessingState {
    Processing,
    Valid,
    Invalid(String),
    Unknown(String),
}

pub(crate) async fn poll_build_processing(
    creds: &AppleCredentials,
    app_id: &str,
    build_number: &str,
) -> Result<ProcessedBuild> {
    use crate::utils::progress;

    let spinner = progress::spinner("Waiting for App Store Connect to process build...");
    let max_attempts = 40; // ~20 minutes
    let poll_interval = Duration::from_secs(30);

    for attempt in 0..max_attempts {
        let jwt = generate_jwt(creds)?;
        let client = asc_client(&jwt);

        // ASC API: filter[version] is the build number (CFBundleVersion), not the marketing version
        let url = format!(
            "{}/builds?filter[app]={}&filter[version]={}&sort=-uploadedDate&limit=5",
            ASC_BASE, app_id, build_number
        );

        let res = send_with_retry(|| client.get(&url), "fetch ASC builds").await?;
        let builds: BuildsResponse = res.json().await?;

        if let Some(build) = builds.data.first() {
            let state = match build.attributes.processing_state.as_str() {
                "PROCESSING" | "RECEIVE" => ProcessingState::Processing,
                "VALID" => ProcessingState::Valid,
                "INVALID" => ProcessingState::Invalid(build.id.clone()),
                other => ProcessingState::Unknown(other.to_string()),
            };

            match state {
                ProcessingState::Valid => {
                    let uploaded = build
                        .attributes
                        .uploaded_date
                        .as_deref()
                        .unwrap_or("unknown");
                    spinner.finish_with_message(format!(
                        "Build {} (v{}) processed — uploaded {}",
                        build_number, build.attributes.version, uploaded,
                    ));
                    return Ok(ProcessedBuild {
                        id: build.id.clone(),
                        version: build.attributes.version.clone(),
                        uploaded_date: build.attributes.uploaded_date.clone(),
                    });
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
                    let ver_hint = if build.attributes.version.is_empty() {
                        String::new()
                    } else {
                        format!(" v{}", build.attributes.version)
                    };
                    spinner.set_message(format!(
                        "Processing{ver_hint}... (attempt {}/{})",
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

pub(crate) async fn submit_to_testflight(creds: &AppleCredentials, build_id: &str) -> Result<()> {
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

    // Not idempotent in the general sense, but ASC returns 409 when the
    // build is already submitted, which we treat as success. No retry —
    // a transient failure leaves the submission in a known state that
    // the caller can re-drive manually.
    let res = client.post(&url).json(&body).send().await?;
    let status = res.status();

    if status.as_u16() == 409 {
        // Already submitted — that's fine.
        tracing::debug!("Build already submitted to TestFlight");
        return Ok(());
    }

    if !status.is_success() {
        let body = res.text().await.unwrap_or_default();
        return Err(map_status_to_error(status.as_u16(), body, "TestFlight submit").into());
    }

    Ok(())
}

// ─── TestFlight group assignment ──────────────────────────────────────────────

#[derive(Debug, Deserialize)]
struct BetaGroupsResponse {
    data: Vec<BetaGroupData>,
}

#[derive(Debug, Deserialize)]
struct BetaGroupData {
    id: String,
}

/// Add a processed build to a named TestFlight beta group.
///
/// Looks up the group by name via ASC API, then creates a relationship
/// between the group and the build. If the build is already in the group
/// ASC returns 409 which is treated as success.
pub(crate) async fn add_build_to_beta_group(
    creds: &AppleCredentials,
    app_id: &str,
    build_id: &str,
    group_name: &str,
) -> Result<()> {
    let jwt = generate_jwt(creds)?;
    let client = asc_client(&jwt);

    // 1. Find the beta group by name
    let url = format!(
        "{}/betaGroups?filter[app]={}&filter[name]={}",
        ASC_BASE, app_id, group_name
    );
    let res = send_with_retry(|| client.get(&url), "fetch beta groups").await?;
    let groups: BetaGroupsResponse = res.json().await?;

    let group_id = groups.data.first().map(|g| g.id.clone()).with_context(|| {
        format!(
            "TestFlight beta group \"{}\" not found in App Store Connect. \
                 Create it first at https://appstoreconnect.apple.com",
            group_name
        )
    })?;

    // 2. Add build to the group
    let add_url = format!("{}/betaGroups/{}/relationships/builds", ASC_BASE, group_id);
    let body = serde_json::json!({
        "data": [{ "type": "builds", "id": build_id }]
    });

    let res = client.post(&add_url).json(&body).send().await?;
    let status = res.status();

    if status.as_u16() == 409 {
        tracing::debug!("Build already in group \"{}\" — skipping", group_name);
        return Ok(());
    }

    if !status.is_success() {
        let body = res.text().await.unwrap_or_default();
        return Err(map_status_to_error(status.as_u16(), body, "add build to beta group").into());
    }

    Ok(())
}
