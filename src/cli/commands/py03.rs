use crate::service::PythonExecutionService;
use clap::Args;
use std::path::PathBuf;

#[derive(Args, Debug)]
#[command(about = "Execute a Python script using uv with inline package dependencies")]
pub struct Py03Args {
    /// Path to the Python script to execute
    #[arg(required = true)]
    pub script: PathBuf,
}

pub fn run_py03(args: Py03Args) -> Result<(), Box<dyn std::error::Error>> {
    let service = PythonExecutionService::new();

    // Use streaming execution to show output in real-time
    let result = service.execute_script_streaming(&args.script, |line| {
        println!("{}", line);
        Ok(())
    })?;

    // If there's stderr output, print it
    if !result.stderr.is_empty() {
        eprintln!("{}", result.stderr);
    }

    // Exit with the script's exit code
    if result.exit_code != 0 {
        std::process::exit(result.exit_code);
    }

    Ok(())
}
