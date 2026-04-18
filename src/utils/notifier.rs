use crate::config::{read_secret, Config};
use anyhow::Result;

pub struct DeployResult {
    pub app_name: String,
    pub platform: String,
    pub version: String,
    pub build_number: String,
    pub destination: String,
    pub success: bool,
    pub error: Option<String>,
}

pub async fn notify(config: &Config, result: &DeployResult) -> Result<()> {
    for channel in config.notify_channels() {
        match channel.as_str() {
            "telegram" => {
                if let Some(tg) = config.telegram_config() {
                    send_telegram(tg.clone(), result).await?;
                }
            }
            other => {
                tracing::warn!("Unknown notification channel: {}", other);
            }
        }
    }
    Ok(())
}

async fn send_telegram(config: crate::config::TelegramConfig, result: &DeployResult) -> Result<()> {
    let token = read_secret(&config.bot_token_path)?;

    let icon = if result.success { "✅" } else { "❌" };
    let text = if result.success {
        format!(
            "{} *{}* v{} ({}) → {} {}",
            icon,
            escape_markdown(&result.app_name),
            escape_markdown(&result.version),
            escape_markdown(&result.build_number),
            escape_markdown(&result.destination),
            result.platform.to_uppercase(),
        )
    } else {
        format!(
            "{} *{}* {} deploy failed\n`{}`",
            icon,
            escape_markdown(&result.app_name),
            result.platform.to_uppercase(),
            escape_markdown(result.error.as_deref().unwrap_or("unknown error")),
        )
    };

    let url = format!("https://api.telegram.org/bot{}/sendMessage", token.expose());
    let client = reqwest::Client::new();
    let res = client
        .post(&url)
        .json(&serde_json::json!({
            "chat_id": config.chat_id,
            "text": text,
            "parse_mode": "MarkdownV2",
        }))
        .send()
        .await?;

    if !res.status().is_success() {
        let body = res.text().await.unwrap_or_default();
        tracing::warn!("Telegram notification failed: {}", body);
    }

    Ok(())
}

fn escape_markdown(s: &str) -> String {
    // Telegram MarkdownV2 special chars
    let special = [
        '_', '*', '[', ']', '(', ')', '~', '`', '>', '#', '+', '-', '=', '|', '{', '}', '.', '!',
    ];
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        if special.contains(&c) {
            out.push('\\');
        }
        out.push(c);
    }
    out
}
