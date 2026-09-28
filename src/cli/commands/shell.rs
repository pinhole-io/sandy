use clap::Args;
use std::io::{self, Write};

#[derive(Args, Debug)]
#[command(about = "Start an interactive Python-like shell")]
pub struct ShellArgs {
    /// Enable verbose output
    #[arg(short, long)]
    pub verbose: bool,
}

pub fn run_shell(args: ShellArgs) -> Result<(), Box<dyn std::error::Error>> {
    println!("Nexus Py Shell v0.1.0");
    println!("Type 'help' for available commands, 'exit' or 'quit' to exit");
    if args.verbose {
        println!("Verbose mode enabled");
    }

    loop {
        print!(">>> ");
        io::stdout().flush()?;

        let mut input = String::new();
        io::stdin().read_line(&mut input)?;
        let input = input.trim();

        if input.is_empty() {
            continue;
        }

        match input {
            "exit" | "quit" => {
                println!("Goodbye!");
                break;
            }
            "help" => {
                println!("Available commands:");
                println!("  help  - Show this help message");
                println!("  exit  - Exit the shell");
                println!("  quit  - Exit the shell");
            }
            _ => {
                // Echo the input for now (can be extended with actual command processing)
                if args.verbose {
                    println!("[DEBUG] Received: {}", input);
                }
                println!("{}", input);
            }
        }
    }

    Ok(())
}
