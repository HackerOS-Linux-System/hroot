use clap::{Arg, Command, ArgMatches};
use std::process::{Command as SysCommand, Stdio};
use std::io::{self, Write};
use std::fs;
use std::path::Path;
use std::error::Error;

// Constants
const CONTAINER_TOOL: &str = "podman"; // Assuming podman for container management, like distrobox
const CONTAINER_NAME_PREFIX: &str = "hammer-container-";
const BTRFS_SUBVOL_ROOT: &str = "/"; // Assuming root is on BTRFS
const SNAPSHOT_DIR: &str = "/.snapshots"; // Common BTRFS snapshot dir

fn main() -> Result<(), Box<dyn Error>> {
    let matches = Command::new("hammer-core")
    .version("0.1.0")
    .author("HackerOS Team")
    .about("Core operations for Hammer tool in HackerOS Atomic")
    .subcommand(
        Command::new("install")
        .about("Install a package in a container")
        .arg(Arg::new("package").required(true).index(1)),
    )
    .subcommand(
        Command::new("remove")
        .about("Remove a package from a container")
        .arg(Arg::new("package").required(true).index(1)),
    )
    .subcommand(
        Command::new("snapshot")
        .about("Create a BTRFS snapshot"),
    )
    .subcommand(
        Command::new("back")
        .about("Rollback to previous system version via snapshot"),
    )
    .subcommand(
        Command::new("clean")
        .about("Clean up unused containers and snapshots"),
    )
    .subcommand(
        Command::new("refresh")
        .about("Refresh container metadata or repos"),
    )
    .get_matches();

    match matches.subcommand() {
        Some(("install", sub_matches)) => install_package(sub_matches)?,
        Some(("remove", sub_matches)) => remove_package(sub_matches)?,
        Some(("snapshot", _)) => create_snapshot()?,
        Some(("back", _)) => rollback_snapshot()?,
        Some(("clean", _)) => clean_up()?,
        Some(("refresh", _)) => refresh()?,
        _ => println!("No subcommand was used"),
    }

    Ok(())
}

fn install_package(matches: &ArgMatches) -> Result<(), Box<dyn Error>> {
    let package = matches.get_one::<String>("package").unwrap();
    println!("Installing package: {}", package);

    // Create or use a container for the distro (e.g., assuming a default Fedora-like container)
    let container_name = format!("{}{}", CONTAINER_NAME_PREFIX, "default");
    ensure_container_exists(&container_name)?;

    // Install package inside container (assuming dnf for Fedora-like)
    let output = SysCommand::new(CONTAINER_TOOL)
    .args(&["exec", "-it", &container_name, "dnf", "install", "-y", package])
    .output()?;

    if !output.status.success() {
        return Err(format!("Failed to install package: {}", String::from_utf8_lossy(&output.stderr)).into());
    }

    // Export binary to host if needed (simplified)
    export_binaries_from_container(&container_name, package)?;

    println!("Package {} installed successfully.", package);
    Ok(())
}

fn remove_package(matches: &ArgMatches) -> Result<(), Box<dyn Error>> {
    let package = matches.get_one::<String>("package").unwrap();
    println!("Removing package: {}", package);

    let container_name = format!("{}{}", CONTAINER_NAME_PREFIX, "default");
    ensure_container_exists(&container_name)?;

    let output = SysCommand::new(CONTAINER_TOOL)
    .args(&["exec", "-it", &container_name, "dnf", "remove", "-y", package])
    .output()?;

    if !output.status.success() {
        return Err(format!("Failed to remove package: {}", String::from_utf8_lossy(&output.stderr)).into());
    }

    println!("Package {} removed successfully.", package);
    Ok(())
}

fn create_snapshot() -> Result<(), Box<dyn Error>> {
    println!("Creating BTRFS snapshot...");

    // Ensure snapshot dir exists
    fs::create_dir_all(SNAPSHOT_DIR)?;

    // Get current timestamp for snapshot name
    let timestamp = chrono::Local::now().format("%Y%m%d_%H%M%S").to_string();
    let snapshot_path = format!("{}/hammer_snapshot_{}", SNAPSHOT_DIR, timestamp);

    // Create read-only snapshot
    let output = SysCommand::new("btrfs")
    .args(&["subvolume", "snapshot", "-r", BTRFS_SUBVOL_ROOT, &snapshot_path])
    .output()?;

    if !output.status.success() {
        return Err(format!("Failed to create snapshot: {}", String::from_utf8_lossy(&output.stderr)).into());
    }

    println!("Snapshot created at: {}", snapshot_path);
    Ok(())
}

