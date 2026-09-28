use crate::service::PythonExecutionService;
use clap::Args;
use std::io::{self, Read};

#[derive(Args, Debug)]
#[command(about = "Execute Python code from a string using uv inline packages")]
pub struct ExecCodeArgs {
    /// Python code to execute (if not provided, reads from stdin)
    #[arg(short, long)]
    pub code: Option<String>,
}

pub fn run_exec_code(args: ExecCodeArgs) -> Result<(), Box<dyn std::error::Error>> {
    let service = PythonExecutionService::new();

    // Get code from argument or stdin
    let code = if let Some(code) = args.code {
        code
    } else {
        // Read from stdin
        let mut buffer = String::new();
        io::stdin().read_to_string(&mut buffer)?;
        buffer
    };

    if code.trim().is_empty() {
        eprintln!("Error: No code provided. Use --code <code> or pipe code via stdin.");
        std::process::exit(1);
    }

    // Use streaming execution to show output in real-time
    let result = service.execute_code_streaming(&code, |line| {
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
