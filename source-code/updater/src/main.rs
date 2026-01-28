use anyhow::{anyhow, Context, Result};
use clap::{Parser, Subcommand};
use hammer_core::{
    calculate_dir_size, check_free_space, create_spinner, load_config, run_command, HammerConfig,
    Logger,
};
use nix::unistd::Uid;
use owo_colors::OwoColorize;
use sha2::{Digest, Sha256};
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Parser)]
#[command(name = "hammer-updater", about = "Atomic Updater for Hammer")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Initialize the OSTree repository
    Init,
    /// Check for updates
    Check,
    /// Perform an atomic update
    Update {
        /// Force update even if metadata matches
        #[arg(long)]
        force: bool,
            /// Show changes in /etc before applying
            #[arg(long)]
            preview_etc: bool,
    },
    /// Layer a local package onto the system (rebuilds image)
    Layer {
        /// Path to the .deb file
        path: String,
    },
    /// Rollback to previous deployment
    Rollback,
}

const OSTREE_REPO: &str = "/ostree/repo";
const BRANCH_NAME: &str = "debian/stable/amd64";
const CACHE_DIR: &str = "/var/cache/hammer/apt";
const LOCAL_PKG_DIR: &str = "/var/lib/hammer/local-pkgs";

fn main() -> Result<()> {
    if !Uid::current().is_root() {
        eprintln!("{}", "Updater must run as root.".red().bold());
        std::process::exit(1);
    }

    let cli = Cli::parse();

    match cli.command {
        Commands::Init => handle_init()?,
        Commands::Check => handle_check()?,
        Commands::Update { force, preview_etc } => handle_update(force, preview_etc, None)?,
        Commands::Layer { path } => handle_layer(path)?,
        Commands::Rollback => handle_rollback()?,
    }

    Ok(())
}

fn handle_init() -> Result<()> {
    Logger::info("Initializing OSTree repository...");

    if !Path::new(OSTREE_REPO).exists() {
        fs::create_dir_all(OSTREE_REPO)?;
        run_command(
            "ostree",
            &["--repo", OSTREE_REPO, "init", "--mode=bare-user"],
            "OSTree Init",
        )?;
        Logger::success("Initialized /ostree/repo");
    } else {
        Logger::info("OSTree repository already exists.");
    }

    // Initialize Cache Dirs
    fs::create_dir_all(CACHE_DIR)?;
    fs::create_dir_all(LOCAL_PKG_DIR)?;

    // Ensure config exists
    let _ = load_config()?;

    Ok(())
}

fn compute_update_hash(config: &HammerConfig) -> Result<String> {
    let spinner = create_spinner("Fetching remote repository metadata...");

    let release_url = format!(
        "{}/dists/{}/Release",
        config.repository.url, config.repository.suite
    );
    let resp = reqwest::blocking::get(&release_url)
    .context("Failed to fetch Release file")?
    .text()?;

    let mut hasher = Sha256::new();
    hasher.update(resp.as_bytes());

    hasher.update(config.packages.include.join(",").as_bytes());
    hasher.update(config.packages.exclude.join(",").as_bytes());

    // Include local packages in hash calculation
    if Path::new(LOCAL_PKG_DIR).exists() {
        for entry in fs::read_dir(LOCAL_PKG_DIR)? {
            let entry = entry?;
            let path = entry.path();
            if path.extension().map_or(false, |e| e == "deb") {
                hasher.update(entry.file_name().as_encoded_bytes());
            }
        }
    }

    let hash = hex::encode(hasher.finalize());
    spinner.finish_with_message("Metadata fetched.");

    Ok(hash)
}

fn get_current_commit_hash() -> Result<Option<String>> {
    let output = std::process::Command::new("ostree")
    .args(&[
        "show",
        "--print-metadata-key=hammer.update-hash",
        BRANCH_NAME,
    ])
    .output();

    match output {
        Ok(out) => {
            if out.status.success() {
                let s = String::from_utf8_lossy(&out.stdout)
                .trim()
                .replace("'", "");
                if s.is_empty() || s == "null" {
                    Ok(None)
                } else {
                    Ok(Some(s))
                }
            } else {
                Ok(None)
            }
        }
        Err(_) => Ok(None),
    }
}

