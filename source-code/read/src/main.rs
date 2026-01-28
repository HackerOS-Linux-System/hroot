use miette::{miette, IntoDiagnostic, Result, WrapErr};
use clap::{Parser, Subcommand};
use hammer_core::{run_command, Logger};
use nix::unistd::Uid;
use owo_colors::OwoColorize;
use std::fs;
use std::path::Path;
use std::process::Command;

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

    // Init logger for fancy output
    Logger::init()?;

    let cli = Cli::parse();

    match cli.command {
        Some(Commands::Install) => install_persistence()?,
        Some(Commands::Lock) => toggle_lock(true)?,
        Some(Commands::Unlock) => toggle_lock(false)?,
        Some(Commands::TemporaryUnlock) => enable_overlay_fs()?,
        None => {
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
    Logger::section("Filesystem Protection");

    // Protect OS binaries
    remount_path_via_bind("/usr", readonly)?;

    // Protect Kernel and Bootloader config
    remount_path_via_bind("/boot", readonly)?;

    Logger::end_section();
    Ok(())
}

// Fix for EINVAL: Use double mount strategy
// 1. Ensure it's a mountpoint (bind mount to self if needed)
// 2. Remount with new flags
fn remount_path_via_bind(path: &str, readonly: bool) -> Result<()> {
    let target = Path::new(path);
    if !target.exists() {
        return Ok(());
    }

    // Check if it is already a mountpoint
    let check_mount = run_command("mountpoint", &["-q", path], "Check Mountpoint");

    // If not a mountpoint, bind mount it to itself to make it one
    if check_mount.is_err() {
        Logger::info(&format!("Converting {} to bind mount...", path));
        run_command("mount", &["--bind", path, path], "Bind Mount Self")?;
    }

    if readonly {
        Logger::info(&format!("Locking {} (Read-Only)...", path));
        // Note: remount,bind,ro is the correct sequence to change flags on a bind mount
        run_command("mount", &["-o", "remount,bind,ro", path], "Remount RO")?;
    } else {
        Logger::info(&format!("Unlocking {} (Read-Write)...", path));
        run_command("mount", &["-o", "remount,bind,rw", path], "Remount RW")?;
    }

    Logger::success(&format!("{} configured.", path));
    Ok(())
}

fn enable_overlay_fs() -> Result<()> {
    Logger::section("Temporary Overlay");
    Logger::info("Setting up OverlayFS for temporary write access...");

    // 1. Prepare tmpfs for upper/work dirs
    let overlay_base = Path::new("/run/hammer/overlay");
    if !overlay_base.exists() {
        fs::create_dir_all(overlay_base).into_diagnostic()?;
        // Mount tmpfs
        run_command("mount", &["-t", "tmpfs", "tmpfs", "/run/hammer/overlay", "-o", "size=1G"], "Mount Tmpfs")?;
    }

    let upper_dir = overlay_base.join("upper");
    let work_dir = overlay_base.join("work");
    fs::create_dir_all(&upper_dir).into_diagnostic()?;
    fs::create_dir_all(&work_dir).into_diagnostic()?;

    // 2. Mount OverlayFS on /usr
    Logger::info("Mounting overlay on /usr...");

    let opts = format!(
        "lowerdir=/usr,upperdir={},workdir={}",
        upper_dir.display(),
                       work_dir.display()
    );

    run_command("mount", &["-t", "overlay", "overlay", "/usr", "-o", &opts], "Mount Overlay")?;

    Logger::success("Temporary unlock active. Changes to /usr are writable but will VANISH after reboot.");
    Logger::end_section();
    Ok(())
}

fn install_persistence() -> Result<()> {
    Logger::section("Installing Persistence");
    install_systemd_service()?;
    update_fstab()?;
    ensure_home_persistence()?;
    Logger::success("Persistence configuration complete.");
    Logger::end_section();
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
    fs::write(service_path, service_content)
    .into_diagnostic()
    .wrap_err("Failed to write service file")?;

    run_command("systemctl", &["daemon-reload"], "Reloading Daemon")?;
    run_command("systemctl", &["enable", "hammer-readonly.service"], "Enabling Service")?;

    Logger::success("Systemd service installed.");
    Ok(())
}

fn update_fstab() -> Result<()> {
    let fstab_path = "/etc/fstab";
    Logger::info(&format!("Analyzing {}...", fstab_path));

    let content = fs::read_to_string(fstab_path)
    .into_diagnostic()
    .wrap_err("Failed to read fstab")?;

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

            if mount_point == "/boot" && !options.contains("ro") {
                let new_opts = replace_option(options, "rw", "ro");
                new_lines.push(reconstruct_fstab_line(&parts, &new_opts));
                modified = true;
                continue;
            }
            // Ensure @home is RW if using btrfs
            if mount_point == "/home" && !options.contains("rw") && !options.contains("defaults") {
                let new_opts = replace_option(options, "ro", "rw");
                new_lines.push(reconstruct_fstab_line(&parts, &new_opts));
                modified = true;
                continue;
            }
        }
        new_lines.push(line.to_string());
    }

    if modified {
        fs::write(format!("{}.bak", fstab_path), &content).into_diagnostic()?;
        fs::write(fstab_path, new_lines.join("\n") + "\n").into_diagnostic()?;
        Logger::success("fstab updated.");
    } else {
        Logger::info("fstab is already correctly configured.");
    }

    Ok(())
}

fn ensure_home_persistence() -> Result<()> {
    let home_path = Path::new("/home");
    // Check if /home is a mountpoint
    let check = run_command("mountpoint", &["-q", "/home"], "Check Home");

    if check.is_err() {
        // If not a mountpoint, maybe we need to bind mount /var/home
        Logger::info("/home is not a mountpoint. Setting up /var/home bind...");
        let var_home = Path::new("/var/home");
        if !var_home.exists() {
            fs::create_dir_all(var_home).into_diagnostic()?;
        }
        // Add bind mount to fstab if not present
        let fstab = fs::read_to_string("/etc/fstab").into_diagnostic()?;
        if !fstab.contains("/var/home /home") {
            let bind_entry = "/var/home /home none defaults,bind 0 0";
            let mut file = fs::OpenOptions::new().append(true).open("/etc/fstab").into_diagnostic()?;
            use std::io::Write;
            writeln!(file, "{}", bind_entry).into_diagnostic()?;
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
