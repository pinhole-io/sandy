use crate::service::docker::{DockerConfig, DockerService, SandboxConfig};
use anyhow::anyhow;
use chrono::{DateTime, Utc};
use clap::Args;
use std::io::{self, Read};
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};
use tracing::{error, info};

fn escape_xml(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

fn format_timestamp(secs: u64) -> String {
    let dt = DateTime::<Utc>::from_timestamp(secs as i64, 0).unwrap_or_else(Utc::now);
    dt.format("%Y-%m-%d %H:%M:%S UTC").to_string()
}

#[derive(Args, Debug)]
#[command(about = "Execute JavaScript code in a Docker container")]
pub struct DockerExecJsArgs {
    /// JavaScript code to execute (if not provided, reads from stdin)
    #[arg(short, long)]
    pub code: Option<String>,

    /// Docker image name (default: sandy)
    #[arg(long, default_value = "sandy")]
    pub image: Option<String>,

    /// Docker image tag (default: latest)
    #[arg(long, default_value = "latest")]
    pub tag: Option<String>,

    /// Path to Dockerfile
    #[arg(long)]
    pub dockerfile: Option<String>,

    /// Build context directory (default: .)
    #[arg(long)]
    pub build_context: Option<String>,

    /// When the image is missing, do not run `docker build` (fail with instructions instead)
    #[arg(long)]
    pub no_auto_build: bool,

    /// `PYTHON_VERSION` build-arg passed to `docker build` when auto-building (image still includes Node)
    #[arg(long, default_value = "3.13")]
    pub python_version: String,

    /// Allow outbound network in the container (default: **off** — `network_mode=none`, no egress).
    /// Turn **on** so `npm` can install packages from the registry.
    #[arg(long)]
    pub allow_network: bool,
}

fn default_dockerfile_path() -> String {
    const CANDIDATES: &[&str] = &[
        ".docker/Dockerfile",
        "sandy/.docker/Dockerfile",
        "src/sandy/.docker/Dockerfile",
    ];
    for p in CANDIDATES {
        if Path::new(p).is_file() {
            return (*p).to_string();
        }
    }
    ".docker/Dockerfile".to_string()
}

pub async fn run_docker_exec_js(args: DockerExecJsArgs) -> Result<(), Box<dyn std::error::Error>> {
    let code = if let Some(code) = args.code {
        code
    } else {
        let mut buffer = String::new();
        io::stdin().read_to_string(&mut buffer)?;
        buffer
    };

    if code.trim().is_empty() {
        error!("No code provided. Use --code <code> or pipe code via stdin.");
        std::process::exit(1);
    }

    let image_name = args.image.as_deref().unwrap_or("sandy");
    let image_tag = args.tag.as_deref().unwrap_or("latest");

    let sandbox = SandboxConfig {
        network_disabled: !args.allow_network,
        python_version: args.python_version.clone(),
        ..Default::default()
    };

    let config = DockerConfig {
        image_name: image_name.to_string(),
        image_tag: image_tag.to_string(),
        dockerfile_path: args
            .dockerfile
            .clone()
            .unwrap_or_else(default_dockerfile_path),
        build_context: args
            .build_context
            .clone()
            .unwrap_or_else(|| ".".to_string()),
        sandbox,
    };
    let dockerfile_for_log = config.dockerfile_path.clone();
    let build_context_for_log = config.build_context.clone();

    info!("Connecting to Docker daemon...");
    let docker = DockerService::new(config).await.inspect_err(|_| {
        error!("Failed to connect to Docker daemon. Is Docker running?");
    })?;
    info!("Connected to Docker daemon.");
    if args.allow_network {
        info!("Container network: outbound allowed (--allow-network)");
    } else {
        info!(
            "Container network: isolated (no egress); use --allow-network for npm registry installs"
        );
    }

    let full_image = format!("{}:{}", image_name, image_tag);
    match docker.image_exists().await {
        Ok(true) => {}
        Ok(false) => {
            if args.no_auto_build {
                error!("Docker image not found: {}", full_image);
                return Err(anyhow!(
                    "Docker image '{full_image}' not found.\n\n\
                         Build it first, or omit --no-auto-build to build automatically. Examples:\n\n\
                         • Workspace root (parent of `sandy/`): docker build -f sandy/.docker/Dockerfile -t {image_name}:{image_tag} .\n\n\
                         • `sandy/` crate root: docker build -f .docker/Dockerfile -t {image_name}:{image_tag} .\n\n\
                         • See README / docs/docker.md.\n",
                )
                .into());
            }
            info!(
                "Image {} not found; building with Dockerfile {} (context {})…",
                full_image, dockerfile_for_log, build_context_for_log
            );
            docker
                .build_image_via_cli(&args.python_version)
                .await
                .map_err(|e| anyhow!("{}", e))?;
            if !docker.image_exists().await.map_err(|e| anyhow!("{}", e))? {
                return Err(anyhow!(
                    "Build completed but image '{}' is still not visible to the Docker daemon.",
                    full_image
                )
                .into());
            }
            info!("Image {} is ready.", full_image);
        }
        Err(e) => {
            error!("Could not verify Docker image: {}", e);
            return Err(anyhow!("Could not verify Docker image: {}", e).into());
        }
    }

    let start_time = SystemTime::now();
    let start_timestamp = start_time
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let start_datetime = format_timestamp(start_timestamp);

    info!("Executing JavaScript in Docker container...");

    let (stdout, stderr, exit_code) = docker.execute_javascript_code(&code).await?;

    let duration = SystemTime::now()
        .duration_since(start_time)
        .unwrap_or_default();

    println!("<?xml version=\"1.0\" encoding=\"UTF-8\"?>");
    println!("<execution>");
    println!("  <output>");
    println!("    <stdout>{}</stdout>", escape_xml(&stdout));
    println!("    <stderr>{}</stderr>", escape_xml(&stderr));
    println!("  </output>");
    println!("  <metrics>");
    println!(
        "    <start_time>{}</start_time>",
        escape_xml(&start_datetime)
    );
    println!(
        "    <duration_seconds>{:.3}</duration_seconds>",
        duration.as_secs_f64()
    );
    println!(
        "    <duration_milliseconds>{}</duration_milliseconds>",
        duration.as_millis()
    );
    println!("    <exit_code>{}</exit_code>", exit_code);
    println!("  </metrics>");
    println!("</execution>");

    if exit_code != 0 {
        std::process::exit(exit_code);
    }

    Ok(())
}
