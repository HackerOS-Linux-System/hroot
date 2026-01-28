use anyhow::{anyhow, Context, Result};
use clap::{Parser, Subcommand};
use hammer_core::{run_command, Logger};
use nix::mount::{mount, MsFlags};
use nix::unistd::Uid;
use owo_colors::OwoColorize;
use std::fs;
use std::path::Path;

#[derive(Parser)]
#[command(name = "hammer-read")]
struct Cli {
    #[command(subcommand)]
    command: Option<Commands>,

    /// Remount /usr as Read-Only (Legacy flag)
    #[arg(long, action)]
    lock: bool,

    /// Remount /usr as Read-Write (Legacy flag)
    #[arg(long, action)]
    unlock: bool,
}

#[derive(Subcommand)]
enum Commands {
    /// Lock the system (Read-Only for /usr and /boot)
    Lock,
    /// Unlock the system (Read-Write for /usr and /boot)
    Unlock,
    /// Create a temporary writable overlay on /usr (changes vanish after reboot)
    TemporaryUnlock,
    /// Install persistence (Systemd service + fstab RO enforcement + /home setup)
    Install,
}

fn main() -> Result<()> {
    if !Uid::current().is_root() {
        eprintln!("{}", "Permission denied. Must be root.".red().bold());
        std::process::exit(1);
    }

    let cli = Cli::parse();

    match cli.command {
        Some(Commands::Install) => install_persistence()?,
        Some(Commands::Lock) => toggle_lock(true)?,
        Some(Commands::Unlock) => toggle_lock(false)?,
        Some(Commands::TemporaryUnlock) => enable_overlay_fs()?,
        None => {
            // Handle legacy flags
            if cli.unlock {
                toggle_lock(false)?;
            } else {
                toggle_lock(true)?;
            }
        }
    }

    Ok(())
}

fn toggle_lock(readonly: bool) -> Result<()> {
    // Protect OS binaries
    remount_path("/usr", readonly)?;

    // Protect Kernel and Bootloader config
    remount_path("/boot", readonly)?;

    Ok(())
}

fn remount_path(path: &str, readonly: bool) -> Result<()> {
    let target = Path::new(path);
    if !target.exists() {
        return Ok(());
    }

    let mut flags = MsFlags::MS_REMOUNT | MsFlags::MS_BIND;

    if readonly {
        flags |= MsFlags::MS_RDONLY;
        Logger::info(&format!("Remounting {} as READ-ONLY...", path));
    } else {
        Logger::info(&format!("Remounting {} as READ-WRITE...", path));
    }

    mount(
        Some(target),
          target,
          None::<&str>,
          flags,
          None::<&str>
    ).with_context(|| format!("Failed to remount {}", path))?;

    Logger::success(&format!("{} is now {}", path, if readonly { "Read-Only" } else { "Read-Write" }));
    Ok(())
}

fn enable_overlay_fs() -> Result<()> {
    Logger::info("Setting up OverlayFS for temporary write access...");

    // 1. Prepare tmpfs for upper/work dirs
    let overlay_base = Path::new("/run/hammer/overlay");
    if !overlay_base.exists() {
        fs::create_dir_all(overlay_base)?;
        // Mount tmpfs
        mount(
            Some("tmpfs"),
              overlay_base,
              Some("tmpfs"),
              MsFlags::empty(),
              Some("size=1G")
        ).context("Failed to mount tmpfs for overlay")?;
    }

    let upper_dir = overlay_base.join("upper");
    let work_dir = overlay_base.join("work");
    fs::create_dir_all(&upper_dir)?;
    fs::create_dir_all(&work_dir)?;

    // 2. Mount OverlayFS on /usr
    Logger::info("Mounting overlay on /usr...");

    let opts = format!(
        "lowerdir=/usr,upperdir={},workdir={}",
        upper_dir.display(),
                       work_dir.display()
    );

    mount(
        Some("overlay"),
          Path::new("/usr"),
          Some("overlay"),
          MsFlags::empty(),
          Some(opts.as_str())
    ).context("Failed to mount overlayfs on /usr")?;

    Logger::success("Temporary unlock active. Changes to /usr are writable but will VANISH after reboot.");
    Ok(())
}

fn install_persistence() -> Result<()> {
    Logger::info("Configuring system persistence...");

    install_systemd_service()?;
    update_fstab()?;
    ensure_home_persistence()?;

    Logger::success("Persistence configuration complete.");
    Ok(())
}

