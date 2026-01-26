use std::process::{Command, Stdio};
use std::io::{Read, Write};
use std::fs::{self, File};
use std::path::Path;
use std::os::unix::fs::symlink;
use std::time::{SystemTime, UNIX_EPOCH};
use chrono::prelude::*;
use serde::{Serialize, Deserialize};
use sha2::{Sha256, Digest};
use anyhow::{Result, anyhow};
use clap::{Parser, Subcommand};
use regex::Regex;
use libc;

const VERSION: &str = "0.9";
const DEPLOYMENTS_DIR: &str = "/btrfs-root/deployments";
const CURRENT_SYMLINK: &str = "/btrfs-root/current";
const LOCK_FILE: &str = "/run/hammer.lock";
const TRANSACTION_MARKER: &str = "/btrfs-root/hammer-transaction";
const BTRFS_TOP: &str = "/btrfs-root";

#[derive(Parser)]
#[command(name = "hammer-updater")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    Update,
    Init,
}

fn main() -> Result<()> {
    if unsafe { libc::getuid() } != 0 {
        eprintln!("This tool must be run as root.");
        std::process::exit(1);
    }
    let cli = Cli::parse();
    match cli.command {
        Commands::Update => update_command()?,
        Commands::Init => init_command()?,
    }
    Ok(())
}

fn ensure_top_mounted() -> Result<()> {
    let output = run_command("mountpoint", &["-q", BTRFS_TOP])?;
    if output.success {
        return Ok(());
    }
    fs::create_dir_all(BTRFS_TOP)?;
    let device = get_root_device()?;
    let output = run_command("mount", &["-o", "subvol=/", &device, BTRFS_TOP])?;
    if !output.success {
        return Err(anyhow!("Failed to mount btrfs top: {}", output.stderr));
    }
    Ok(())
}

fn get_root_device() -> Result<String> {
    let output = run_command("findmnt", &["-no", "SOURCE", "/"])?;
    if !output.success {
        return Err(anyhow!("Failed to find root device: {}", output.stderr));
    }
    let stdout = output.stdout.trim();
    let device = if let Some(pos) = stdout.find('[') {
        stdout[..pos].trim().to_string()
    } else {
        stdout.to_string()
    };
    Ok(device)
}

fn acquire_lock() -> Result<()> {
    if Path::new(LOCK_FILE).exists() {
        return Err(anyhow!("Hammer operation in progress (lock file exists)."));
    }
    File::create(LOCK_FILE)?;
    Ok(())
}

fn release_lock() {
    let _ = fs::remove_file(LOCK_FILE);
}

fn validate_system() -> Result<()> {
    let output = run_command("btrfs", &["filesystem", "show", "/"])?;
    if !output.success {
        return Err(anyhow!("Root filesystem is not BTRFS."));
    }
    if !Path::new(CURRENT_SYMLINK).is_symlink() {
        return Err(anyhow!("Current deployment symlink missing."));
    }
    let current = fs::read_link(CURRENT_SYMLINK)?;
    let current_str = current.to_str().ok_or(anyhow!("Invalid symlink"))?;
    let prop_output = run_command("btrfs", &["property", "get", "-ts", current_str, "ro"])?;
    if !prop_output.success || prop_output.stdout.trim() != "ro=true" {
        return Err(anyhow!("Current deployment is not read-only."));
    }
    Ok(())
}

struct CommandOutput {
    success: bool,
    stdout: String,
    stderr: String,
}

