use anyhow::{Result};
use clap::{Parser, Subcommand};
use hammer_core::{create_spinner, run_command, Logger};
use owo_colors::OwoColorize;
use nix::unistd::Uid;
use std::path::{Path, PathBuf};
use std::fs;

#[derive(Parser)]
#[command(name = "hammer-builder")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Initialize a build directory
    Init,
    /// Build an ISO image using live-build
    Build {
        /// Name of the output ISO file
        #[arg(long, default_value = "live-image.iso")]
        output: String,

        /// Path to source configuration directory (will be copied to ./config)
        #[arg(long)]
        config: Option<String>,
    },
    /// Generate static deltas for OSTree repository
    Delta {
        /// Path to OSTree repository
        #[arg(long, default_value = "/ostree/repo")]
        repo: String,
    },
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    
    match cli.command {
        Commands::Init => {
            Logger::info("Initializing build environment...");
            // Create lb config
            run_command("lb", &["config"], "Live Build Config")?;
            Logger::success("Build environment initialized. Edit ./config to customize.");
        }
        Commands::Build { output, config } => {
            require_root()?;
            Logger::section("BUILDING LIVE ISO");

            // 1. Handle Configuration
            if let Some(cfg_path) = config {
                let src_path = PathBuf::from(&cfg_path);
                let dest_path = PathBuf::from("config");

                if !src_path.exists() {
                    Logger::error(&format!("Config path does not exist: {}", cfg_path));
                    std::process::exit(1);
                }

                Logger::info(&format!("Using custom config from: {}", cfg_path.cyan()));

                // Clean existing config to avoid mixing
                if dest_path.exists() {
                    Logger::info("Removing old ./config...");
                    fs::remove_dir_all(&dest_path)?;
                }

                // Copy new config
                // Using cp -r is safer/easier than recursive fs::copy implementation
                run_command("cp", &["-r", cfg_path.as_str(), "config"], "Copy Config")?;
            }

            if !Path::new("config").exists() {
                Logger::warn("No ./config directory found. Running default 'lb config'...");
                run_command("lb", &["config"], "Default Config")?;
            }

            // 2. Clean previous build artifacts
            let clean_spinner = create_spinner("Cleaning previous build environment...");
            run_command("lb", &["clean"], "Live Build Clean")?;
            clean_spinner.finish_with_message("Environment cleaned.");

            // 3. Build
            Logger::info("Starting build process. This may take a long time...");
            let build_start = std::time::Instant::now();
            
            // Run lb build
            // streaming output to stdout so user sees progress of apt/bootstrap
            let status = std::process::Command::new("lb")
                .arg("build")
                .stdout(std::process::Stdio::inherit())
                .stderr(std::process::Stdio::inherit())
                .status()?;

            if !status.success() {
                Logger::error("Live Build failed.");
                std::process::exit(1);
            }

            // 4. Handle Output
            let duration = build_start.elapsed();
            Logger::info(&format!("Build finished in {:.2?}.", duration));

            // live-build usually outputs live-image-amd64.hybrid.iso (depends on arch)
            // We look for any .iso file created recently or specific names
            let possible_names = vec![
                "live-image-amd64.hybrid.iso",
                "live-image-amd64.iso",
                "live-image-i386.hybrid.iso"
            ];

            let mut found = false;
            for name in possible_names {
                if Path::new(name).exists() {
                    run_command("mv", &[name, &output], "Rename ISO")?;
                    found = true;
                    break;
                }
            }

            if found {
                Logger::success(&format!("ISO generated successfully: {}", output.green().bold()));
            } else {
                Logger::warn("Build command succeeded, but could not auto-detect output ISO to rename.");
                Logger::warn("Check the current directory for the generated file.");
            }
            Logger::end_section();
        }
        Commands::Delta { repo } => {
            Logger::info(&format!("Generating static deltas for repo: {}", repo));
            
            let spinner = create_spinner("Calculating deltas...");
            
            run_command("ostree", &[
                "static-delta", 
                "generate", 
                "--repo", &repo,
                "--inline",
                "--min-fallback-size=0" 
            ], "OSTree Delta Generation")?;
            
            spinner.finish_with_message("Deltas generated.");
            Logger::success("Repository optimized with static deltas.");
        }
    }

    Ok(())
}

fn require_root() -> Result<()> {
    if !Uid::current().is_root() {
        Logger::error("Permission denied. Building a live image requires root privileges.");
        Logger::info(&format!("Try: sudo hammer-builder build ..."));
        std::process::exit(1);
    }
    Ok(())
}