fn install_systemd_service() -> Result<()> {
    Logger::info("Installing hammer-readonly systemd service...");

    let service_content = r#"[Unit]
    Description=Hammer Read-Only Enforcement
    DefaultDependencies=no
    After=systemd-remount-fs.service
    Before=local-fs.target

    [Service]
    Type=oneshot
    ExecStart=/usr/bin/hammer read-only lock
    RemainAfterExit=yes
    StandardOutput=journal

    [Install]
    WantedBy=sysinit.target
    "#;

    let service_path = "/etc/systemd/system/hammer-readonly.service";
    fs::write(service_path, service_content).context("Failed to write service file")?;

    run_command("systemctl", &["daemon-reload"], "Reloading Daemon")?;
    run_command("systemctl", &["enable", "hammer-readonly.service"], "Enabling Service")?;

    Logger::success("Systemd service installed.");
    Ok(())
}

fn update_fstab() -> Result<()> {
    let fstab_path = "/etc/fstab";
    Logger::info(&format!("Analyzing {}...", fstab_path));

    let content = fs::read_to_string(fstab_path).context("Failed to read fstab")?;
    let mut new_lines = Vec::new();
    let mut modified = false;

    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            new_lines.push(line.to_string());
            continue;
        }

        let parts: Vec<&str> = trimmed.split_whitespace().collect();
        if parts.len() >= 4 {
            let mount_point = parts[1];
            let options = parts[3];

            // 1. Enforce RO on /boot
            if mount_point == "/boot" && !options.contains("ro") {
                let new_opts = replace_option(options, "rw", "ro");
                new_lines.push(reconstruct_fstab_line(&parts, &new_opts));
                modified = true;
                Logger::info("Configured /boot as read-only.");
                continue;
            }

            // 2. Ensure /var and /home are RW (if explicitly listed)
            if (mount_point == "/var" || mount_point == "/home") && !options.contains("rw") && !options.contains("defaults") {
                let new_opts = replace_option(options, "ro", "rw");
                new_lines.push(reconstruct_fstab_line(&parts, &new_opts));
                modified = true;
                Logger::info(&format!("Configured {} as read-write.", mount_point));
                continue;
            }
        }
        new_lines.push(line.to_string());
    }

    if modified {
        fs::write(format!("{}.bak", fstab_path), &content)?;
        fs::write(fstab_path, new_lines.join("\n") + "\n")?;
        Logger::success("fstab updated.");
    } else {
        Logger::info("fstab is already correctly configured.");
    }

    Ok(())
}

fn ensure_home_persistence() -> Result<()> {
    // In atomic systems, /home is often a symlink to /var/home or a bind mount.
    // We need to ensure user data is writable.

    let home_path = Path::new("/home");

    // Check if /home is a symlink
    if fs::symlink_metadata(home_path)?.file_type().is_symlink() {
        Logger::info("/home is a symlink (likely to /var/home). Persistence is handled by OSTree/var.");
        return Ok(());
    }

    // Check if /home is a mountpoint
    let mounts = fs::read_to_string("/proc/mounts")?;
    let is_mount = mounts.lines().any(|l| l.split_whitespace().nth(1) == Some("/home"));

    if !is_mount {
        Logger::info("/home is a directory on root. Setting up bind mount from /var/home for persistence...");

        let var_home = Path::new("/var/home");
        if !var_home.exists() {
            fs::create_dir_all(var_home)?;
        }

        // Add bind mount to fstab if not present
        let fstab = fs::read_to_string("/etc/fstab")?;
        if !fstab.contains("/var/home /home") {
            let bind_entry = "/var/home /home none defaults,bind 0 0";
            let mut file = fs::OpenOptions::new().append(true).open("/etc/fstab")?;
            use std::io::Write;
            writeln!(file, "{}", bind_entry)?;
            Logger::success("Added /var/home bind mount to fstab.");
        }
    }

    Ok(())
}

fn replace_option(options: &str, remove: &str, add: &str) -> String {
    let mut opts: Vec<String> = options.split(',')
    .filter(|&opt| opt != remove)
    .map(|s| s.to_string())
    .collect();
    opts.push(add.to_string());
    opts.join(",")
}

fn reconstruct_fstab_line(parts: &[&str], new_opts: &str) -> String {
    let mut line = format!("{}\t{}\t{}\t{}", parts[0], parts[1], parts[2], new_opts);
    if parts.len() > 4 { line.push_str(&format!("\t{}", parts[4])); }
    if parts.len() > 5 { line.push_str(&format!("\t{}", parts[5])); }
    line
}
