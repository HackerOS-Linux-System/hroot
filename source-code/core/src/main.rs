use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::os::unix::fs::symlink;
use std::path::Path;
use std::process::Command;

use chrono::prelude::*;
use clap::{Parser, Subcommand, CommandFactory};
use nix::unistd::Uid;
use regex::Regex;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

const BTRFS_TOP: &str = "/btrfs-root";
const DEPLOYMENTS_DIR: &str = "/btrfs-root/deployments";
const CURRENT_SYMLINK: &str = "/btrfs-root/current";
const LOCK_FILE: &str = "/run/hammer.lock";
const TRANSACTION_MARKER: &str = "/btrfs-root/hammer-transaction";

#[derive(Serialize, Deserialize)]
struct Meta {
    created: String,
    description: String,
    parent: String,
    kernel: String,
    system_version: String,
    status: String,
    rollback_reason: Option<String>,
}

#[derive(Parser)]
#[command(name = "hammer")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    Install { package: String },
    Remove { package: String },
    Switch { deployment: Option<String> },
    Rollback { n: Option<u32> },
    Cleanup,
    Refresh,
    Init,
    List,
    Status,
    Version,
    Help,
}

fn main() -> io::Result<()> {
    if !Uid::current().is_root() {
        eprintln!("This tool must be run as root.");
        std::process::exit(1);
    }

    let cli = Cli::parse();

    let result = match cli.command {
        Commands::Install { package } => atomic_install(&package),
        Commands::Remove { package } => atomic_remove(&package),
        Commands::Switch { deployment } => switch_deployment(deployment),
        Commands::Rollback { n } => rollback(n.unwrap_or(1)),
        Commands::Cleanup => clean_up(),
        Commands::Refresh => atomic_refresh(),
        Commands::Init => init(),
        Commands::List => list_deployments(),
        Commands::Status => status(),
        Commands::Version => {
            println!("hammer version 1.0");
            Ok(())
        }
        Commands::Help => {
            Cli::command().print_help().unwrap();
            Ok(())
        }
    };

    result
}

fn run_command(cmd: &str, args: &[&str]) -> Result<(bool, String, String), io::Error> {
    let output = Command::new(cmd).args(args).output()?;
    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
    Ok((output.status.success(), stdout, stderr))
}

fn acquire_lock() -> Result<(), io::Error> {
    if Path::new(LOCK_FILE).exists() {
        return Err(io::Error::new(
            io::ErrorKind::Other,
            "Hammer operation in progress (lock file exists).",
        ));
    }
    File::create(LOCK_FILE)?;
    Ok(())
}

fn release_lock() {
    let _ = fs::remove_file(LOCK_FILE);
}

fn ensure_top_mounted() -> Result<(), io::Error> {
    let (success, _, _) = run_command("mountpoint", &["-q", BTRFS_TOP])?;
    if success {
        return Ok(());
    }
    if !Path::new(BTRFS_TOP).exists() {
        fs::create_dir(BTRFS_TOP)?;
    }
    let device = get_root_device()?;
    let (success, _, stderr) = run_command("mount", &["-o", "subvol=/", &device, BTRFS_TOP])?;
    if !success {
        return Err(io::Error::new(
            io::ErrorKind::Other,
            format!("Failed to mount btrfs top: {}", stderr),
        ));
    }
    Ok(())
}

