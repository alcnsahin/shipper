use console::style;
use indicatif::{ProgressBar, ProgressStyle};
use std::time::Duration;

pub fn spinner(msg: &str) -> ProgressBar {
    let pb = ProgressBar::new_spinner();
    pb.set_style(
        ProgressStyle::default_spinner()
            .tick_strings(&["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"])
            .template("{spinner:.cyan} {msg}")
            .unwrap(),
    );
    pb.set_message(msg.to_string());
    pb.enable_steady_tick(Duration::from_millis(80));
    pb
}

pub fn step(n: usize, total: usize, msg: &str) {
    println!(
        "  {} {}",
        style(format!("[{}/{}]", n, total)).bold().dim(),
        msg
    );
}

pub fn success(msg: &str) {
    println!("  {} {}", style("✓").bold().green(), msg);
}

pub fn failure(msg: &str) {
    println!("  {} {}", style("✗").bold().red(), msg);
}

pub fn info(msg: &str) {
    println!("  {} {}", style("·").dim(), msg);
}