fn rollback_snapshot() -> Result<(), Box<dyn Error>> {
    println!("Rolling back to previous snapshot...");

    // Find the latest snapshot (simplified: assume we list and pick the last one)
    let snapshots = get_snapshots()?;
    if snapshots.is_empty() {
        return Err("No snapshots available for rollback.".into());
    }

    let latest_snapshot = snapshots.last().unwrap();
    println!("Rolling back to: {}", latest_snapshot);

    // Set the snapshot as default (make it the new root)
    let output = SysCommand::new("btrfs")
    .args(&["subvolume", "set-default", latest_snapshot])
    .output()?;

    if !output.status.success() {
        return Err(format!("Failed to set default subvolume: {}", String::from_utf8_lossy(&output.stderr)).into());
    }

    // Note: Reboot might be required, but we can't handle that here
    println!("Rollback set. Reboot the system to apply.");
    Ok(())
}

fn clean_up() -> Result<(), Box<dyn Error>> {
    println!("Cleaning up unused resources...");

    // Clean unused containers
    let _ = SysCommand::new(CONTAINER_TOOL)
    .args(&["system", "prune", "-f"])
    .output()?;

    // Clean old snapshots (keep last 5, simplified)
    let mut snapshots = get_snapshots()?;
    snapshots.sort();
    if snapshots.len() > 5 {
        for snap in snapshots.iter().take(snapshots.len() - 5) {
            let output = SysCommand::new("btrfs")
            .args(&["subvolume", "delete", snap])
            .output()?;
            if !output.status.success() {
                eprintln!("Failed to delete snapshot {}: {}", snap, String::from_utf8_lossy(&output.stderr));
            }
        }
    }

    println!("Clean up completed.");
    Ok(())
}

fn refresh() -> Result<(), Box<dyn Error>> {
    println!("Refreshing container metadata...");

    let container_name = format!("{}{}", CONTAINER_NAME_PREFIX, "default");
    ensure_container_exists(&container_name)?;

    // Assuming dnf update metadata
    let output = SysCommand::new(CONTAINER_TOOL)
    .args(&["exec", "-it", &container_name, "dnf", "makecache"])
    .output()?;

    if !output.status.success() {
        return Err(format!("Failed to refresh: {}", String::from_utf8_lossy(&output.stderr)).into());
    }

    println!("Refresh completed.");
    Ok(())
}

// Helper functions

fn ensure_container_exists(container_name: &str) -> Result<(), Box<dyn Error>> {
    let status = SysCommand::new(CONTAINER_TOOL)
    .args(&["ps", "-a", "-f", &format!("name={}", container_name)])
    .status()?;

    if !status.success() {
        // Create container if not exists (assuming Fedora image)
        let output = SysCommand::new(CONTAINER_TOOL)
        .args(&["run", "-d", "--name", container_name, "fedora:latest", "sleep", "infinity"])
        .output()?;

        if !output.status.success() {
            return Err(format!("Failed to create container: {}", String::from_utf8_lossy(&output.stderr)).into());
        }
    }
    Ok(())
}

fn export_binaries_from_container(container_name: &str, package: &str) -> Result<(), Box<dyn Error>> {
    // Simplified: assume we copy /usr/bin/* from container to host ~/.hackeros/bin or something
    // In reality, this would be more selective
    let host_bin_dir = Path::new("/home/user/.local/bin"); // Adjust as needed
    fs::create_dir_all(host_bin_dir)?;

    // This is placeholder; in practice, identify binaries from package
    let _ = SysCommand::new(CONTAINER_TOOL)
    .args(&["cp", &format!("{}:/usr/bin/{}", container_name, package), host_bin_dir.to_str().unwrap()])
    .output()?;

    Ok(())
}

fn get_snapshots() -> Result<Vec<String>, Box<dyn Error>> {
    let output = SysCommand::new("ls")
    .arg(SNAPSHOT_DIR)
    .output()?;

    if !output.status.success() {
        return Err("Failed to list snapshots.".into());
    }

    let snapshots: Vec<String> = String::from_utf8_lossy(&output.stdout)
    .lines()
    .filter(|line| line.starts_with("hammer_snapshot_"))
    .map(|line| format!("{}/{}", SNAPSHOT_DIR, line.to_string()))
    .collect();

    Ok(snapshots)
}

