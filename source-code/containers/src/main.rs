use miette::{IntoDiagnostic, Result};
use clap::{Parser, Subcommand};
use hammer_core::{create_spinner, run_command, Logger};
use owo_colors::OwoColorize;
use dialoguer::{Select, Input, Confirm};
use std::fs;
use std::path::Path;
use std::os::unix::fs::PermissionsExt;

#[derive(Parser)]
#[command(name = "hammer-containers")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Install an application inside the hammer-box container
    Install {
        package: String,
    },
    /// Remove an application wrapper
    Remove {
        package: String,
    },
    /// List installed wrappers
    List,
}

const CONTAINER_NAME: &str = "hammer-box";
const CONTAINER_IMAGE: &str = "docker.io/library/debian:bookworm";
const WRAPPER_DIR: &str = "/usr/local/bin";
const DESKTOP_DIR: &str = "/usr/share/applications";

fn main() -> Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Commands::Install { package } => handle_install(package)?,
        Commands::Remove { package } => handle_remove(package)?,
        Commands::List => handle_list()?,
    }

    Ok(())
}

fn ensure_container_exists() -> Result<()> {
    let output = run_command("podman", &["ps", "-a", "--format", "{{.Names}}"], "Check Container")?;

    if !output.contains(CONTAINER_NAME) {
        Logger::info("Initializing hammer-box container environment...");
        let spinner = create_spinner("Pulling base image & Creating container...");

        // Create an infinite loop container that we can exec into
        run_command("podman", &[
            "run", "-d",
            "--name", CONTAINER_NAME,
            "--restart", "always",
            // Share networking and X11 for GUI apps
            "--net=host",
            "-v", "/tmp/.X11-unix:/tmp/.X11-unix",
            "-e", "DISPLAY",
            "-e", "WAYLAND_DISPLAY",
            "-e", "XDG_RUNTIME_DIR",
            CONTAINER_IMAGE,
            "sleep", "infinity"
        ], "Create Container")?;

        // Update apt inside
        run_command("podman", &["exec", CONTAINER_NAME, "apt-get", "update"], "Update Container APT")?;

        spinner.finish_with_message("Container environment ready.");
    } else {
        // Ensure it's running
        run_command("podman", &["start", CONTAINER_NAME], "Start Container")?;
    }
    Ok(())
}

fn handle_install(package: String) -> Result<()> {
    ensure_container_exists()?;

    Logger::info(&format!("Installing {} in container...", package.cyan()));

    // Install in container
    let status = std::process::Command::new("podman")
    .args(&["exec", "-it", CONTAINER_NAME, "apt-get", "install", "-y", &package])
    .status()
    .into_diagnostic()?;

    if !status.success() {
        Logger::error("Failed to install package in container.");
        return Ok(());
    }

    // Determine App Type
    let types = vec!["CLI (Command Line Tool)", "GUI (Desktop Application)"];
    let selection = Select::new()
    .with_prompt("What type of application is this?")
    .items(&types)
    .default(0)
    .interact()
    .into_diagnostic()?;

    let bin_name: String = Input::new()
    .with_prompt("Enter the command name to launch it (e.g. alacritty)")
    .with_initial_text(&package)
    .interact_text()
    .into_diagnostic()?;

    if selection == 0 {
        // CLI
        create_cli_wrapper(&bin_name, &bin_name)?;
    } else {
        // GUI
        create_gui_wrapper(&bin_name, &bin_name)?;
    }

    Ok(())
}

fn create_cli_wrapper(wrapper_name: &str, inner_cmd: &str) -> Result<()> {
    let wrapper_path = Path::new(WRAPPER_DIR).join(wrapper_name);

    let content = format!(r#"#!/bin/bash
    exec podman exec -it {} {} "$@"
    "#, CONTAINER_NAME, inner_cmd);

    fs::write(&wrapper_path, content).into_diagnostic()?;

    let mut perms = fs::metadata(&wrapper_path).into_diagnostic()?.permissions();
    perms.set_mode(0o755);
    fs::set_permissions(&wrapper_path, perms).into_diagnostic()?;

    Logger::success(&format!("CLI wrapper created at {}", wrapper_path.display()));
    Ok(())
}

fn create_gui_wrapper(wrapper_name: &str, inner_cmd: &str) -> Result<()> {
    // 1. Create binary wrapper to launch it
    let bin_wrapper_path = Path::new(WRAPPER_DIR).join(wrapper_name);
    let bin_content = format!(r#"#!/bin/bash
    # Pass X11/Wayland vars
    xhost +local:root > /dev/null 2>&1
    exec podman exec -e DISPLAY=$DISPLAY -e XDG_RUNTIME_DIR=$XDG_RUNTIME_DIR {} {} "$@"
    "#, CONTAINER_NAME, inner_cmd);

    fs::write(&bin_wrapper_path, bin_content).into_diagnostic()?;
    let mut perms = fs::metadata(&bin_wrapper_path).into_diagnostic()?.permissions();
    perms.set_mode(0o755);
    fs::set_permissions(&bin_wrapper_path, perms).into_diagnostic()?;

    // 2. Create .desktop file
    let desktop_path = Path::new(DESKTOP_DIR).join(format!("{}.desktop", wrapper_name));
    let desktop_content = format!(r#"[Desktop Entry]
    Name={} (Container)
    Exec={}
    Type=Application
    Categories=Utility;Application;
    Terminal=false
    "#, wrapper_name, bin_wrapper_path.display());

    fs::write(&desktop_path, desktop_content).into_diagnostic()?;

    Logger::success(&format!("GUI installed. Wrapper: {}, Desktop: {}", bin_wrapper_path.display(), desktop_path.display()));
    Ok(())
}

fn handle_remove(package: String) -> Result<()> {
    // Remove wrapper
    let wrapper_path = Path::new(WRAPPER_DIR).join(&package);
    if wrapper_path.exists() {
        fs::remove_file(wrapper_path).into_diagnostic()?;
        Logger::success(&format!("Removed binary wrapper for {}", package));
    }

    let desktop_path = Path::new(DESKTOP_DIR).join(format!("{}.desktop", package));
    if desktop_path.exists() {
        fs::remove_file(desktop_path).into_diagnostic()?;
        Logger::success("Removed .desktop file");
    }

    // Optional: Remove from container
    if Confirm::new().with_prompt("Uninstall from container as well?").interact().into_diagnostic()? {
        run_command("podman", &["exec", CONTAINER_NAME, "apt-get", "remove", "-y", &package], "Apt Remove")?;
    }

    Ok(())
}

fn handle_list() -> Result<()> {
    Logger::info("Installed container wrappers:");
    for entry in fs::read_dir(WRAPPER_DIR).into_diagnostic()? {
        let entry = entry.into_diagnostic()?;
        let path = entry.path();
        if path.is_file() {
            let content = fs::read_to_string(&path).unwrap_or_default();
            if content.contains("podman exec") {
                println!(" - {}", path.file_name().unwrap().to_string_lossy().cyan());
            }
        }
    }
    Ok(())
}
