use anyhow::{bail, Context};
use clap::{Args, Parser, Subcommand};
use owo_colors::OwoColorize;
use reqwest::blocking::Client;
use semver::Version;
use std::fs::{self, File};
use std::io::Write;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::process::{Command, Stdio};

const VERSION: &str = "0.9";
const HAMMER_PATH: &str = "/usr/lib/HackerOS/hammer/bin";
const VERSION_FILE: &str = "/usr/lib/hammer/version.hacker";
const REMOTE_VERSION_URL: &str = "https://raw.githubusercontent.com/HackerOS-Linux-System/hammer/main/config/version.hacker";
const RELEASE_BASE_URL: &str = "https://github.com/HackerOS-Linux-System/hammer/releases/download/v";

#[derive(Parser)]
#[command(version, about = "Hammer CLI Tool for HackerOS Atomic")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    Install(InstallArgs),
    Remove(RemoveArgs),
    Update,
    Clean,
    Refresh,
    Build,
    Switch(SwitchArgs),
    Deploy,
    #[command(name = "build-init", alias = "build init")]
    BuildInit,
    About,
    Tui,
    Status,
    History,
    Rollback(RollbackArgs),
    Init,
    Upgrade,
    Issue,
}

#[derive(Args)]
struct InstallArgs {
    #[arg(long)]
    container: bool,

    #[arg(required = true)]
    package: String,
}

#[derive(Args)]
struct RemoveArgs {
    #[arg(long)]
    container: bool,

    #[arg(required = true)]
    package: String,
}

#[derive(Args)]
struct SwitchArgs {
    deployment: Option<String>,
}

#[derive(Args)]
struct RollbackArgs {
    n: Option<String>,
}

fn main() -> anyhow::Result<()> {
    if std::env::args().len() < 2 {
        usage();
        return Ok(());
    }

    let cli = Cli::parse();

    match cli.command {
        Commands::Install(args) => install_command(&args)?,
        Commands::Remove(args) => remove_command(&args)?,
        Commands::Update => update_command()?,
        Commands::Clean => clean_command()?,
        Commands::Refresh => refresh_command()?,
        Commands::Build => build_command()?,
        Commands::Switch(args) => switch_command(&args)?,
        Commands::Deploy => deploy_command()?,
        Commands::BuildInit => build_init_command()?,
        Commands::About => about_command()?,
        Commands::Tui => tui_command()?,
        Commands::Status => status_command()?,
        Commands::History => history_command()?,
        Commands::Rollback(args) => rollback_command(&args)?,
        Commands::Init => init_command()?,
        Commands::Upgrade => upgrade_command()?,
        Commands::Issue => issue_command()?,
    }

    Ok(())
}

fn install_command(args: &InstallArgs) -> anyhow::Result<()> {
    if args.container {
        run_containers("install", vec![&args.package])?;
    } else {
        run_core("install", vec![&args.package])?;
    }
    Ok(())
}

fn remove_command(args: &RemoveArgs) -> anyhow::Result<()> {
    if args.container {
        run_containers("remove", vec![&args.package])?;
    } else {
        run_core("remove", vec![&args.package])?;
    }
    Ok(())
}

fn update_command() -> anyhow::Result<()> {
    run_updater("update", vec![])?;
    Ok(())
}

fn clean_command() -> anyhow::Result<()> {
    run_core("clean", vec![])?;
    Ok(())
}

fn refresh_command() -> anyhow::Result<()> {
    run_core("refresh", vec![])?;
    Ok(())
}

fn build_command() -> anyhow::Result<()> {
    run_builder("build", vec![])?;
    Ok(())
}

fn switch_command(args: &SwitchArgs) -> anyhow::Result<()> {
    let run_args = match &args.deployment {
        Some(d) => vec![d.as_str()],
        None => vec![],
    };
    run_core("switch", run_args)?;
    Ok(())
}

fn deploy_command() -> anyhow::Result<()> {
    run_core("deploy", vec![])?;
    Ok(())
}

fn build_init_command() -> anyhow::Result<()> {
    run_builder("init", vec![])?;
    Ok(())
}

