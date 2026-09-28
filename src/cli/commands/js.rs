use crate::service::JsExecutionService;
use clap::Args;
use std::path::PathBuf;

#[derive(Args, Debug)]
#[command(
    about = "Execute a JavaScript file with Node.js and optional inline package dependencies"
)]
pub struct JsArgs {
    /// Path to the JavaScript file to execute
    #[arg(required = true)]
    pub script: PathBuf,
}

pub fn run_js(args: JsArgs) -> Result<(), Box<dyn std::error::Error>> {
    let service = JsExecutionService::new();

    let result = service.execute_script_streaming(&args.script, |line| {
        println!("{}", line);
        Ok(())
    })?;

    if !result.stderr.is_empty() {
        eprintln!("{}", result.stderr);
    }

    if result.exit_code != 0 {
        std::process::exit(result.exit_code);
    }

    Ok(())
}
