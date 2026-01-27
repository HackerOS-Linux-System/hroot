use clap::{Parser, Subcommand};
use nix::unistd::Uid;
use std::process::Command;
use anyhow::{Result, anyhow, Context};

#[derive(Parser)]
#[command(name = "hammer-read")]
#[command(about = "Manage filesystem read-only status for Hammer")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Mount the root filesystem as Read-Only (Lock)
    Lock,
    /// Mount the root filesystem as Read-Write (Unlock)
    Unlock,
}

fn main() -> Result<()> {
    if !Uid::current().is_root() {
        return Err(anyhow!("This tool must be run as root."));
    }

    let cli = Cli::parse();

    match cli.command {
        Commands::Lock => set_filesystem_state(true),
        Commands::Unlock => set_filesystem_state(false),
    }
}

fn set_filesystem_state(readonly: bool) -> Result<()> {
    let state_str = if readonly { "read-only (locked)" } else { "read-write (unlocked)" };
    println!("Setting root filesystem to {}...", state_str);

    // 1. Remount / as ro/rw
    let mount_opt = if readonly { "remount,ro" } else { "remount,rw" };
    
    let status = Command::new("mount")
        .args(["-o", mount_opt, "/"])
        .status()
        .context("Failed to execute mount command")?;

    if !status.success() {
        return Err(anyhow!("Failed to remount / as {}", mount_opt));
    }

    // 2. If locking, ensure the btrfs property is also set if possible, 
    // though 'mount -o remount,ro' is usually sufficient for runtime.
    // For unlocking, we MUST ensure the BTRFS property permits writing if we are on BTRFS.
    if !readonly {
        // Attempt to set BTRFS property to ro=false just in case
        let _ = Command::new("btrfs")
            .args(["property", "set", "-ts", "/", "ro", "false"])
            .status(); 
    }

    println!("System is now {}.", state_str);
    Ok(())
}

