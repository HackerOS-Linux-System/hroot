use anyhow::{Result};
use clap::{Parser, Subcommand};
use hammer_core::{create_spinner, run_command, Logger};
use owo_colors::OwoColorize;

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
    /// Build an ISO image
    Build {
        #[arg(long, default_value = "live-image.iso")]
        output: String,
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
            Logger::success("Build environment initialized.");
        }
        Commands::Build { output } => {
            Logger::info(&format!("Building ISO: {}", output));

            let spinner = create_spinner("Running live-build (this may take a while)...");

            // Running lb build
            // Note: This requires root usually
            run_command("lb", &["build"], "Live Build")?;

            spinner.finish_with_message("Build process finished.");

            if std::path::Path::new("live-image-amd64.hybrid.iso").exists() {
                run_command("mv", &["live-image-amd64.hybrid.iso", &output], "Rename ISO")?;
                Logger::success(&format!("ISO generated at: {}", output.green().bold()));
            } else {
                Logger::error("ISO file was not found after build.");
            }
        }
        Commands::Delta { repo } => {
            Logger::info(&format!("Generating static deltas for repo: {}", repo));

            // ostree static-delta generate --repo=<repo> --inline --min-fallback-size=0
            // --inline: Puts delta data into the commit object for single-request updates
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