fn run_command(cmd: &str, args: &[&str]) -> Result<CommandOutput> {
    let mut child = Command::new(cmd)
        .args(args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    let status = child.wait()?;
    let mut stdout = String::new();
    child.stdout.ok_or(anyhow!("No stdout"))?.read_to_string(&mut stdout)?;
    let mut stderr = String::new();
    child.stderr.ok_or(anyhow!("No stderr"))?.read_to_string(&mut stderr)?;
    Ok(CommandOutput {
        success: status.success(),
        stdout,
        stderr,
    })
}

fn get_subvol_name(path: &str) -> Result<String> {
    let output = run_command("btrfs", &["subvolume", "show", path])?;
    if !output.success {
        return Err(anyhow!("Failed to get subvolume for {}: {}", path, output.stderr));
    }
    let lines: Vec<&str> = output.stdout.lines().collect();
    let first_line = lines.get(0).unwrap_or(&"").trim().to_string();
    if first_line == "" || first_line == "/" {
        Ok("".to_string())
    } else {
        Ok(first_line)
    }
}

fn snapshot_recursive(source: &str, dest: &str, writable: bool) -> Result<()> {
    let mut args = vec!["subvolume", "snapshot"];
    if !writable {
        args.push("-r");
    }
    args.push(source);
    args.push(dest);
    let output = run_command("btrfs", &args)?;
    if !output.success {
        return Err(anyhow!("Failed to snapshot {} to {}: {}", source, dest, output.stderr));
    }
    let list_output = run_command("btrfs", &["subvolume", "list", "-a", "--sort=path", source])?;
    if !list_output.success {
        return Err(anyhow!("Failed to list subvolumes: {}", list_output.stderr));
    }
    let source_subvol = get_subvol_name(source)?;
    let prefix = if source_subvol.is_empty() { "/".to_string() } else { format!("/{}", source_subvol) };
    let prefix_length = prefix.len();
    for line in list_output.stdout.lines() {
        if let Some(captures) = Regex::new(r"ID \d+ gen \d+ path (.*)").unwrap().captures(line) {
            let full_path = captures.get(1).unwrap().as_str();
            if full_path.starts_with(&prefix) {
                let rel_path = &full_path[prefix_length..];
                if rel_path.is_empty() {
                    continue;
                }
                let sub_source = format!("{}/{}", source, rel_path);
                let sub_dest = format!("{}/{}", dest, rel_path);
                let rmdir_output = run_command("rmdir", &[&sub_dest])?;
                if !rmdir_output.success {
                    return Err(anyhow!("Failed to rmdir placeholder {}: {}", sub_dest, rmdir_output.stderr));
                }
                let mut args = vec!["subvolume", "snapshot"];
                if !writable {
                    args.push("-r");
                }
                args.push(&sub_source);
                args.push(&sub_dest);
                let output = run_command("btrfs", &args)?;
                if !output.success {
                    return Err(anyhow!("Failed to snapshot nested {} to {}: {}", sub_source, sub_dest, output.stderr));
                }
            }
        }
    }
    Ok(())
}

fn set_readonly_recursive(path: &str, readonly: bool) -> Result<()> {
    set_subvolume_readonly(path, readonly)?;
    let list_output = run_command("btrfs", &["subvolume", "list", "-a", "--sort=path", path])?;
    if !list_output.success {
        return Err(anyhow!("Failed to list subvolumes: {}", list_output.stderr));
    }
    let path_subvol = get_subvol_name(path)?;
    let prefix = if path_subvol.is_empty() { "/".to_string() } else { format!("/{}", path_subvol) };
    let prefix_length = prefix.len();
    for line in list_output.stdout.lines() {
        if let Some(captures) = Regex::new(r"ID \d+ gen \d+ path (.*)").unwrap().captures(line) {
            let full_path = captures.get(1).unwrap().as_str();
            if full_path.starts_with(&prefix) {
                let rel_path = &full_path[prefix_length..];
                if rel_path.is_empty() {
                    continue;
                }
                let sub_path = format!("{}/{}", path, rel_path);
                set_subvolume_readonly(&sub_path, readonly)?;
            }
        }
    }
    Ok(())
}

fn set_subvolume_readonly(path: &str, readonly: bool) -> Result<()> {
    let value = if readonly { "true" } else { "false" };
    let output = run_command("btrfs", &["property", "set", "-ts", path, "ro", value])?;
    if !output.success {
        return Err(anyhow!("Failed to set readonly for {}: {}", path, output.stderr));
    }
    Ok(())
}

#[allow(unused_variables)]
#[allow(unused_assignments)]
fn init_command() -> Result<()> {
    let mut _new_deployment = None;
    let mut _temp_chroot = None;
    let mut _temp_mounted = false;
    let mut _chroot_mounted = false;
    ensure_top_mounted()?;
    acquire_lock()?;
    println!("Initializing system...");
    let output = run_command("btrfs", &["filesystem", "show", "/"])?;
    if !output.success {
        return Err(anyhow!("Root filesystem is not BTRFS."));
    }
    let current_subvol = get_subvol_name("/")?;
    if current_subvol.contains(' ') || current_subvol.contains('\n') {
        return Err(anyhow!("Unable to parse subvolume path from: {}", current_subvol));
    }
    let current_path = if current_subvol.is_empty() { BTRFS_TOP.to_string() } else { format!("{}/{}", BTRFS_TOP, current_subvol) };
    if !Path::new(DEPLOYMENTS_DIR).exists() {
        let output = run_command("btrfs", &["subvolume", "create", DEPLOYMENTS_DIR])?;
        if !output.success {
            return Err(anyhow!("Failed to create deployments subvolume: {}", output.stderr));
        }
    }
    let timestamp = Local::now().format("%Y%m%d%H%M%S").to_string();
    let new_deployment_path = format!("{}/hammer-{}", DEPLOYMENTS_DIR, timestamp);
    _new_deployment = Some(new_deployment_path.clone());
    snapshot_recursive(&current_path, &new_deployment_path, true)?;
    let device = get_root_device()?;
    let new_subvol = get_subvol_name(&new_deployment_path)?;
    let temp_dir = create_temp_dir("hammer")?;
    _temp_chroot = Some(temp_dir.clone());
    let output = run_command("mount", &["-o", &format!("subvol={}", new_subvol), &device, &temp_dir])?;
    if !output.success {
        return Err(anyhow!("Failed to mount temp_chroot: {}", output.stderr));
    }
    _temp_mounted = true;
    bind_mounts_for_chroot(&temp_dir, true)?;
    _chroot_mounted = true;
    let chroot_cmd = format!("chroot {} /bin/sh -c 'apt update && apt install --reinstall -y plymouth && apt-mark manual plymouth && dpkg -l > /var/log/packages.list && update-initramfs -u -k all && chmod -x /etc/grub.d/10_linux /etc/grub.d/20_linux_xen /etc/grub.d/30_os-prober'", temp_dir);
    let output = run_command("/bin/sh", &["-c", &chroot_cmd])?;
    if !output.success {
        return Err(anyhow!("Failed in chroot for initial setup: {}", output.stderr));
    }
    let kernel = get_kernel_version(&temp_dir)?;
    sanity_check(&new_deployment_path, &kernel, &temp_dir)?;
    let system_version = compute_system_version(&new_deployment_path)?;
    write_meta(&new_deployment_path, "initial", &current_subvol, &kernel, &system_version, "ready")?;
    update_bootloader_entries(&new_deployment_path)?;
    let grub_cmd = format!("chroot {} /bin/sh -c 'update-grub'", temp_dir);
    let grub_output = run_command("/bin/sh", &["-c", &grub_cmd])?;
    if !grub_output.success {
        return Err(anyhow!("Failed in chroot for grub update: {}", grub_output.stderr));
    }
    bind_mounts_for_chroot(&temp_dir, false)?;
    _chroot_mounted = false;
    let umount_output = run_command("umount", &[&temp_dir])?;
    if !umount_output.success {
        return Err(anyhow!("Failed to umount temp_chroot: {}", umount_output.stderr));
    }
    _temp_mounted = false;
    set_subvolume_readonly(&new_deployment_path, true)?;
    create_transaction_marker(&new_deployment_path)?;
    switch_to_deployment(&new_deployment_path)?;
    println!("System initialized. Please reboot to apply the initial deployment.");
    Ok(())
}

#[allow(unused_variables)]
#[allow(unused_assignments)]
fn update_command() -> Result<()> {
    ensure_top_mounted()?;
    let mut _new_deployment = None;
    let mut _temp_chroot = None;
    let mut _temp_mounted = false;
    let mut _chroot_mounted = false;
    acquire_lock()?;
    validate_system()?;
    println!("Updating system atomically...");
    let current = fs::read_link(CURRENT_SYMLINK)?.to_str().unwrap().to_string();
    let parent = Path::new(&current).file_name().unwrap().to_str().unwrap().to_string();
    let new_deployment_path = create_deployment(true)?;
    _new_deployment = Some(new_deployment_path.clone());
    create_transaction_marker(&new_deployment_path)?;
    let device = get_root_device()?;
    let new_subvol = get_subvol_name(&new_deployment_path)?;
    let temp_dir = create_temp_dir("hammer")?;
    _temp_chroot = Some(temp_dir.clone());
    let output = run_command("mount", &["-o", &format!("subvol={}", new_subvol), &device, &temp_dir])?;
    if !output.success {
        return Err(anyhow!("Failed to mount temp_chroot: {}", output.stderr));
    }
    _temp_mounted = true;
    bind_mounts_for_chroot(&temp_dir, true)?;
    _chroot_mounted = true;
    let chroot_cmd = format!("chroot {} /bin/sh -c 'apt update && apt-mark manual plymouth && apt upgrade -y -o Dpkg::Options::=--force-confold && apt autoremove -y && dpkg -l > /var/log/packages.list && update-initramfs -u -k all && chmod -x /etc/grub.d/10_linux /etc/grub.d/20_linux_xen /etc/grub.d/30_os-prober'", temp_dir);
    let output = run_command("/bin/sh", &["-c", &chroot_cmd])?;
    if !output.success {
        return Err(anyhow!("Failed to update in chroot: {}", output.stderr));
    }
    let kernel = get_kernel_version(&temp_dir)?;
    sanity_check(&new_deployment_path, &kernel, &temp_dir)?;
    let system_version = compute_system_version(&new_deployment_path)?;
    write_meta(&new_deployment_path, "update", &parent, &kernel, &system_version, "ready")?;
    update_bootloader_entries(&new_deployment_path)?;
    let grub_cmd = format!("chroot {} /bin/sh -c 'update-grub'", temp_dir);
    let grub_output = run_command("/bin/sh", &["-c", &grub_cmd])?;
    if !grub_output.success {
        return Err(anyhow!("Failed in chroot for grub update: {}", grub_output.stderr));
    }
    bind_mounts_for_chroot(&temp_dir, false)?;
    _chroot_mounted = false;
    let umount_output = run_command("umount", &[&temp_dir])?;
    if !umount_output.success {
        return Err(anyhow!("Failed to umount temp_chroot: {}", umount_output.stderr));
    }
    _temp_mounted = false;
    set_subvolume_readonly(&new_deployment_path, true)?;
    switch_to_deployment(&new_deployment_path)?;
    remove_transaction_marker()?;
    println!("System updated. Reboot to apply changes.");
    release_lock();
    Ok(())
}

fn create_deployment(writable: bool) -> Result<String> {
    println!("Creating new deployment...");
    fs::create_dir_all(DEPLOYMENTS_DIR)?;
    let current = fs::read_link(CURRENT_SYMLINK)?.to_str().unwrap().to_string();
    let timestamp = Local::now().format("%Y%m%d%H%M%S").to_string();
    let new_deployment = format!("{}/hammer-{}", DEPLOYMENTS_DIR, timestamp);
    snapshot_recursive(&current, &new_deployment, writable)?;
    if writable {
        set_readonly_recursive(&new_deployment, false)?;
    }
    println!("Deployment created: {}", new_deployment);
    Ok(new_deployment)
}

fn create_temp_dir(prefix: &str) -> Result<String> {
    let output = run_command("mktemp", &["-d", "--tmpdir", &format!("{}.XXXXXX", prefix)])?;
    if !output.success {
        return Err(anyhow!("Failed to create temp dir: {}", output.stderr));
    }
    Ok(output.stdout.trim().to_string())
}

fn bind_mounts_for_chroot(chroot: &str, mount: bool) -> Result<()> {
    let binds = vec![
        "/proc", "/sys", "/dev", "/run", "/tmp",
    ];
    for bind in binds {
        let target = format!("{}{}", chroot, bind);
        fs::create_dir_all(&target)?;
        let cmd = if mount { "mount" } else { "umount" };
        let args: Vec<&str> = if mount {
            vec!["--bind", bind, target.as_str()]
        } else {
            vec![target.as_str()]
        };
        let output = run_command(cmd, &args)?;
        if !output.success {
            return Err(anyhow!("Failed to {} {}: {}", cmd, bind, output.stderr));
        }
    }
    Ok(())
}

fn get_kernel_version(chroot: &str) -> Result<String> {
    let output = run_command("chroot", &[chroot, "uname", "-r"])?;
    if !output.success {
        return Err(anyhow!("Failed to get kernel version"));
    }
    Ok(output.stdout.trim().to_string())
}

fn sanity_check(_deployment: &str, _kernel: &str, _chroot: &str) -> Result<()> {
    Ok(())
}

fn compute_system_version(deployment: &str) -> Result<String> {
    let mut hasher = Sha256::new();
    let packages_path = format!("{}/var/log/packages.list", deployment);
    let mut file = File::open(packages_path)?;
    let mut buffer = Vec::new();
    file.read_to_end(&mut buffer)?;
    hasher.update(&buffer);
    let hash = hasher.finalize();
    Ok(format!("{:x}", hash))
}

#[derive(Serialize, Deserialize)]
struct Meta {
    kind: String,
    parent: String,
    kernel: String,
    system_version: String,
    status: String,
}

fn write_meta(deployment: &str, kind: &str, parent: &str, kernel: &str, system_version: &str, status: &str) -> Result<()> {
    let meta = Meta {
        kind: kind.to_string(),
        parent: parent.to_string(),
        kernel: kernel.to_string(),
        system_version: system_version.to_string(),
        status: status.to_string(),
    };
    let meta_path = format!("{}/.meta.json", deployment);
    let mut file = File::create(meta_path)?;
    let json = serde_json::to_string(&meta)?;
    file.write_all(json.as_bytes())?;
    Ok(())
}

fn update_bootloader_entries(_deployment: &str) -> Result<()> {
    Ok(())
}

fn set_status_broken(_deployment: &str) {
}

fn create_transaction_marker(deployment: &str) -> Result<()> {
    let mut file = File::create(TRANSACTION_MARKER)?;
    file.write_all(deployment.as_bytes())?;
    Ok(())
}

fn switch_to_deployment(deployment: &str) -> Result<()> {
    fs::remove_file(CURRENT_SYMLINK)?;
    symlink(deployment, CURRENT_SYMLINK)?;
    Ok(())
}

fn remove_transaction_marker() -> Result<()> {
    fs::remove_file(TRANSACTION_MARKER)?;
    Ok(())
}
