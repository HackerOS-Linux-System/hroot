use miette::{IntoDiagnostic, Result};
use clap::{Parser, Subcommand};
use hammer_core::{
    btrfs_delete_atomic_snapshot, btrfs_list_atomic_snapshots, btrfs_snapshot_atomic,
    create_spinner, create_progress_bar, run_command, Logger,
};
use owo_colors::OwoColorize;
use dialoguer::{Select, Confirm};
use std::process::{Command, Stdio};
use indicatif::ProgressBar;

#[derive(Parser)]
#[command(name = "hammer-updater")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    Update,
    Layer { packages: Vec<String> },
    Clean,
    Rollback,
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Commands::Update => handle_update()?,
        Commands::Layer { packages } => handle_layer(packages)?,
        Commands::Clean => handle_clean()?,
        Commands::Rollback => handle_rollback()?,
    }
    Ok(())
}

fn create_snapshot_name(suffix: &str) -> String {
    let timestamp = chrono::Local::now().format("%Y-%m-%d-%H%M%S");
    format!("{}-{}", timestamp, suffix)
}

fn handle_update() -> Result<()> {
    Logger::section("ATOMIC SYSTEM UPDATE");

    // Initialize global progress bar for steps
    let steps = 4;
    let main_pb = create_progress_bar(steps, "Initializing...");

    // Step 1: Prep
    main_pb.set_message("Step 1/4: Preparing Filesystem...");
    main_pb.set_position(1);

    // Ensure RW
    Logger::info("Remounting Root as RW...");
    run_command("mount", &["-o", "remount,rw", "/"], "Remount RW")?;

    // Step 2: Snapshot
    main_pb.set_message("Step 2/4: Creating Snapshot...");
    main_pb.set_position(2);

    let snap_name = create_snapshot_name("pre-update");
    let spinner = create_spinner("Snapshotting @ subvolume...");
    btrfs_snapshot_atomic(&snap_name)?;
    spinner.finish_with_message("Snapshot created in @snapshots");

    // Step 3: APT Update
    main_pb.set_message("Step 3/4: Downloading Updates...");
    main_pb.set_position(3);

    Logger::info("Running apt update & upgrade (Logs below)...");

    // We pause the main PB briefly or let logs flow under it?
    // indicatif output handles this if configured, but mixing streams is hard.
    // We will just let logs print.

    let status = Command::new("apt")
    .args(&["update"])
    .stdout(Stdio::inherit())
    .stderr(Stdio::inherit())
    .status()
    .into_diagnostic()?;

    if !status.success() {
        Logger::error("apt update failed.");
        return Ok(());
    }

    let status = Command::new("apt")
    .args(&["full-upgrade", "-y"])
    .stdout(Stdio::inherit())
    .stderr(Stdio::inherit())
    .status()
    .into_diagnostic()?;

    if status.success() {
        // Step 4: Finalize
        main_pb.set_message("Step 4/4: Finalizing...");
        main_pb.set_position(4);

        run_command("sync", &[], "Sync Filesystem")?;

        main_pb.finish_with_message("Update Complete!");
        Logger::success("System successfully updated.");
    } else {
        main_pb.abandon_with_message("Update Failed");
        Logger::error("APT Upgrade failed.");

        if Confirm::new().with_prompt("Rollback now?").interact().into_diagnostic()? {
            // Rollback logic here (complex on live system)
            Logger::warn("Please run 'hammer rollback' or select snapshot at boot.");
        }
    }

    Logger::end_section();
    Ok(())
}

fn handle_layer(packages: Vec<String>) -> Result<()> {
    if packages.is_empty() { return Ok(()); }

    Logger::section("PACKAGE LAYERING");
    run_command("mount", &["-o", "remount,rw", "/"], "Remount RW")?;

    let snap_name = create_snapshot_name("pre-layer");
    let spinner = create_spinner("Safety Snapshot...");
    btrfs_snapshot_atomic(&snap_name)?;
    spinner.finish_with_message("Snapshot created.");

    let mut args = vec!["install", "-y"];
    let pkgs_refs: Vec<&str> = packages.iter().map(|s| s.as_str()).collect();
    args.extend(pkgs_refs);

    let status = Command::new("apt")
    .args(&args)
    .stdin(Stdio::inherit())
    .stdout(Stdio::inherit())
    .stderr(Stdio::inherit())
    .status()
    .into_diagnostic()?;

    if status.success() {
        run_command("sync", &[], "Sync")?;
        Logger::success("Layer applied.");
    } else {
        Logger::error("Failed.");
    }
    Logger::end_section();
    Ok(())
}

fn handle_clean() -> Result<()> {
    Logger::section("CLEANING SNAPSHOTS");
    let snapshots = btrfs_list_atomic_snapshots()?;

    if snapshots.len() <= 3 {
        Logger::info("Nothing to clean.");
    } else {
        let to_delete = &snapshots[0..(snapshots.len() - 3)];
        for snap in to_delete {
            Logger::info(&format!("Deleting {}", snap));
            btrfs_delete_atomic_snapshot(snap)?;
        }
        Logger::success("Cleanup done.");
    }
    Logger::end_section();
    Ok(())
}

fn handle_rollback() -> Result<()> {
    Logger::section("SYSTEM ROLLBACK");
    let snapshots = btrfs_list_atomic_snapshots()?;

    if snapshots.is_empty() {
        Logger::error("No snapshots found in @snapshots.");
        return Ok(());
    }

    let selection = Select::new()
    .with_prompt("Select snapshot to restore")
    .items(&snapshots)
    .default(snapshots.len() - 1)
    .interact()
    .into_diagnostic()?;

    let target = &snapshots[selection];

    Logger::warn(&format!("Target: {}", target.yellow()));
    Logger::warn("To restore: The system will rename current '@' to '@bad-date' and restore snapshot to '@'.");
    Logger::warn("REBOOT IS REQUIRED IMMEDIATELY AFTER.");

    if Confirm::new().with_prompt("Proceed?").interact().into_diagnostic()? {
        use hammer_core::{mount_btrfs_root, umount_btrfs_root, MOUNT_POINT};
        use std::path::Path;

        let spinner = create_spinner("Performing rollback...");
        mount_btrfs_root()?;

        // 1. Rename current @
        let timestamp = chrono::Local::now().format("%Y%m%d-%H%M%S");
        let bad_name = format!("@bad-{}", timestamp);
        let root = Path::new(MOUNT_POINT);

        run_command("mv", &[
            &root.join("@").to_string_lossy(),
                    &root.join(&bad_name).to_string_lossy()
        ], "Rename current @")?;

        // 2. Snapshot target to @
        let snap_src = root.join("@snapshots").join(target);
        let new_root = root.join("@");

        run_command("btrfs", &[
            "subvolume", "snapshot",
            &snap_src.to_string_lossy(),
                    &new_root.to_string_lossy()
        ], "Restore Snapshot to @")?;

        umount_btrfs_root()?;
        spinner.finish_with_message("Rollback applied.");

        Logger::success("Rollback successful. Please REBOOT now.");
    }

    Logger::end_section();
    Ok(())
}
