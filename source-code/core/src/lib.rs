use anyhow::{Context, Result, anyhow};
use indicatif::{ProgressBar, ProgressStyle};
use owo_colors::OwoColorize;
use serde::{Deserialize, Serialize};
use std::fs::{self, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::Duration;
use walkdir::WalkDir;
use nix::sys::statvfs::statvfs;

pub const LOG_DIR: &str = "/usr/lib/HackerOS/hammer/logs";
pub const CONFIG_PATH: &str = "/etc/hammer/config.toml";
pub const SOURCE_LIST_HK: &str = "/etc/hammer/source-list.hk";
pub const APT_SOURCES: &str = "/etc/apt/sources.list";

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct HammerConfig {
    pub repository: RepositoryConfig,
    pub packages: PackagesConfig,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct RepositoryConfig {
    pub url: String,
    pub suite: String,
    pub components: Vec<String>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct PackagesConfig {
    pub include: Vec<String>,
    pub exclude: Vec<String>,
}

impl Default for HammerConfig {
    fn default() -> Self {
        Self {
            repository: RepositoryConfig {
                url: "http://deb.debian.org/debian".to_string(),
                suite: "bookworm".to_string(),
                components: vec!["main".to_string()],
            },
            packages: PackagesConfig {
                include: vec!["linux-image-amd64".to_string(), "systemd".to_string(), "coreutils".to_string()],
                exclude: vec!["apt".to_string(), "dpkg".to_string()],
            },
        }
    }
}

pub fn load_config() -> Result<HammerConfig> {
    // 1. Load Base Config (Package lists etc)
    let mut config = if Path::new(CONFIG_PATH).exists() {
        let content = fs::read_to_string(CONFIG_PATH).context("Failed to read config file")?;
        toml::from_str(&content).context("Failed to parse config file")?
    } else {
        HammerConfig::default()
    };

    // 2. Override Repository Sources (Priority: .hk -> apt sources -> toml default)
    if Path::new(SOURCE_LIST_HK).exists() {
        Logger::info(&format!("Loading sources from {}", SOURCE_LIST_HK));
        if let Ok(repo_config) = parse_hk_file(SOURCE_LIST_HK) {
            config.repository = repo_config;
            return Ok(config);
        }
    } else if Path::new(APT_SOURCES).exists() {
        Logger::info(&format!("Loading sources from {}", APT_SOURCES));
        if let Ok(repo_config) = parse_apt_sources(APT_SOURCES) {
            config.repository = repo_config;
            return Ok(config);
        }
    }

    Ok(config)
}

/// Parses the custom HackerOS .hk format
/// Format:
/// [section]
/// -> key => value
fn parse_hk_file(path: &str) -> Result<RepositoryConfig> {
    let file = fs::File::open(path)?;
    let reader = BufReader::new(file);

    let mut url = String::new();
    let mut suite = String::new();
    let mut components = Vec::new();

    // Regex for: -> key => value
    let re = regex::Regex::new(r"^\s*->\s*(.*?)\s*=>\s*(.*)$")?;

    for line in reader.lines() {
        let line = line?;
        let trimmed = line.trim();

        if trimmed.starts_with('!') || trimmed.is_empty() {
            continue;
        }

        if let Some(caps) = re.captures(trimmed) {
            let key = caps.get(1).map_or("", |m| m.as_str()).trim();
            let value = caps.get(2).map_or("", |m| m.as_str()).trim();

            match key {
                "url" | "mirror" => url = value.to_string(),
                "suite" | "dist" => suite = value.to_string(),
                "components" => {
                    components = value.split([',', ' '])
                    .filter(|s| !s.is_empty())
                    .map(|s| s.to_string())
                    .collect();
                }
                _ => {}
            }
        }
    }

    if url.is_empty() {
        return Err(anyhow!("No 'url' found in .hk file"));
    }
    if suite.is_empty() {
        suite = "stable".to_string(); // Fallback
    }
    if components.is_empty() {
        components = vec!["main".to_string()];
    }

    Ok(RepositoryConfig { url, suite, components })
}

fn parse_apt_sources(path: &str) -> Result<RepositoryConfig> {
    let content = fs::read_to_string(path)?;
    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("deb ") {
            let parts: Vec<&str> = trimmed.split_whitespace().collect();
            if parts.len() >= 4 {
                let url = parts[1].to_string();
                let suite = parts[2].to_string();
                let components = parts[3..].iter().map(|s| s.to_string()).collect();
                return Ok(RepositoryConfig { url, suite, components });
            }
        }
    }
    Err(anyhow!("No valid deb line found"))
}

pub struct Logger;

impl Logger {
    pub fn init() -> Result<()> {
        if !Path::new(LOG_DIR).exists() {
            fs::create_dir_all(LOG_DIR).context("Failed to create log directory")?;
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
        println!("{} {}", "INFO".blue().bold(), message);
        Self::log(&format!("INFO: {}", message));
    }

    pub fn error(message: &str) {
        eprintln!("{} {}", "ERROR".red().bold(), message);
        Self::log(&format!("ERROR: {}", message));
    }

    pub fn success(message: &str) {
        println!("{} {}", "SUCCESS".green().bold(), message);
        Self::log(&format!("SUCCESS: {}", message));
    }
}

pub fn create_spinner(msg: &str) -> ProgressBar {
    let pb = ProgressBar::new_spinner();
    pb.set_style(
        ProgressStyle::default_spinner()
        .tick_strings(&[
            "⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"
        ])
        .template("{spinner:.blue} {msg}")
        .unwrap(),
    );
    pb.set_message(msg.to_string());
    pb.enable_steady_tick(Duration::from_millis(80));
    pb
}

pub fn run_command(cmd: &str, args: &[&str], description: &str) -> Result<()> {
    Logger::log(&format!("Running: {} {}", cmd, args.join(" ")));

    let output = Command::new(cmd)
    .args(args)
    .stdout(Stdio::piped())
    .stderr(Stdio::piped())
    .output()
    .context(format!("Failed to execute {}", description))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        Logger::error(&format!("Command failed: {}\n{}", description, stderr));
        return Err(anyhow::anyhow!("{} failed: {}", description, stderr));
    }
    Ok(())
}

// --- Pre-flight Utils ---

pub fn calculate_dir_size(path: &Path) -> Result<u64> {
    let mut total_size = 0;
    for entry in WalkDir::new(path) {
        let entry = entry?;
        let metadata = entry.metadata()?;
        if metadata.is_file() {
            total_size += metadata.len();
        }
    }
    Ok(total_size)
}

pub fn check_free_space(path: &str, required_bytes: u64) -> Result<()> {
    let stat = statvfs(path)?;
    // Use rust-style getters provided by nix crate
    let available_bytes = stat.blocks_available() as u64 * stat.fragment_size() as u64;

    if available_bytes < required_bytes {
        return Err(anyhow!(
            "Insufficient disk space on {}. Required: {:.2} MB, Available: {:.2} MB",
            path,
            required_bytes as f64 / 1024.0 / 1024.0,
            available_bytes as f64 / 1024.0 / 1024.0
        ));
    }
    Ok(())
}