fn about_command() -> anyhow::Result<()> {
    about();
    Ok(())
}

fn tui_command() -> anyhow::Result<()> {
    run_tui(vec![])?;
    Ok(())
}

fn status_command() -> anyhow::Result<()> {
    run_core("status", vec![])?;
    Ok(())
}

fn history_command() -> anyhow::Result<()> {
    run_core("history", vec![])?;
    Ok(())
}

fn rollback_command(args: &RollbackArgs) -> anyhow::Result<()> {
    let n = args.n.as_ref().map_or("1", |s| s.as_str());
    run_core("rollback", vec![n])?;
    Ok(())
}

fn init_command() -> anyhow::Result<()> {
    run_updater("init", vec![])?;
    Ok(())
}

fn upgrade_command() -> anyhow::Result<()> {
    let local_version_str = if Path::new(VERSION_FILE).exists() {
        fs::read_to_string(VERSION_FILE)?
            .trim()
            .replace(['[', ']'], "")
            .trim()
            .to_string()
    } else {
        "0.0".to_string()
    };

    let local_version = Version::parse(&local_version_str).context("Failed to parse local version")?;

    let client = Client::new();
    let response = client
        .get(REMOTE_VERSION_URL)
        .send()
        .context("Failed to fetch remote version")?;

    if !response.status().is_success() {
        bail!("Failed to fetch remote version: {}", response.status());
    }

    let remote_version_str = response
        .text()?
        .trim()
        .replace(['[', ']'], "")
        .trim()
        .to_string();

    let remote_version = Version::parse(&remote_version_str).context("Failed to parse remote version")?;

    if remote_version > local_version {
        println!(
            "{}",
            format!(
                "Upgrading from {} to {}...",
                local_version, remote_version
            )
            .green()
        );

        let binaries = vec![
            ("hammer", "/usr/bin/hammer"),
            ("hammer-updater", &format!("{}/hammer-updater", HAMMER_PATH)),
            ("hammer-core", &format!("{}/hammer-core", HAMMER_PATH)),
            ("hammer-tui", &format!("{}/hammer-tui", HAMMER_PATH)),
            ("hammer-builder", &format!("{}/hammer-builder", HAMMER_PATH)),
            ("hammer-containers", &format!("{}/hammer-containers", HAMMER_PATH)),
        ];

        for (bin_name, bin_path) in binaries {
            let url = format!(
                "{}{}/{}",
                RELEASE_BASE_URL, remote_version_str, bin_name
            );
            let resp = client.get(&url).send().context(format!("Failed to download {}", bin_name))?;

            if !resp.status().is_success() {
                bail!("Failed to download {}: {}", bin_name, resp.status());
            }

            let bytes = resp.bytes().context(format!("Failed to read {} body", bin_name))?;
            let mut file = File::create(bin_path).context(format!("Failed to create file {}", bin_path))?;
            file.write_all(&bytes).context(format!("Failed to write to {}", bin_path))?;

            let mut perms = file.metadata()?.permissions();
            perms.set_mode(0o755);
            fs::set_permissions(bin_path, perms).context(format!("Failed to set permissions for {}", bin_path))?;
        }

        fs::write(VERSION_FILE, format!("[ {} ]", remote_version_str))
            .context("Failed to update version file")?;

        println!("{}", "Upgrade completed.".green());
    } else {
        println!(
            "{}",
            format!("Already up to date (version {}).", local_version).yellow()
        );
    }

    Ok(())
}

fn issue_command() -> anyhow::Result<()> {
    let url = "https://github.com/HackerOS-Linux-System/hammer/issues/new";

    if let Ok(mut child) = Command::new("vivaldi")
        .arg(url)
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .spawn()
    {
        let _ = child.wait();
        Ok(())
    } else if let Ok(mut child) = Command::new("xdg-open")
        .arg(url)
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .spawn()
    {
        let _ = child.wait();
        Ok(())
    } else {
        bail!("Error: No browser found to open the URL. Please install Vivaldi or ensure xdg-open is available.");
    }
}

fn run_core(subcommand: &str, args: Vec<&str>) -> anyhow::Result<()> {
    let binary = format!("{}/hammer-core", HAMMER_PATH);
    execute_command(&binary, subcommand, args)
}

