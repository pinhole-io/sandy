use crate::service::JsExecutionService;
use clap::Args;
use std::io::{self, Read};

#[derive(Args, Debug)]
#[command(about = "Execute JavaScript code from a string using Node.js")]
pub struct ExecJsArgs {
    /// JavaScript code to execute (if not provided, reads from stdin)
    #[arg(short, long)]
    pub code: Option<String>,
}

pub fn run_exec_js(args: ExecJsArgs) -> Result<(), Box<dyn std::error::Error>> {
    let service = JsExecutionService::new();

    let code = if let Some(code) = args.code {
        code
    } else {
        let mut buffer = String::new();
        io::stdin().read_to_string(&mut buffer)?;
        buffer
    };

    if code.trim().is_empty() {
        eprintln!("Error: No code provided. Use --code <code> or pipe code via stdin.");
        std::process::exit(1);
    }

    let result = service.execute_code_streaming(&code, |line| {
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
