use miette::{Diagnostic, IntoDiagnostic, Result, WrapErr};
use indicatif::{ProgressBar, ProgressStyle};
use owo_colors::OwoColorize;
use std::fs::{self, OpenOptions};
use std::io::{Write};
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::Duration;
use thiserror::Error;

pub const LOG_DIR: &str = "/var/log/hammer";
pub const MOUNT_POINT: &str = "/run/hammer/btrfs-root";

#[derive(Error, Debug, Diagnostic)]
pub enum HammerError {
    #[error("Command failed: {0}")]
    #[diagnostic(code(hammer::command_failed), help("Check the output log for details."))]
    CommandFailed(String),

    #[error("IO Error: {0}")]
    #[diagnostic(code(hammer::io_error))]
    IoError(String),

    #[error("Configuration Error: {0}")]
    #[diagnostic(code(hammer::config_error))]
    ConfigError(String),

    #[error("Btrfs Error: {0}")]
    #[diagnostic(code(hammer::btrfs_error), help("Ensure / is a Btrfs subvolume and layout uses @."))]
    BtrfsError(String),
}

pub struct Logger;

impl Logger {
    pub fn init() -> Result<()> {
        if !Path::new(LOG_DIR).exists() {
            fs::create_dir_all(LOG_DIR).into_diagnostic()?;
        }
        Ok(())
    }

    pub fn log(message: &str) {
        let timestamp = chrono::Local::now().format("%Y-%m-%d %H:%M:%S");
        let log_line = format!("[{}] {}\n", timestamp, message);

        let log_file = Path::new(LOG_DIR).join("hammer.log");
        if let Ok(mut file) = OpenOptions::new().create(true).append(true).open(log_file) {
            let _ = file.write_all(log_line.as_bytes());
        }
    }

    pub fn info(message: &str) {
        println!(" {} {}", "│".blue(), message);
        Self::log(&format!("INFO: {}", message));
    }

    pub fn section(title: &str) {
        println!("\n{} {}", "┌──".magenta(), title.magenta().bold());
    }

    pub fn end_section() {
        println!("{}", "└──".magenta());
    }

    pub fn error(message: &str) {
        eprintln!(" {} {}", "✖".red(), message.red());
        Self::log(&format!("ERROR: {}", message));
    }

    pub fn success(message: &str) {
        println!(" {} {}", "✓".green(), message.green());
        Self::log(&format!("SUCCESS: {}", message));
    }

    pub fn warn(message: &str) {
        println!(" {} {}", "!".yellow(), message.yellow());
        Self::log(&format!("WARN: {}", message));
    }
}

pub fn create_progress_bar(len: u64, msg: &str) -> ProgressBar {
    let pb = ProgressBar::new(len);
    pb.set_style(
        ProgressStyle::default_bar()
        .template("{spinner:.cyan} [{elapsed_precise}] [{bar:40.cyan/blue}] {pos}/{len} {msg}")
        .unwrap()
        .progress_chars("=>-"),
    );
    pb.set_message(msg.to_string());
    pb
}

pub fn create_spinner(msg: &str) -> ProgressBar {
    let pb = ProgressBar::new_spinner();
    pb.set_style(
        ProgressStyle::default_spinner()
        .tick_strings(&["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"])
        .template("{spinner:.cyan} {msg}")
        .unwrap(),
    );
    pb.set_message(msg.to_string());
    pb.enable_steady_tick(Duration::from_millis(80));
    pb
}

pub fn run_command(cmd: &str, args: &[&str], description: &str) -> Result<String> {
    Logger::log(&format!("Running: {} {}", cmd, args.join(" ")));

    let output = Command::new(cmd)
    .args(args)
    .stdout(Stdio::piped())
    .stderr(Stdio::piped())
    .output()
    .into_diagnostic()
    .wrap_err(format!("Failed to execute binary: {}", cmd))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).to_string();
        Logger::log(&format!("Command failed stderr: {}", stderr));
        return Err(HammerError::CommandFailed(format!("{} failed: {}", description, stderr)).into());
    }

    Ok(String::from_utf8_lossy(&output.stdout).to_string())
}

// --- Btrfs Helpers ---

/// Mounts the top-level Btrfs root (ID 5) to a temporary location
pub fn mount_btrfs_root() -> Result<String> {
    if !Path::new(MOUNT_POINT).exists() {
        fs::create_dir_all(MOUNT_POINT).into_diagnostic()?;
    }

    // Identify the device / is mounted on
    let output = run_command("findmnt", &["-n", "-o", "SOURCE", "/"], "Find Root Device")?;

    // Fix: findmnt often returns "/dev/sda2[/@]" or similar.
    // We need just "/dev/sda2" for the mount command.
    let device_raw = output.trim();
    let device = device_raw.split('[').next().unwrap_or(device_raw);

    Logger::info(&format!("Detected root device: {}", device));

    // Mount subvolid=5
    let status = Command::new("mount")
    .args(&["-t", "btrfs", "-o", "subvolid=5", device, MOUNT_POINT])
    .output()
    .into_diagnostic()?;

    if !status.status.success() {
        // Check if already mounted
        let check = run_command("mount", &[], "Check mounts")?;
        if check.contains(MOUNT_POINT) {
            return Ok(MOUNT_POINT.to_string());
        }
        return Err(HammerError::BtrfsError("Failed to mount Btrfs top-level root".into()).into());
    }

    Ok(MOUNT_POINT.to_string())
}

pub fn umount_btrfs_root() -> Result<()> {
    // Attempt unmount, but don't fail hard if it fails (it might be lazy unmounted later by OS)
    let _ = run_command("umount", &[MOUNT_POINT], "Unmount Btrfs Root");
    Ok(())
}

pub fn btrfs_snapshot_atomic(name: &str) -> Result<()> {
    // Requires @ layout
    mount_btrfs_root()?;

    let root_subvol = Path::new(MOUNT_POINT).join("@");
    let snap_dir = Path::new(MOUNT_POINT).join("@snapshots");
    let snap_target = snap_dir.join(name);

    if !root_subvol.exists() {
        umount_btrfs_root()?;
        return Err(HammerError::BtrfsError("Subvolume @ not found. Hammer requires @ layout.".into()).into());
    }

    if !snap_dir.exists() {
        fs::create_dir_all(&snap_dir).into_diagnostic()?;
    }

    let src = root_subvol.to_string_lossy();
    let dest = snap_target.to_string_lossy();

    run_command("btrfs", &["subvolume", "snapshot", &src, &dest], "Create Snapshot")?;

    umount_btrfs_root()?;
    Ok(())
}

pub fn btrfs_list_atomic_snapshots() -> Result<Vec<String>> {
    mount_btrfs_root()?;
    let snap_dir = Path::new(MOUNT_POINT).join("@snapshots");

    let mut snaps = Vec::new();
    if snap_dir.exists() {
        for entry in fs::read_dir(snap_dir).into_diagnostic()? {
            let entry = entry.into_diagnostic()?;
            snaps.push(entry.file_name().to_string_lossy().to_string());
        }
    }

    umount_btrfs_root()?;
    snaps.sort();
    Ok(snaps)
}

pub fn btrfs_delete_atomic_snapshot(name: &str) -> Result<()> {
    mount_btrfs_root()?;
    let snap_path = Path::new(MOUNT_POINT).join("@snapshots").join(name);

    if snap_path.exists() {
        run_command("btrfs", &["subvolume", "delete", &snap_path.to_string_lossy()], "Delete Snapshot")?;
    }

    umount_btrfs_root()?;
    Ok(())
}