fn handle_check() -> Result<()> {
    let config = load_config()?;
    let remote_hash = compute_update_hash(&config)?;
    let local_hash = get_current_commit_hash()?;

    if let Some(local) = local_hash {
        if local == remote_hash {
            Logger::success("System is up-to-date (Hashes match).");
            return Ok(());
        }
        println!(
            "Update available:\nLocal:  {}\nRemote: {}",
            local.yellow(),
                 remote_hash.green()
        );
    } else {
        println!("No local metadata found. Update recommended.");
    }

    Ok(())
}

fn handle_layer(path: String) -> Result<()> {
    Logger::info(&format!("Layering local package: {}", path));

    let src_path = Path::new(&path);
    if !src_path.exists() {
        return Err(anyhow!("Package file not found: {}", path));
    }

    // Ensure local pkg dir exists
    fs::create_dir_all(LOCAL_PKG_DIR)?;

    let file_name = src_path.file_name().ok_or(anyhow!("Invalid filename"))?;
    let dest_path = Path::new(LOCAL_PKG_DIR).join(file_name);

    fs::copy(src_path, &dest_path)?;
    Logger::success(&format!("Package added to staging: {:?}", dest_path));

    // Force update to rebuild image with new package
    Logger::info("Triggering system rebuild to apply layer...");
    handle_update(true, false, Some(dest_path))?;

    Ok(())
}

fn handle_update(force: bool, preview_etc: bool, _new_pkg: Option<PathBuf>) -> Result<()> {
    let config = load_config()?;

    // 1. Idempotency Check
    let update_hash = compute_update_hash(&config)?;
    if !force {
        let current_hash = get_current_commit_hash()?;
        if let Some(current) = current_hash {
            if current == update_hash {
                Logger::success(
                    "No changes detected in repository or config. System is up to date.",
                );
                return Ok(());
            }
        }
    }

    if preview_etc {
        show_etc_preview()?;
    }

    // 2. Prepare Staging Area
    let temp_dir = tempfile::tempdir()?;
    let rootfs = temp_dir.path().join("rootfs");
    fs::create_dir(&rootfs)?;

    Logger::info(&format!(
        "Starting atomic update. Staging in {:?}",
        rootfs
    ));

    // Ensure cache directory exists
    fs::create_dir_all(CACHE_DIR)?;

    // 3. Build Rootfs with mmdebstrap
    let spinner = create_spinner("Building new system image (mmdebstrap)...");

    let include_str = config.packages.include.join(",");

    // Binding values for argument lifetime
    let include_flag = format!("--include={}", include_str);
    let rootfs_str = rootfs.to_str().unwrap();

    // Setup apt cache
    // We mount the host CACHE_DIR into the container/chroot
    let apt_opt_cache = format!("Dir::Cache::Archives \"{}\";", CACHE_DIR);

    // Setup local packages
    // If we have packages in LOCAL_PKG_DIR, we need to tell mmdebstrap to install them.
    // The easiest way is to use --include with path, but mmdebstrap needs them reachable.
    // We'll skip complex logic for now and assume the 'include' list covers repo packages,
    // and we append any local .deb files to the command line if supported,
    // or copy them in via hook.

    let mut args = vec![
        "--variant=minbase",
        &include_flag,
        "--format=directory",
        "--aptopt", &apt_opt_cache, // USE CACHE!
        &config.repository.suite,
        rootfs_str,
        &config.repository.url,
    ];

    // If local packages exist, we need to install them.
    // Since mmdebstrap doesn't natively take a list of local .debs in main args easily mixed with repo,
    // we use a setup-hook to copy them in and dpkg -i them.
    let hook_cmd = format!(
        "copy-in {} /tmp/local-pkgs && chroot \"$1\" dpkg -iR /tmp/local-pkgs && chroot \"$1\" rm -rf /tmp/local-pkgs",
        LOCAL_PKG_DIR
    );

    let setup_hook_arg = format!("--setup-hook={}", hook_cmd);

    // Only add hook if directory is not empty
    let has_local = Path::new(LOCAL_PKG_DIR).exists() && fs::read_dir(LOCAL_PKG_DIR)?.next().is_some();
    if has_local {
        args.insert(1, &setup_hook_arg);
    }

    run_command("mmdebstrap", &args, "Building RootFS")?;

    // Cleanup excluded packages
    if config.packages.exclude.contains(&"apt".to_string()) {
        run_command(
            "rm",
            &["-rf", rootfs.join("var/lib/apt").to_str().unwrap()],
                    "Cleaning APT",
        )?;
        run_command(
            "rm",
            &["-f", rootfs.join("usr/bin/apt").to_str().unwrap()],
                    "Removing APT binary",
        )?;
    }

    spinner.finish_with_message("RootFS built.");

    // --- PRE-FLIGHT CHECKS ---
    Logger::info("Running Pre-flight Checks...");

    // A. Kernel Check
    verify_kernel(&rootfs)?;

    // B. Disk Space Check
    let rootfs_size = calculate_dir_size(&rootfs)?;
    let required_space = rootfs_size * 2;
    check_free_space(OSTREE_REPO, required_space)?;
    Logger::success(&format!(
        "Disk space check passed (Image size: {:.2} MB)",
                             rootfs_size as f64 / 1024.0 / 1024.0
    ));

    // 4. Commit to OSTree
    let spinner_ostree = create_spinner("Committing to OSTree...");

    let timestamp = chrono::Local::now().format("%Y%m%d-%H%M%S").to_string();

    run_command(
        "ostree",
        &[
            "--repo",
            OSTREE_REPO,
            "commit",
            "--branch",
            BRANCH_NAME,
            "--subject",
            &format!("Update {}", timestamp),
                "--add-metadata-string",
                &format!("hammer.update-hash={}", update_hash),
                "--tree",
                &format!("dir={}", rootfs.to_str().unwrap()),
        ],
        "OSTree Commit",
    )?;

    spinner_ostree.finish_with_message("Changes committed.");

    // 5. Deploy & Bootloader
    deploy_commit(BRANCH_NAME)?;
    update_bootloader()?;

    Logger::success("Update complete. Reboot to apply changes.");
    Ok(())
}