fn get_root_device() -> Result<String, io::Error> {
    let (success, stdout, stderr) = run_command("findmnt", &["-no", "SOURCE", "/"])?;
    if !success {
        return Err(io::Error::new(
            io::ErrorKind::Other,
            format!("Failed to find root device: {}", stderr),
        ));
    }
    Ok(stdout.trim().replace(r#"\[.*\]"#, ""))
}

fn create_temp_dir(prefix: &str) -> Result<String, io::Error> {
    let (success, stdout, stderr) = run_command("mktemp", &["-d", "--tmpdir", &format!("{}.XXXXXX", prefix)])?;
    if !success {
        return Err(io::Error::new(
            io::ErrorKind::Other,
            format!("Failed to create temp dir: {}", stderr),
        ));
    }
    Ok(stdout.trim().to_string())
}

fn snapshot_recursive(source: &str, dest: &str, writable: bool) -> Result<(), io::Error> {
    let mut args: Vec<&str> = vec!["subvolume", "snapshot"];
    if !writable {
        args.push("-r");
    }
    args.push(source);
    args.push(dest);
    let (success, _, stderr) = run_command("btrfs", &args)?;
    if !success {
        return Err(io::Error::new(
            io::ErrorKind::Other,
            format!("Failed to create deployment: {}", stderr),
        ));
    }

    let (success_list, stdout_list, stderr_list) = run_command("btrfs", &["subvolume", "list", "-a", "--sort=path", source])?;
    if !success_list {
        return Err(io::Error::new(
            io::ErrorKind::Other,
            format!("Failed to list subvolumes: {}", stderr_list),
        ));
    }

    let source_subvol = get_subvol_name(source)?;
    let prefix = if source_subvol.is_empty() {
        "<FS_TREE>/".to_string()
    } else {
        format!("<FS_TREE>/{}/", source_subvol)
    };
    let prefix_length = prefix.len();

    let re = Regex::new(r"ID \d+ gen \d+ path (.*)").unwrap();
    for line in stdout_list.lines() {
        if let Some(captures) = re.captures(&line) {
            let full_path = captures.get(1).unwrap().as_str();
            if full_path.starts_with(&prefix) {
                let rel_path = &full_path[prefix_length..];
                if rel_path.is_empty() {
                    continue;
                }
                let sub_dest = format!("{}/{}", dest, rel_path);
                let _ = run_command("rmdir", &[&sub_dest]);
                let mut sub_args: Vec<&str> = vec!["subvolume", "snapshot"];
                if !writable {
                    sub_args.push("-r");
                }
                let source_rel = format!("{}/{}", source, rel_path);
                sub_args.push(&source_rel);
                sub_args.push(&sub_dest);
                let (sub_success, _, sub_stderr) = run_command("btrfs", &sub_args)?;
                if !sub_success {
                    return Err(io::Error::new(
                        io::ErrorKind::Other,
                        format!("Failed to snapshot nested {}: {}", rel_path, sub_stderr),
                    ));
                }
            }
        }
    }
    Ok(())
}

fn get_subvol_name(path: &str) -> Result<String, io::Error> {
    let (success, stdout, stderr) = run_command("btrfs", &["subvolume", "show", path])?;
    if !success {
        return Err(io::Error::new(
            io::ErrorKind::Other,
            format!("Failed to get subvol name: {}", stderr),
        ));
    }
    let mut lines = stdout.lines();
    let first_line = lines.next().ok_or(io::Error::new(io::ErrorKind::Other, "No output"))?.trim().to_string();
    Ok(first_line)
}

fn validate_system() -> Result<(), io::Error> {
    let (success, _, stderr) = run_command("btrfs", &["filesystem", "show", "/"])?;
    if !success {
        return Err(io::Error::new(io::ErrorKind::Other, format!("Root filesystem is not BTRFS: {}", stderr)));
    }
    if !Path::new(CURRENT_SYMLINK).is_symlink() {
        return Err(io::Error::new(
            io::ErrorKind::Other,
            "Current deployment symlink missing. System may not be initialized. Run 'sudo hammer init' to initialize.".to_string(),
        ));
    }
    let current = fs::read_link(CURRENT_SYMLINK)?.to_string_lossy().to_string();
    let (success_prop, stdout_prop, stderr_prop) = run_command("btrfs", &["property", "get", "-ts", &current, "ro"])?;
    if !success_prop {
        return Err(io::Error::new(
            io::ErrorKind::Other,
            format!("Failed to get property: {}", stderr_prop),
        ));
    }
    if stdout_prop.trim() != "ro=true" {
        return Err(io::Error::new(io::ErrorKind::Other, "Current deployment is not read-only.".to_string()));
    }
    Ok(())
}

fn atomic_install(package: &str) -> Result<(), io::Error> {
    let mut new_deployment: Option<String> = None;
    let mut temp_chroot: Option<String> = None;
    let mut mounted = false;
    let result = (|| -> Result<(), io::Error> {
        acquire_lock()?;
        validate_system()?;
        ensure_top_mounted()?;
        println!("Performing atomic install of {}...", package);
        new_deployment = Some(create_deployment(true)?);
        let nd = new_deployment.as_ref().unwrap();
        create_transaction_marker(nd)?;
        let symlink = fs::read_link(CURRENT_SYMLINK)?.file_name().unwrap().to_string_lossy().to_string();
        let device = get_root_device()?;
        let new_subvol = get_subvol_name(nd)?;
        temp_chroot = Some(create_temp_dir("hammer")?);
        let tc = temp_chroot.as_ref().unwrap();
        let (success, _, stderr) = run_command("mount", &["-o", &format!("subvol={}", new_subvol), &device, tc])?;
        if !success {
            return Err(io::Error::new(
                io::ErrorKind::Other,
                format!("Failed to mount temp_chroot: {}", stderr),
            ));
        }
        bind_mounts_for_chroot(tc, true)?;
        mounted = true;
        let check_cmd = format!("chroot {} /bin/sh -c 'dpkg -s {}'", tc, package);
        let (check_success, _, check_stderr) = run_command("/bin/sh", &["-c", &check_cmd])?;
        if check_success {
            println!("Package {} is already installed in the system.", package);
            return Err(io::Error::new(io::ErrorKind::Other, format!("Already installed: {}", check_stderr)));
        }
        let chroot_cmd = format!(
            "chroot {} /bin/sh -c 'apt update && apt install -y {} && apt autoremove -y && dpkg -l > /tmp/packages.list && update-initramfs -u -k all'",
            tc, package
        );
        let (success_ch, _, stderr_ch) = run_command("/bin/sh", &["-c", &chroot_cmd])?;
        if !success_ch {
            return Err(io::Error::new(
                io::ErrorKind::Other,
                format!("Failed to install in chroot: {}", stderr_ch),
            ));
        }
        let kernel = get_kernel_version(tc)?;
        sanity_check(nd, &kernel, tc)?;
        let system_version = compute_system_version(nd)?;
        write_meta(nd, &format!("install {}", package), &symlink, &kernel, &system_version, "ready")?;
        update_bootloader_entries(nd)?;
        let grub_cmd = format!("chroot {} /bin/sh -c 'update-grub'", tc);
        let (grub_success, _, grub_stderr) = run_command("/bin/sh", &["-c", &grub_cmd])?;
        if !grub_success {
            return Err(io::Error::new(
                io::ErrorKind::Other,
                format!("Failed to update grub in chroot: {}", grub_stderr),
            ));
        }
        bind_mounts_for_chroot(tc, false)?;
        let (um_success, _, um_stderr) = run_command("umount", &[tc])?;
        if !um_success {
            return Err(io::Error::new(
                io::ErrorKind::Other,
                format!("Failed to umount temp_chroot: {}", um_stderr),
            ));
        }
        mounted = false;
        set_subvolume_readonly(nd, true)?;
        switch_to_deployment_inner(nd)?;
        remove_transaction_marker()?;
        println!("Atomic install completed. Reboot to apply.");
        Ok(())
    })();
    if result.is_err() {
        if let Some(nd) = &new_deployment {
            let _ = set_status_broken(nd);
        }
    }
    if mounted {
        if let Some(tc) = &temp_chroot {
            let _ = bind_mounts_for_chroot(tc, false);
            let _ = run_command("umount", &[tc]);
        }
    }
    if let Some(tc) = temp_chroot {
        if Path::new(&tc).exists() {
            let _ = fs::remove_dir_all(&tc);
        }
    }
    release_lock();
    result
}

fn atomic_remove(package: &str) -> Result<(), io::Error> {
    let mut new_deployment: Option<String> = None;
    let mut temp_chroot: Option<String> = None;
    let mut mounted = false;
    let result = (|| -> Result<(), io::Error> {
        acquire_lock()?;
        validate_system()?;
        ensure_top_mounted()?;
        println!("Performing atomic remove of {}...", package);
        new_deployment = Some(create_deployment(true)?);
        let nd = new_deployment.as_ref().unwrap();
        create_transaction_marker(nd)?;
        let symlink = fs::read_link(CURRENT_SYMLINK)?.file_name().unwrap().to_string_lossy().to_string();
        let device = get_root_device()?;
        let new_subvol = get_subvol_name(nd)?;
        temp_chroot = Some(create_temp_dir("hammer")?);
        let tc = temp_chroot.as_ref().unwrap();
        let (success, _, stderr) = run_command("mount", &["-o", &format!("subvol={}", new_subvol), &device, tc])?;
        if !success {
            return Err(io::Error::new(
                io::ErrorKind::Other,
                format!("Failed to mount temp_chroot: {}", stderr),
            ));
        }
        bind_mounts_for_chroot(tc, true)?;
        mounted = true;
        let check_cmd = format!("chroot {} /bin/sh -c 'dpkg -s {}'", tc, package);
        let (check_success, _, check_stderr) = run_command("/bin/sh", &["-c", &check_cmd])?;
        if !check_success {
            println!("Package {} is not installed in the system.", package);
            return Err(io::Error::new(io::ErrorKind::Other, format!("Not installed: {}", check_stderr)));
        }
        let chroot_cmd = format!(
            "chroot {} /bin/sh -c 'apt remove -y {} && apt autoremove -y && dpkg -l > /tmp/packages.list && update-initramfs -u -k all'",
            tc, package
        );
        let (success_ch, _, stderr_ch) = run_command("/bin/sh", &["-c", &chroot_cmd])?;
        if !success_ch {
            return Err(io::Error::new(
                io::ErrorKind::Other,
                format!("Failed to remove in chroot: {}", stderr_ch),
            ));
        }
        let kernel = get_kernel_version(tc)?;
        sanity_check(nd, &kernel, tc)?;
        let system_version = compute_system_version(nd)?;
        write_meta(nd, &format!("remove {}", package), &symlink, &kernel, &system_version, "ready")?;
        update_bootloader_entries(nd)?;
        let grub_cmd = format!("chroot {} /bin/sh -c 'update-grub'", tc);
        let (grub_success, _, grub_stderr) = run_command("/bin/sh", &["-c", &grub_cmd])?;
        if !grub_success {
            return Err(io::Error::new(
                io::ErrorKind::Other,
                format!("Failed to update grub in chroot: {}", grub_stderr),
            ));
        }
        bind_mounts_for_chroot(tc, false)?;
        let (um_success, _, um_stderr) = run_command("umount", &[tc])?;
        if !um_success {
            return Err(io::Error::new(
                io::ErrorKind::Other,
                format!("Failed to umount temp_chroot: {}", um_stderr),
            ));
        }
        mounted = false;
        set_subvolume_readonly(nd, true)?;
        switch_to_deployment_inner(nd)?;
        remove_transaction_marker()?;
        println!("Atomic remove completed. Reboot to apply.");
        Ok(())
    })();
    if result.is_err() {
        if let Some(nd) = &new_deployment {
            let _ = set_status_broken(nd);
        }
    }
    if mounted {
        if let Some(tc) = &temp_chroot {
            let _ = bind_mounts_for_chroot(tc, false);
            let _ = run_command("umount", &[tc]);
        }
    }
    if let Some(tc) = temp_chroot {
        if Path::new(&tc).exists() {
            let _ = fs::remove_dir_all(&tc);
        }
    }
    release_lock();
    result
}

fn create_deployment(writable: bool) -> Result<String, io::Error> {
    println!("Creating new deployment...");
    fs::create_dir_all(DEPLOYMENTS_DIR)?;
    let current = fs::read_link(CURRENT_SYMLINK)?.to_string_lossy().to_string();
    let timestamp = Local::now().format("%Y%m%d%H%M%S").to_string();
    let new_deployment = format!("{}/hammer-{}", DEPLOYMENTS_DIR, timestamp);
    snapshot_recursive(&current, &new_deployment, writable)?;
    if writable {
        set_readonly_recursive(&new_deployment, false)?;
    }
    println!("Deployment created at: {}", new_deployment);
    Ok(new_deployment)
}

fn switch_deployment(deployment: Option<String>) -> Result<(), io::Error> {
    acquire_lock()?;
    validate_system()?;
    println!("Switching deployment...");
    let target = match deployment {
        Some(d) => format!("{}/{}", DEPLOYMENTS_DIR, d),
        None => {
            let mut deployments = get_deployments()?;
            if deployments.len() < 2 {
                return Err(io::Error::new(
                    io::ErrorKind::Other,
                    "Not enough deployments for rollback.",
                ));
            }
            deployments.sort();
            deployments[deployments.len() - 2].clone()
        }
    };
    if !Path::new(&target).exists() {
        return Err(io::Error::new(
            io::ErrorKind::Other,
            format!("Deployment {} does not exist.", target),
        ));
    }
    let old_current = fs::read_link(CURRENT_SYMLINK)?.to_string_lossy().to_string();
    switch_to_deployment_inner(&target)?;
    update_meta(&old_current, Some("previous".to_string()), Some("manual".to_string()))?;
    println!("Switched to deployment: {}. Reboot to apply.", target);
    release_lock();
    Ok(())
}

fn switch_to_deployment_inner(deployment: &str) -> Result<(), io::Error> {
    let id = get_subvol_id(deployment)?;
    let (success, _, stderr) = run_command("btrfs", &["subvolume", "set-default", &id, "/"])?;
    if !success {
        return Err(io::Error::new(
            io::ErrorKind::Other,
            format!("Failed to set default subvolume: {}", stderr),
        ));
    }
    if Path::new(CURRENT_SYMLINK).exists() {
        fs::remove_file(CURRENT_SYMLINK)?;
    }
    symlink(deployment, CURRENT_SYMLINK)?;
    Ok(())
}

fn clean_up() -> Result<(), io::Error> {
    acquire_lock()?;
    validate_system()?;
    println!("Cleaning up unused resources...");
    let mut deployments = get_deployments()?;
    deployments.sort();
    if deployments.len() > 5 {
        for dep in deployments[0..deployments.len() - 5].iter() {
            let (success, _, stderr) = run_command("btrfs", &["subvolume", "delete", dep])?;
            if !success {
                eprintln!("Failed to delete deployment {}: {}", dep, stderr);
            }
        }
    }
    println!("Clean up completed.");
    release_lock();
    Ok(())
}

fn rollback(n: u32) -> Result<(), io::Error> {
    acquire_lock()?;
    validate_system()?;
    println!("Performing rollback...");
    let mut deployments = get_deployments()?;
    deployments.sort_by(|a, b| b.cmp(a)); // Assuming newer have higher timestamps
    if deployments.len() < (n as usize + 1) {
        return Err(io::Error::new(
            io::ErrorKind::Other,
            "Not enough deployments for rollback.",
        ));
    }
    let target = deployments[n as usize].clone();
    let old_current = fs::read_link(CURRENT_SYMLINK)?.to_string_lossy().to_string();
    switch_to_deployment_inner(&target)?;
    update_meta(
        &old_current,
        Some("rollback".to_string()),
                Some("user requested".to_string()),
    )?;
    println!("Rollback completed. Reboot to apply.");
    release_lock();
    Ok(())
}

fn atomic_refresh() -> Result<(), io::Error> {
    let mut new_deployment: Option<String> = None;
    let mut temp_chroot: Option<String> = None;
    let mut mounted = false;
    let result = (|| -> Result<(), io::Error> {
        acquire_lock()?;
        validate_system()?;
        ensure_top_mounted()?;
        println!("Performing atomic refresh...");
        new_deployment = Some(create_deployment(true)?);
        let nd = new_deployment.as_ref().unwrap();
        create_transaction_marker(nd)?;
        let symlink = fs::read_link(CURRENT_SYMLINK)?.file_name().unwrap().to_string_lossy().to_string();
        let device = get_root_device()?;
        let new_subvol = get_subvol_name(nd)?;
        temp_chroot = Some(create_temp_dir("hammer")?);
        let tc = temp_chroot.as_ref().unwrap();
        let (success, _, stderr) = run_command("mount", &["-o", &format!("subvol={}", new_subvol), &device, tc])?;
        if !success {
            return Err(io::Error::new(
                io::ErrorKind::Other,
                format!("Failed to mount temp_chroot: {}", stderr),
            ));
        }
        bind_mounts_for_chroot(tc, true)?;
        mounted = true;
        let chroot_cmd = format!(
            "chroot {} /bin/sh -c 'apt update && apt upgrade -y && apt autoremove -y && dpkg -l > /tmp/packages.list && update-initramfs -u -k all'",
            tc
        );
        let (success_ch, _, stderr_ch) = run_command("/bin/sh", &["-c", &chroot_cmd])?;
        if !success_ch {
            return Err(io::Error::new(
                io::ErrorKind::Other,
                format!("Failed to refresh in chroot: {}", stderr_ch),
            ));
        }
        let kernel = get_kernel_version(tc)?;
        sanity_check(nd, &kernel, tc)?;
        let system_version = compute_system_version(nd)?;
        write_meta(nd, "refresh", &symlink, &kernel, &system_version, "ready")?;
        update_bootloader_entries(nd)?;
        let grub_cmd = format!("chroot {} /bin/sh -c 'update-grub'", tc);
        let (grub_success, _, grub_stderr) = run_command("/bin/sh", &["-c", &grub_cmd])?;
        if !grub_success {
            return Err(io::Error::new(
                io::ErrorKind::Other,
                format!("Failed to update grub in chroot: {}", grub_stderr),
            ));
        }
        bind_mounts_for_chroot(tc, false)?;
        let (um_success, _, um_stderr) = run_command("umount", &[tc])?;
        if !um_success {
            return Err(io::Error::new(
                io::ErrorKind::Other,
                format!("Failed to umount temp_chroot: {}", um_stderr),
            ));
        }
        mounted = false;
        set_subvolume_readonly(nd, true)?;
        switch_to_deployment_inner(nd)?;
        remove_transaction_marker()?;
        println!("Atomic refresh completed. Reboot to apply.");
        Ok(())
    })();
    if result.is_err() {
        if let Some(nd) = &new_deployment {
            let _ = set_status_broken(nd);
        }
    }
    if mounted {
        if let Some(tc) = &temp_chroot {
            let _ = bind_mounts_for_chroot(tc, false);
            let _ = run_command("umount", &[tc]);
        }
    }
    if let Some(tc) = temp_chroot {
        if Path::new(&tc).exists() {
            let _ = fs::remove_dir_all(&tc);
        }
    }
    release_lock();
    result
}

fn init() -> Result<(), io::Error> {
    acquire_lock()?;
    ensure_top_mounted()?;
    fs::create_dir_all(DEPLOYMENTS_DIR)?;
    let timestamp = Local::now().format("%Y%m%d%H%M%S").to_string();
    let new_deployment = format!("{}/hammer-{}", DEPLOYMENTS_DIR, timestamp);
    snapshot_recursive(BTRFS_TOP, &new_deployment, false)?;
    let current = new_deployment;
    set_subvolume_readonly(&current, true)?;
    switch_to_deployment_inner(&current)?;
    let kernel = get_kernel_version("/")?;
    let system_version = compute_system_version(&current)?;
    write_meta(&current, "initial", "none", &kernel, &system_version, "current")?;
    println!("System initialized.");
    release_lock();
    Ok(())
}

fn list_deployments() -> Result<(), io::Error> {
    let deployments = get_deployments()?;
    for dep in deployments {
        let meta_path = format!("{}/.hammer-meta.json", dep);
        if Path::new(&meta_path).exists() {
            let json = fs::read_to_string(&meta_path)?;
            let meta: Meta = serde_json::from_str(&json)?;
            println!("Deployment: {}", dep);
            println!("Created: {}", meta.created);
            println!("Description: {}", meta.description);
            println!("Status: {}", meta.status);
            if let Some(reason) = meta.rollback_reason {
                println!("Rollback reason: {}", reason);
            }
            println!("---");
        } else {
            println!("Deployment: {} (no meta)", dep);
        }
    }
    Ok(())
}

fn status() -> Result<(), io::Error> {
    let current = fs::read_link(CURRENT_SYMLINK)?.to_string_lossy().to_string();
    println!("Current deployment: {}", current);
    let meta_path = format!("{}/.hammer-meta.json", current);
    if Path::new(&meta_path).exists() {
        let json = fs::read_to_string(&meta_path)?;
        let meta: Meta = serde_json::from_str(&json)?;
        println!("Kernel: {}", meta.kernel);
        println!("System version: {}", meta.system_version);
        println!("Status: {}", meta.status);
    }
    Ok(())
}

fn bind_mounts_for_chroot(chroot: &str, mount: bool) -> Result<(), io::Error> {
    let binds = vec!["/proc", "/sys", "/dev", "/run"];
    for b in binds {
        let target = format!("{}{}", chroot, b);
        if mount {
            if !Path::new(&target).exists() {
                fs::create_dir_all(&target)?;
            }
            let (success, _, stderr) = run_command("mount", &["--bind", b, &target])?;
            if !success {
                return Err(io::Error::new(
                    io::ErrorKind::Other,
                    format!("Failed to bind mount {}: {}", b, stderr),
                ));
            }
        } else {
            let (success, _, stderr) = run_command("umount", &[&target])?;
            if !success {
                eprintln!("Failed to umount {}: {}", target, stderr);
            }
        }
    }
    Ok(())
}

fn get_kernel_version(chroot: &str) -> Result<String, io::Error> {
    let cmd = format!("chroot {} /bin/sh -c 'uname -r'", chroot);
    let (success, stdout, stderr) = run_command("/bin/sh", &["-c", &cmd])?;
    if !success {
        return Err(io::Error::new(
            io::ErrorKind::Other,
            format!("Failed to get kernel version: {}", stderr),
        ));
    }
    Ok(stdout.trim().to_string())
}

fn sanity_check(_deployment: &str, _kernel: &str, _chroot: &str) -> Result<(), io::Error> {
    // Placeholder for sanity checks
    Ok(())
}

fn compute_system_version(deployment: &str) -> Result<String, io::Error> {
    let packages_list = format!("{}/tmp/packages.list", deployment);
    let mut file = File::open(packages_list)?;
    let mut contents = String::new();
    file.read_to_string(&mut contents)?;
    let mut hasher = Sha256::new();
    hasher.update(contents.as_bytes());
    let result = hasher.finalize();
    Ok(hex::encode(result))
}

fn write_meta(
    deployment: &str,
    description: &str,
    parent: &str,
    kernel: &str,
    system_version: &str,
    status: &str,
) -> Result<(), io::Error> {
    let meta = Meta {
        created: Local::now().to_rfc3339(),
        description: description.to_string(),
        parent: parent.to_string(),
        kernel: kernel.to_string(),
        system_version: system_version.to_string(),
        status: status.to_string(),
        rollback_reason: None,
    };
    let json = serde_json::to_string(&meta)?;
    let meta_path = format!("{}/.hammer-meta.json", deployment);
    let mut file = File::create(meta_path)?;
    file.write_all(json.as_bytes())?;
    Ok(())
}

fn update_meta(deployment: &str, status: Option<String>, rollback_reason: Option<String>) -> Result<(), io::Error> {
    let meta_path = format!("{}/.hammer-meta.json", deployment);
    if !Path::new(&meta_path).exists() {
        return Ok(());
    }
    let mut file = OpenOptions::new().read(true).write(true).open(&meta_path)?;
    let mut json_str = String::new();
    file.read_to_string(&mut json_str)?;
    let mut meta: Meta = serde_json::from_str(&json_str)?;
    if let Some(s) = status {
        meta.status = s;
    }
    if let Some(r) = rollback_reason {
        meta.rollback_reason = Some(r);
    }
    let new_json = serde_json::to_string(&meta)?;
    file.seek(SeekFrom::Start(0))?;
    file.write_all(new_json.as_bytes())?;
    file.set_len(new_json.len() as u64)?;
    Ok(())
}

fn set_subvolume_readonly(deployment: &str, ro: bool) -> Result<(), io::Error> {
    let value = if ro { "true" } else { "false" };
    let (success, _, stderr) = run_command("btrfs", &["property", "set", "-ts", deployment, "ro", value])?;
    if !success {
        return Err(io::Error::new(
            io::ErrorKind::Other,
            format!("Failed to set readonly: {}", stderr),
        ));
    }
    Ok(())
}

fn set_readonly_recursive(deployment: &str, ro: bool) -> Result<(), io::Error> {
    set_subvolume_readonly(deployment, ro)?;
    let (success, stdout, stderr) = run_command("btrfs", &["subvolume", "list", "-a", deployment])?;
    if !success {
        return Err(io::Error::new(
            io::ErrorKind::Other,
            format!("Failed to list subvols for readonly: {}", stderr),
        ));
    }
    let re = Regex::new(r"ID \d+ gen \d+ path (.*)").unwrap();
    for line in stdout.lines() {
        if let Some(captures) = re.captures(&line) {
            let path = captures.get(1).unwrap().as_str();
            if !path.is_empty() {
                let full_path = format!("{}/{}", deployment, path.replace("<FS_TREE>/", ""));
                set_subvolume_readonly(&full_path, ro)?;
            }
        }
    }
    Ok(())
}

fn create_transaction_marker(deployment: &str) -> Result<(), io::Error> {
    let marker_path = format!("{}/hammer-transaction", deployment);
    File::create(marker_path)?;
    Ok(())
}

fn remove_transaction_marker() -> Result<(), io::Error> {
    let _ = fs::remove_file(TRANSACTION_MARKER);
    Ok(())
}

fn set_status_broken(deployment: &str) -> Result<(), io::Error> {
    update_meta(deployment, Some("broken".to_string()), None)
}

fn get_subvol_id(deployment: &str) -> Result<String, io::Error> {
    let (success, stdout, stderr) = run_command("btrfs", &["subvolume", "show", deployment])?;
    if !success {
        return Err(io::Error::new(
            io::ErrorKind::Other,
            format!("Failed to get subvol id: {}", stderr),
        ));
    }
    for line in stdout.lines() {
        if line.contains("Subvolume ID:") {
            let parts: Vec<&str> = line.split_whitespace().collect();
            if parts.len() >= 3 {
                return Ok(parts[2].to_string());
            }
        }
    }
    Err(io::Error::new(io::ErrorKind::Other, "Subvolume ID not found."))
}

fn update_bootloader_entries(_deployment: &str) -> Result<(), io::Error> {
    // Placeholder, implement if specific bootloader updates are needed
    Ok(())
}

fn get_deployments() -> Result<Vec<String>, io::Error> {
    let mut deployments = Vec::new();
    if let Ok(entries) = fs::read_dir(DEPLOYMENTS_DIR) {
        for entry in entries {
            if let Ok(entry) = entry {
                let path = entry.path();
                if path.is_dir() {
                    if let Some(name) = path.file_name() {
                        if name.to_string_lossy().starts_with("hammer-") {
                            deployments.push(path.to_string_lossy().to_string());
                        }
                    }
                }
            }
        }
    }
    Ok(deployments)
}
