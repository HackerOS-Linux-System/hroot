use std::fs::{self, File};
use std::io::{self, copy};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use lexopt::{Arg, Parser};
use owo_colors::{OwoColorize};
use miette::{self, Diagnostic, Result};
use figlet_rs::FIGfont;
use indicatif::{ProgressBar, ProgressStyle};
use reqwest::blocking;
use which::which;
use thiserror::Error;

const VERSION: &str = "0.9";
const HAMMER_PATH: &str = "/usr/lib/HackerOS/hammer/bin";
const VERSION_FILE: &str = "/usr/lib/hammer/version.hacker";
const REMOTE_VERSION_URL: &str = "https://raw.githubusercontent.com/HackerOS-Linux-System/hammer/main/config/version.hacker";
const RELEASE_BASE_URL: &str = "https://github.com/HackerOS-Linux-System/hammer/releases/download/v";

#[derive(Error, Diagnostic, Debug)]
enum HammerError {
    #[error("IO error: {0}")]
    #[diagnostic(code(hammer::io))]
    Io(#[from] io::Error),

    #[error("HTTP error: {0}")]
    #[diagnostic(code(hammer::http))]
    Http(#[from] reqwest::Error),

    #[error("Command execution failed: {0}")]
    #[diagnostic(code(hammer::command))]
    Command(String),

    #[error("Version parse error: {0}")]
    #[diagnostic(code(hammer::version))]
    Version(String),

    #[error("Browser not found")]
    #[diagnostic(code(hammer::no_browser))]
    NoBrowser,

    #[error("Invalid arguments: {0}")]
    #[diagnostic(code(hammer::invalid_args))]
    InvalidArgs(String),

    #[error("Progress template error: {0}")]
    #[diagnostic(code(hammer::progress_template))]
    ProgressTemplate(String),

    #[error("Argument parse error: {0}")]
    #[diagnostic(code(hammer::parse))]
    Parse(#[from] lexopt::Error),
}

fn main() -> miette::Result<()> {
    the_logic()?;
    Ok(())
}

fn the_logic() -> std::result::Result<(), HammerError> {
    let mut parser = Parser::from_env();

    let first = parser.next().map_err(HammerError::from)?;
    let command = match first {
        Some(Arg::Short('v') | Arg::Long("version")) => return print_version(),
        Some(Arg::Short('h') | Arg::Long("help")) => return print_help(),
        Some(Arg::Value(cmd)) => cmd.into_string().map_err(|e| HammerError::InvalidArgs(e.to_string_lossy().to_string()))?,
        None => return print_help(),
        _ => return Err(HammerError::InvalidArgs("Unexpected option before command".to_string())),
    };

    match command.as_str() {
        "install" => handle_install(&mut parser)?,
        "remove" => handle_remove(&mut parser)?,
        "update" => run_updater("update", vec![])?,
        "clean" => run_core("clean", vec![])?,
        "refresh" => run_core("refresh", vec![])?,
        "build" => run_builder("build", vec![])?,
        "switch" => handle_switch(&mut parser)?,
        "deploy" => run_core("deploy", vec![])?,
        "build-init" | "build init" => run_builder("init", vec![])?,
        "about" => about()?,
        "tui" => run_tui(vec![])?,
        "status" => run_core("status", vec![])?,
        "history" => run_core("history", vec![])?,
        "rollback" => handle_rollback(&mut parser)?,
        "init" => run_updater("init", vec![])?,
        "upgrade" => upgrade()?,
        "issue" => issue()?,
        _ => return Err(HammerError::InvalidArgs(format!("Unknown command: {}", command))),
    }

    Ok(())
}

fn print_version() -> std::result::Result<(), HammerError> {
    println!("{}", VERSION.bold().bright_blue());
    Ok(())
}

fn print_help() -> std::result::Result<(), HammerError> {
    println!("{}", "Hammer CLI Tool for HackerOS Atomic".bold().bright_blue());
    println!("{}", "Usage: hammer <command> [options]".bright_purple().bold());

    println!("\n{}", "Commands:".green().bold());
    let commands = vec![
        ("install [--container] <package>", "Install a package (optionally in container)"),
        ("remove [--container] <package>", "Remove a package (optionally from container)"),
        ("update", "Update the system atomically"),
        ("clean", "Clean up unused resources"),
        ("refresh", "Refresh repositories"),
        ("build", "Build atomic ISO (must be in project dir)"),
        ("switch [deployment]", "Switch to a deployment (rollback if no arg)"),
        ("deploy", "Create a new deployment"),
        ("build init", "Initialize build project"),
        ("about", "Show tool information"),
        ("tui", "Launch TUI interface"),
        ("status", "Show current deployment status"),
        ("history", "Show deployment history"),
        ("rollback [n]", "Rollback n steps (default 1)"),
        ("init", "Initialize the atomic system (linking without update)"),
        ("upgrade", "Upgrade the hammer tool"),
        ("issue", "Open new issue in GitHub repository"),
    ];

    for (cmd, desc) in commands {
        println!(" {} {}", cmd.yellow().bold(), desc.bright_cyan());
    }

    Ok(())
}

fn handle_install(parser: &mut Parser) -> std::result::Result<(), HammerError> {
    let mut container = false;
    let mut pkg = None;

    while let Some(arg) = parser.next().map_err(HammerError::from)? {
        match arg {
            Arg::Long("container") => container = true,
            Arg::Value(val) => pkg = Some(val.into_string().map_err(|e| HammerError::InvalidArgs(e.to_string_lossy().to_string()))?),
            _ => return Err(HammerError::InvalidArgs(format!("Unexpected argument: {:?}", arg))),
        }
    }

    let pkg = pkg.ok_or(HammerError::InvalidArgs("Missing package name".to_string()))?;
    if container {
        run_containers("install", vec![pkg])?;
    } else {
        run_core("install", vec![pkg])?;
    }
    Ok(())
}

fn handle_remove(parser: &mut Parser) -> std::result::Result<(), HammerError> {
    let mut container = false;
    let mut pkg = None;

    while let Some(arg) = parser.next().map_err(HammerError::from)? {
        match arg {
            Arg::Long("container") => container = true,
            Arg::Value(val) => pkg = Some(val.into_string().map_err(|e| HammerError::InvalidArgs(e.to_string_lossy().to_string()))?),
            _ => return Err(HammerError::InvalidArgs(format!("Unexpected argument: {:?}", arg))),
        }
    }

    let pkg = pkg.ok_or(HammerError::InvalidArgs("Missing package name".to_string()))?;
    if container {
        run_containers("remove", vec![pkg])?;
    } else {
        run_core("remove", vec![pkg])?;
    }
    Ok(())
}

fn handle_switch(parser: &mut Parser) -> std::result::Result<(), HammerError> {
    let mut deployment = None;

    while let Some(arg) = parser.next().map_err(HammerError::from)? {
        match arg {
            Arg::Value(val) => deployment = Some(val.into_string().map_err(|e| HammerError::InvalidArgs(e.to_string_lossy().to_string()))?),
            _ => return Err(HammerError::InvalidArgs(format!("Unexpected argument: {:?}", arg))),
        }
    }

    let args = deployment.map_or(vec![], |d| vec![d]);
    run_core("switch", args)?;
    Ok(())
}

fn handle_rollback(parser: &mut Parser) -> std::result::Result<(), HammerError> {
    let mut n = "1".to_string();

    while let Some(arg) = parser.next().map_err(HammerError::from)? {
        match arg {
            Arg::Value(val) => n = val.into_string().map_err(|e| HammerError::InvalidArgs(e.to_string_lossy().to_string()))?,
            _ => return Err(HammerError::InvalidArgs(format!("Unexpected argument: {:?}", arg))),
        }
    }

    run_core("rollback", vec![n])?;
    Ok(())
}

fn run_core(subcommand: &str, args: Vec<String>) -> std::result::Result<(), HammerError> {
    let binary = PathBuf::from(HAMMER_PATH).join("hammer-core");
    spawn_command(&binary, &[subcommand.to_string()], &args)
}

fn run_updater(subcommand: &str, args: Vec<String>) -> std::result::Result<(), HammerError> {
    let binary = PathBuf::from(HAMMER_PATH).join("hammer-updater");
    spawn_command(&binary, &[subcommand.to_string()], &args)
}

fn run_builder(subcommand: &str, args: Vec<String>) -> std::result::Result<(), HammerError> {
    let binary = PathBuf::from(HAMMER_PATH).join("hammer-builder");
    spawn_command(&binary, &[subcommand.to_string()], &args)
}

fn run_tui(args: Vec<String>) -> std::result::Result<(), HammerError> {
    let binary = PathBuf::from(HAMMER_PATH).join("hammer-tui");
    spawn_command(&binary, &[], &args)
}

fn run_containers(subcommand: &str, args: Vec<String>) -> std::result::Result<(), HammerError> {
    let binary = PathBuf::from(HAMMER_PATH).join("hammer-containers");
    spawn_command(&binary, &[subcommand.to_string()], &args)
}

fn spawn_command(binary: &Path, subcmds: &[String], args: &[String]) -> std::result::Result<(), HammerError> {
    let mut cmd = Command::new(binary);
    for s in subcmds {
        cmd.arg(s);
    }
    for a in args {
        cmd.arg(a);
    }
    cmd.stdin(Stdio::inherit())
       .stdout(Stdio::inherit())
       .stderr(Stdio::inherit());

    let status = cmd.status()?;
    if !status.success() {
        return Err(HammerError::Command(format!("Command failed with exit code {:?}", status.code())));
    }
    Ok(())
}

fn about() -> std::result::Result<(), HammerError> {
    let standard_font = FIGfont::standard().map_err(|e| HammerError::Command(e.to_string()))?;
    let figure = standard_font.convert("Hammer");
    if let Some(fig) = figure {
        println!("{}", fig.to_string().bright_magenta().bold());
    } else {
        println!("{}", "Hammer CLI Tool for HackerOS Atomic".bold().blue());
    }

    let content = format!(
        "{} {}\n{} Tool for managing atomic installations, updates, and builds inspired by apx and rpm-ostree.\n{} \n- {} Core operations in Crystal\n- {} System updater in Crystal\n- {} ISO builder in Crystal\n- {} TUI interface in Go with Bubble Tea\n{} {}",
        "Version:".green().bold(), VERSION.bright_yellow(),
        "Description:".green().bold(),
        "Components:".green().bold(),
        "hammer-core:".yellow().bold(),
        "hammer-updater:".yellow().bold(),
        "hammer-builder:".yellow().bold(),
        "hammer-tui:".yellow().bold(),
        "Location:".green().bold(), HAMMER_PATH.bright_cyan()
    );

    let width = content.lines().map(|l| l.len()).max().unwrap_or(50) + 4;
    let top = format!("╭{}╮", "─".repeat(width - 2));
    let bottom = format!("╰{}╯", "─".repeat(width - 2));
    println!("{}", top.cyan().bold());
    for line in content.lines() {
        println!("│ {:<width$} │", line, width = width - 4);
    }
    println!("{}", bottom.cyan().bold());

    Ok(())
}

fn upgrade() -> std::result::Result<(), HammerError> {
    let pb = ProgressBar::new_spinner();
    pb.set_style(ProgressStyle::with_template("{spinner:.blue} {msg}")
        .map_err(|e| HammerError::ProgressTemplate(e.to_string()))?
        .tick_strings(&["🌑", "🌒", "🌓", "🌔", "🌕", "🌖", "🌗", "🌘"]));
    pb.set_message("Checking for updates...".bright_yellow().to_string());

    let local_version = if Path::new(VERSION_FILE).exists() {
        fs::read_to_string(VERSION_FILE)?
            .trim()
            .replace(['[', ']'], "")
            .trim()
            .to_string()
    } else {
        "0.0".to_string()
    };

    let response = blocking::get(REMOTE_VERSION_URL)?;
    let remote_version = response.text()?
        .trim()
        .replace(['[', ']'], "")
        .trim()
        .to_string();

    if remote_version > local_version {
        pb.set_message(format!("Upgrading from {} to {}...", local_version.bright_red(), remote_version.bright_green()));

        let hammer_path = PathBuf::from(HAMMER_PATH);
        let binaries = vec![
            ("hammer", PathBuf::from("/usr/bin/hammer")),
            ("hammer-updater", hammer_path.join("hammer-updater")),
            ("hammer-core", hammer_path.join("hammer-core")),
            ("hammer-tui", hammer_path.join("hammer-tui")),
            ("hammer-builder", hammer_path.join("hammer-builder")),
            ("hammer-containers", hammer_path.join("hammer-containers")),
        ];

        for (name, path) in binaries {
            let url = format!("{}{}/{}", RELEASE_BASE_URL, remote_version, name);
            pb.set_message(format!("Downloading {}...", name.yellow().bold()));

            let mut response = blocking::get(&url)?;
            let mut file = File::create(&path)?;
            copy(&mut response, &mut file)?;

            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755))?;
            pb.println(format!("Downloaded {}", name.green().bold()));
        }

        fs::write(VERSION_FILE, format!("[ {} ]", remote_version))?;
        pb.finish_with_message("Upgrade completed.".bright_green().to_string());
    } else {
        pb.finish_with_message(format!("Already up to date (version {}).", local_version.bright_blue()));
    }

    Ok(())
}

fn issue() -> std::result::Result<(), HammerError> {
    let url = "https://github.com/HackerOS-Linux-System/hammer/issues/new";

    if which("vivaldi").is_ok() {
        Command::new("vivaldi")
            .arg(url)
            .stdin(Stdio::inherit())
            .stdout(Stdio::inherit())
            .stderr(Stdio::inherit())
            .status()?;
    } else if which("xdg-open").is_ok() {
        Command::new("xdg-open")
            .arg(url)
            .stdin(Stdio::inherit())
            .stdout(Stdio::inherit())
            .stderr(Stdio::inherit())
            .status()?;
    } else {
        open::that(url).map_err(|_| HammerError::NoBrowser)?;
    }

    Ok(())
}