fn verify_kernel(rootfs: &Path) -> Result<()> {
    let boot_dir = rootfs.join("boot");
    if !boot_dir.exists() {
        return Err(anyhow!("Pre-flight Failed: /boot directory missing in rootfs!"));
    }

    let mut found_kernel = false;
    let mut found_initrd = false;

    for entry in fs::read_dir(boot_dir)? {
        let entry = entry?;
        let name = entry.file_name().to_string_lossy().to_string();
        if name.starts_with("vmlinuz") {
            found_kernel = true;
        }
        if name.starts_with("initrd.img") {
            found_initrd = true;
        }
    }

    if !found_kernel {
        return Err(anyhow!("Pre-flight Failed: No vmlinuz found in rootfs."));
    }
    if !found_initrd {
        return Err(anyhow!("Pre-flight Failed: No initrd.img found in rootfs."));
    }

    Logger::success("Pre-flight: Kernel and Initrd verified.");
    Ok(())
}

fn show_etc_preview() -> Result<()> {
    Logger::info("Analyzing configuration changes (/etc)...");

    let status = std::process::Command::new("ostree")
    .args(&["admin", "config-diff"])
    .status();

    if let Ok(s) = status {
        if s.success() {
            println!("{}", "No changes detected in /etc.".green());
        } else {
            Logger::info("Configuration differences found above.");
        }
    } else {
        Logger::error("Failed to run config-diff");
    }

    Ok(())
}

fn deploy_commit(ref_name: &str) -> Result<()> {
    Logger::info("Deploying new commit...");

    if !Path::new("/ostree/deploy/debian").exists() {
        run_command("ostree", &["admin", "os-init", "debian"], "OS Init")?;
    }

    run_command(
        "ostree",
        &["admin", "deploy", "debian", ref_name],
        "OSTree Deploy",
    )?;

    Ok(())
}

fn update_bootloader() -> Result<()> {
    Logger::info("Updating Bootloader Configuration...");

    if !Path::new("/boot/grub").exists() {
        return Ok(());
    }

    let status = run_command("update-grub", &[], "Updating GRUB");

    if status.is_err() {
        run_command(
            "grub-mkconfig",
            &["-o", "/boot/grub/grub.cfg"],
            "Generating GRUB Config",
        )?;
    }

    Ok(())
}

fn handle_rollback() -> Result<()> {
    Logger::info("Rolling back to previous deployment...");

    run_command("ostree", &["admin", "undeploy", "0"], "Rollback")?;

    update_bootloader()?;

    Logger::success("Rolled back. Reboot to enter previous state.");
    Ok(())
}
