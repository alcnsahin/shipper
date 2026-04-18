use anyhow::Result;
use console::style;

use crate::config;

pub fn run() -> Result<()> {
    let mut ok = true;

    // ── Global config ───────────────────────────────────────────────────
    let global_path = config::global_config_path();
    if global_path.exists() {
        match config::load_global_or_default() {
            cfg if cfg.credentials.is_some() => {
                println!(
                    "  {} {} — parsed OK",
                    style("✓").green().bold(),
                    global_path.display()
                );

                // Check credential file accessibility.
                if let Some(apple) = cfg.credentials.as_ref().and_then(|c| c.apple.as_ref()) {
                    let key = config::expand_path(&apple.key_path);
                    if !key.exists() {
                        println!(
                            "  {} Apple key_path not found: {}",
                            style("!").yellow().bold(),
                            key.display()
                        );
                    }
                }
                if let Some(google) = cfg.credentials.as_ref().and_then(|c| c.google.as_ref()) {
                    let sa = config::expand_path(&google.service_account);
                    if !sa.exists() {
                        println!(
                            "  {} Google service_account not found: {}",
                            style("!").yellow().bold(),
                            sa.display()
                        );
                    }
                }
            }
            _ => {
                println!(
                    "  {} {} — parsed OK (no credentials configured)",
                    style("✓").green().bold(),
                    global_path.display()
                );
            }
        }
    } else {
        println!(
            "  {} {} — not found (optional)",
            style("-").dim(),
            global_path.display()
        );
    }

    // ── Project config ──────────────────────────────────────────────────
    match config::validate_project_config() {
        Ok(project) => {
            println!("  {} shipper.toml — parsed OK", style("✓").green().bold());

            // Summary
            if project.ios.is_some() {
                println!("    iOS:     configured");
            }
            if project.android.is_some() {
                println!("    Android: configured");
            }
            if project.ios.is_none() && project.android.is_none() {
                println!(
                    "  {} No [ios] or [android] section — nothing to deploy",
                    style("!").yellow().bold()
                );
            }
        }
        Err(e) => {
            println!("  {} shipper.toml — {}", style("✗").red().bold(), e);
            ok = false;
        }
    }

    println!();
    if ok {
        println!("  {} Configuration is valid", style("✓").green().bold());
    } else {
        anyhow::bail!("Configuration has errors — see above");
    }

    Ok(())
}
