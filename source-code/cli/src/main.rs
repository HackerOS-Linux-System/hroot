use anyhow::{anyhow, Result};
use hammer_core::Logger;
use lexopt::{Arg, Parser, ValueExt};
use nix::unistd::Uid;
use owo_colors::OwoColorize;
use std::env;
use std::path::PathBuf;
use std::process::{Command, Stdio};

const BIN_DIR: &str = "/usr/lib/HackerOS/hammer/bin";

fn main() -> Result<()> {
    if !Uid::current().is_root() {
        eprintln!("{}", "This tool must be run as root.".red().bold());
        std::process::exit(1);
    }

    Logger::init()?;

    let args: Vec<String> = env::args().collect();
    let mut parser = Parser::from_env();

    // Peek at the first argument to decide dispatch
    let arg = parser.next()?;

    match arg {
        Some(Arg::Value(val)) => {
            let command = val.string()?;
            match command.as_str() {
                // UPDATE / INIT -> hammer-updater
                "update" => run_binary("hammer-updater", &["update"], &args[2..])?,
                "init" => run_binary("hammer-updater", &["init"], &args[2..])?,
                "check" => run_binary("hammer-updater", &["check"], &args[2..])?,
                "rollback" => run_binary("hammer-updater", &["rollback"], &args[2..])?,
                "layer" => run_binary("hammer-updater", &["layer"], &args[2..])?,

                // BUILD -> hammer-builder
                "build" => run_binary("hammer-builder", &["build"], &args[2..])?,
                "delta" => run_binary("hammer-builder", &["delta"], &args[2..])?,
                "build-init" => run_binary("hammer-builder", &["init"], &args[2..])?,

                // READ-ONLY -> hammer-read
                "read-only" | "ro" => run_binary("hammer-read", &[], &args[2..])?,

                "help" => print_help(),
                "version" => print_version(),
                _ => {
                    print_help();
                    return Err(anyhow!("Unknown command: {}", command));
                }
            }
        }
        Some(Arg::Long("help")) | Some(Arg::Short('h')) => print_help(),
        Some(Arg::Long("version")) | Some(Arg::Short('v')) => print_version(),
        None => print_help(),
        _ => return Err(anyhow!("Unexpected argument")),
    }

    Ok(())
}

fn run_binary(binary_name: &str, prefix_args: &[&str], user_args: &[String]) -> Result<()> {
    let binary_path = PathBuf::from(BIN_DIR).join(binary_name);

    // Construct the full argument list: [prefix_command, ...user_flags]
    let mut final_args: Vec<String> = Vec::new();
    for p in prefix_args {
        final_args.push(p.to_string());
    }
    final_args.extend_from_slice(user_args);

    if !binary_path.exists() {
        Logger::log(&format!("Binary not found at {:?}, attempting fallback lookup...", binary_path));
    }

    let cmd_to_run = if binary_path.exists() {
        binary_path.to_string_lossy().to_string()
    } else {
        // Assume it's in PATH for dev
        binary_name.to_string()
    };

    let mut child = Command::new(cmd_to_run)
    .args(&final_args)
    .stdin(Stdio::inherit())
    .stdout(Stdio::inherit())
    .stderr(Stdio::inherit())
    .spawn()
    .map_err(|e| anyhow!("Failed to spawn {}: {}", binary_name, e))?;

    let status = child.wait()?;

    if !status.success() {
        std::process::exit(status.code().unwrap_or(1));
    }

    Ok(())
}

fn print_help() {
    println!("{}", "Hammer Next-Gen".blue().bold());
    println!("Atomic Debian Management Tool\n");
    println!("{} {}", "Usage:".yellow(), "hammer <command> [options]");
    println!("\n{}", "Commands:".green());
    println!("  {: <20} {}", "update", "Update system (check/download/apply)");
    println!("  {: <20} {}", "layer <deb>", "Install local .deb (rebuilds image)");
    println!("  {: <20} {}", "check", "Check for updates without applying");
    println!("  {: <20} {}", "init", "Initialize OSTree system");
    println!("  {: <20} {}", "rollback", "Rollback to previous version");
    println!("  {: <20} {}", "build", "Build ISO images");
    println!("  {: <20} {}", "read-only", "Manage Read-Only state");
    println!("  {: <20} {}", "  lock/unlock", "  - Permanent RO/RW toggle");
    println!("  {: <20} {}", "  temporary-unlock", "  - Writable OverlayFS (resets on reboot)");
    println!("  {: <20} {}", "help", "Show this message");
}

fn print_version() {
    println!("hammer 1.0");
}
