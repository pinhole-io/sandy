use chrono::{DateTime, Utc};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::service::firecracker::{
    FirecrackerConfig, FirecrackerExecutionResult, FirecrackerService,
};

pub(crate) enum FirecrackerCodeKind {
    Python,
    Javascript,
}

pub(crate) async fn execute(
    kind: FirecrackerCodeKind,
    code: String,
    config: FirecrackerConfig,
) -> Result<(), Box<dyn std::error::Error>> {
    let started = SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs();
    let timer = std::time::Instant::now();
    let service = FirecrackerService::new(config)?;
    let result = match kind {
        FirecrackerCodeKind::Python => service.execute_python_code(&code).await?,
        FirecrackerCodeKind::Javascript => service.execute_javascript_code(&code).await?,
    };
    print_xml(started, timer.elapsed(), &result);
    Ok(())
}

fn print_xml(started: u64, elapsed: std::time::Duration, result: &FirecrackerExecutionResult) {
    let timestamp = DateTime::<Utc>::from_timestamp(started as i64, 0).unwrap_or_else(Utc::now);
    println!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
         <execution>\n\
         <output><stdout>{}</stdout><stderr>{}</stderr></output>\n\
         <metrics><start_time>{}</start_time><duration_seconds>{:.3}</duration_seconds>\
         <duration_milliseconds>{}</duration_milliseconds><exit_code>{}</exit_code></metrics>\n\
         </execution>",
        escape_xml(&result.stdout),
        escape_xml(&result.stderr),
        timestamp.format("%Y-%m-%d %H:%M:%S UTC"),
        elapsed.as_secs_f64(),
        elapsed.as_millis(),
        result.exit_code
    );
}

pub(crate) fn escape_xml(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}