fn run_updater(subcommand: &str, args: Vec<&str>) -> anyhow::Result<()> {
    let binary = format!("{}/hammer-updater", HAMMER_PATH);
    execute_command(&binary, subcommand, args)
}

fn run_builder(subcommand: &str, args: Vec<&str>) -> anyhow::Result<()> {
    let binary = format!("{}/hammer-builder", HAMMER_PATH);
    execute_command(&binary, subcommand, args)
}

fn run_tui(args: Vec<&str>) -> anyhow::Result<()> {
    let binary = format!("{}/hammer-tui", HAMMER_PATH);
    execute_command(&binary, "", args)
}

fn run_containers(subcommand: &str, args: Vec<&str>) -> anyhow::Result<()> {
    let binary = format!("{}/hammer-containers", HAMMER_PATH);
    execute_command(&binary, subcommand, args)
}

fn execute_command(binary: &str, subcommand: &str, args: Vec<&str>) -> anyhow::Result<()> {
    let mut cmd = Command::new(binary);
    if !subcommand.is_empty() {
        cmd.arg(subcommand);
    }
    for arg in args {
        cmd.arg(arg);
    }
    cmd.stdout(Stdio::inherit());
    cmd.stderr(Stdio::inherit());

    let status = cmd
        .status()
        .context(format!("Failed to execute {}", binary))?;

    if !status.success() {
        std::process::exit(status.code().unwrap_or(1));
    }

    Ok(())
}

fn about() {
    println!("{}", "Hammer CLI Tool for HackerOS Atomic".bold().blue());
    println!("{} {}", "Version:".green(), VERSION);
    println!(
        "{} {}",
        "Description:".green(),
        "Tool for managing atomic installations, updates, and builds inspired by apx and rpm-ostree."
    );
    println!("{}", "Components:".green());
    println!("- {} {}", "hammer-core:".yellow(), "Core operations in Crystal");
    println!("- {} {}", "hammer-updater:".yellow(), "System updater in Crystal");
    println!("- {} {}", "hammer-builder:".yellow(), "ISO builder in Crystal");
    println!("- {} {}", "hammer-tui:".yellow(), "TUI interface in Go with Bubble Tea");
    println!("{} {}", "Location:".green(), HAMMER_PATH);
}

fn usage() {
    println!("{}", "Usage: hammer <command> [options]".bold().blue());
    println!();
    println!("{}", "Commands:".green());
    println!(
        " {} {}",
        "install [--container] <package>".yellow(),
        "Install a package (optionally in container)"
    );
    println!(
        " {} {}",
        "remove [--container] <package>".yellow(),
        "Remove a package (optionally from container)"
    );
    println!(" {} {}", "update".yellow(), "Update the system atomically");
    println!(" {} {}", "clean".yellow(), "Clean up unused resources");
    println!(" {} {}", "refresh".yellow(), "Refresh repositories");
    println!(
        " {} {}",
        "build".yellow(),
        "Build atomic ISO (must be in project dir)"
    );
    println!(
        " {} {}",
        "switch [deployment]".yellow(),
        "Switch to a deployment (rollback if no arg)"
    );
    println!(" {} {}", "deploy".yellow(), "Create a new deployment");
    println!(" {} {}", "build init".yellow(), "Initialize build project");
    println!(" {} {}", "about".yellow(), "Show tool information");
    println!(" {} {}", "tui".yellow(), "Launch TUI interface");
    println!(
        " {} {}",
        "status".yellow(),
        "Show current deployment status"
    );
    println!(
        " {} {}",
        "history".yellow(),
        "Show deployment history"
    );
    println!(
        " {} {}",
        "rollback [n]".yellow(),
        "Rollback n steps (default 1)"
    );
    println!(
        " {} {}",
        "init".yellow(),
        "Initialize the atomic system (linking without update)"
    );
    println!(" {} {}", "upgrade".yellow(), "Upgrade the hammer tool");
    println!(
        " {} {}",
        "issue".yellow(),
        "Open new issue in GitHub repository"
    );
}
