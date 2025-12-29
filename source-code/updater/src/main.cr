require "option_parser"
require "file_utils"
require "time"
require "json"
require "digest/sha256"

if LibC.getuid != 0
  puts "This tool must be run as root."
  exit(1)
end

BTRFS_TOP = "/btrfs-root"
DEPLOYMENTS_DIR = "/btrfs-root/deployments"
CURRENT_SYMLINK = "/btrfs-root/current"
LOCK_FILE = "/run/hammer.lock"
TRANSACTION_MARKER = "/btrfs-root/hammer-transaction"

LOG_DIR = "/usr/lib/HackerOS/hammer/logs"
LOG_FILE = "#{LOG_DIR}/hammer-updater.log"

def log(message : String)
  Dir.mkdir_p(LOG_DIR)
  File.open(LOG_FILE, "a") do |f|
    f.puts "#{Time.local.to_s("%Y-%m-%d %H:%M:%S")} - #{message}"
  end
end

def run_command(cmd : String, args : Array(String)) : {success: Bool, stdout: String, stderr: String}
  stdout = IO::Memory.new
  stderr = IO::Memory.new
  status = Process.run(cmd, args: args, output: stdout, error: stderr)
  {success: status.success?, stdout: stdout.to_s, stderr: stderr.to_s}
end

def acquire_lock
  if File.exists?(LOCK_FILE)
    log("Failed to acquire lock: operation in progress")
    raise "Hammer operation in progress (lock file exists)."
  end
  File.touch(LOCK_FILE)
  log("Acquired lock")
end

def release_lock
  File.delete(LOCK_FILE) if File.exists?(LOCK_FILE)
  log("Released lock")
end

def validate_system
  # Check if root is BTRFS
  output = run_command("btrfs", ["filesystem", "show", "/"])
  unless output[:success]
    log("Root filesystem is not BTRFS")
    raise "Root filesystem is not BTRFS."
  end
  # Check current symlink exists
  unless File.symlink?(CURRENT_SYMLINK)
    log("Current deployment symlink missing")
    raise "Current deployment symlink missing. System may not be initialized."
  end
  # Check current is read-only
  current = File.readlink(CURRENT_SYMLINK)
  prop_output = run_command("btrfs", ["property", "get", "-ts", current, "ro"])
  unless prop_output[:success] && prop_output[:stdout].strip == "ro=true"
    log("Current deployment is not read-only")
    raise "Current deployment is not read-only."
  end
  log("System validated")
end

def atomic_update
  new_deployment : String? = nil
  mounted = false
  begin
    acquire_lock
    validate_system
    log("Performing atomic system update")
    puts "Performing atomic system update..."
    # Create new deployment
    new_deployment = create_deployment(true)
    create_transaction_marker(new_deployment)
    parent = File.basename(File.readlink(CURRENT_SYMLINK))
    bind_mounts_for_chroot(new_deployment, true)
    mounted = true
    chroot_cmd = "chroot #{new_deployment} /bin/sh -c 'apt update && apt upgrade -y && apt autoremove -y && dpkg -l > /tmp/packages.list && update-initramfs -u -k all && update-grub'"
    output = run_command("/bin/sh", ["-c", chroot_cmd])
    unless output[:success]
      log("Failed to update in chroot: #{output[:stderr]}")
      raise "Failed to update in chroot: #{output[:stderr]}"
    end
    bind_mounts_for_chroot(new_deployment, false)
    mounted = false
    kernel = get_kernel_version(new_deployment)
    sanity_check(new_deployment, kernel)
    system_version = compute_system_version(new_deployment)
    write_meta(new_deployment, "update", parent, kernel, system_version, "ready")
    update_bootloader_entries(new_deployment)
    set_subvolume_readonly(new_deployment, true)
    switch_to_deployment(new_deployment)
    remove_transaction_marker
    log("Atomic update completed")
    puts "Atomic update completed. Reboot to apply."
  rescue ex : Exception
    log("Error during atomic update: #{ex.message}")
    if new_deployment
      set_status_broken(new_deployment)
    end
    raise ex
  ensure
    if mounted && new_deployment
      bind_mounts_for_chroot(new_deployment, false) rescue nil
    end
    release_lock
  end
end

def create_deployment(writable : Bool) : String
  log("Creating new deployment")
  puts "Creating new deployment..."
  Dir.mkdir_p(DEPLOYMENTS_DIR)
  current = File.readlink(CURRENT_SYMLINK)
  timestamp = Time.local.to_s("%Y%m%d%H%M%S")
  new_deployment = "#{DEPLOYMENTS_DIR}/hammer-#{timestamp}"
  args = ["subvolume", "snapshot"]
  args << "-r" unless writable
  args << current
  args << new_deployment
  output = run_command("btrfs", args)
  unless output[:success]
    log("Failed to create deployment: #{output[:stderr]}")
    raise "Failed to create deployment: #{output[:stderr]}"
  end
  set_subvolume_readonly(new_deployment, false) if writable
  log("Deployment created at: #{new_deployment}")
  puts "Deployment created at: #{new_deployment}"
  new_deployment
end

def switch_to_deployment(deployment : String)
  id = get_subvol_id(deployment)
  output = run_command("btrfs", ["subvolume", "set-default", id, "/"])
  unless output[:success]
    log("Failed to set default subvolume: #{output[:stderr]}")
    raise "Failed to set default subvolume: #{output[:stderr]}"
  end
  File.delete(CURRENT_SYMLINK) if File.exists?(CURRENT_SYMLINK)
  File.symlink(deployment, CURRENT_SYMLINK)
end

def get_subvol_id(path : String) : String
  output = run_command("btrfs", ["subvolume", "show", path])
  unless output[:success]
    log("Failed to get subvolume ID: #{output[:stderr]}")
    raise "Failed to get subvolume ID."
  end
  output[:stdout].lines.each do |line|
    if line.includes?("Subvolume ID:")
      parts = line.split(":")
      return parts[1].strip if parts.size > 1
    end
  end
  log("Subvolume ID not found")
  raise "Subvolume ID not found."
end

def set_subvolume_readonly(path : String, readonly : Bool)
  value = readonly ? "true" : "false"
  output = run_command("btrfs", ["property", "set", "-ts", path, "ro", value])
  unless output[:success]
    log("Failed to set readonly #{value}: #{output[:stderr]}")
    raise "Failed to set readonly #{value}: #{output[:stderr]}"
  end
end

def bind_mounts_for_chroot(chroot_path : String, mount : Bool)
  dirs = ["proc", "sys", "dev"]
  dirs.each do |dir|
    target = "#{chroot_path}/#{dir}"
    Dir.mkdir_p(target)
    if mount
      output = run_command("mount", ["--bind", "/#{dir}", target])
    else
      output = run_command("umount", [target])
    end
    unless output[:success]
      log("Failed to #{mount ? "mount" : "umount"} #{dir}: #{output[:stderr]}")
      raise "Failed to #{mount ? "mount" : "umount"} #{dir}: #{output[:stderr]}"
    end
  end
end

def get_kernel_version(chroot_path : String) : String
  cmd = "chroot #{chroot_path} /bin/sh -c \"dpkg -l | grep ^ii | grep linux-image | awk '{print \\$3}' | sort -V | tail -1\""
  output = run_command("/bin/sh", ["-c", cmd])
  unless output[:success]
    log("Failed to get kernel version: #{output[:stderr]}")
    raise "Failed to get kernel version: #{output[:stderr]}"
  end
  output[:stdout].strip
end

def write_meta(deployment : String, action : String, parent : String, kernel : String, system_version : String, status : String = "ready", rollback_reason : String? = nil)
  meta = {
    "created" => Time.utc.to_rfc3339,
    "action" => action,
    "parent" => parent,
    "kernel" => kernel,
    "system_version" => system_version,
    "status" => status,
    "rollback_reason" => rollback_reason,
  }.reject { |k, v| v.nil? }
  File.write("#{deployment}/meta.json", meta.to_json)
end

def set_status_broken(deployment : String)
  update_meta(deployment, status: "broken")
  log("Set deployment #{deployment} to broken")
end

def update_meta(deployment : String, **updates)
  meta = read_meta(deployment)
  updates.each { |k, v| meta[k.to_s] = v.to_s if v }
  File.write("#{deployment}/meta.json", meta.to_json)
end

def read_meta(deployment : String) : Hash(String, String)
  meta_path = "#{deployment}/meta.json"
  if File.exists?(meta_path)
    JSON.parse(File.read(meta_path)).as_h.transform_values(&.to_s)
  else
    {} of String => String
  end
end

def create_transaction_marker(deployment : String)
  data = {"deployment" => File.basename(deployment)}
  File.write(TRANSACTION_MARKER, data.to_json)
end

def remove_transaction_marker
  File.delete(TRANSACTION_MARKER) if File.exists?(TRANSACTION_MARKER)
end

def sanity_check(deployment : String, kernel : String)
  unless File.exists?("#{deployment}/boot/vmlinuz-#{kernel}")
    log("Kernel file missing: /boot/vmlinuz-#{kernel}")
    raise "Kernel file missing: /boot/vmlinuz-#{kernel}"
  end
  unless File.exists?("#{deployment}/boot/initrd.img-#{kernel}")
    log("Initramfs file missing: /boot/initrd.img-#{kernel}")
    raise "Initramfs file missing: /boot/initrd.img-#{kernel}"
  end
  # Check fstab
  cmd = "chroot #{deployment} /bin/mount -f -a"
  output = run_command("/bin/sh", ["-c", cmd])
  unless output[:success]
    log("Fstab sanity check failed: #{output[:stderr]}")
    raise "Fstab sanity check failed: #{output[:stderr]}"
  end
end

def compute_system_version(deployment : String) : String
  packages_file = "#{deployment}/tmp/packages.list"
  if File.exists?(packages_file)
    content = File.read(packages_file)
    hash = Digest::SHA256.hexdigest(content)
    File.delete(packages_file)
    hash
  else
    log("Packages list not found for version computation")
    raise "Packages list not found for version computation"
  end
end

def get_fs_uuid : String
  output = run_command("btrfs", ["filesystem", "show", "/"])
  unless output[:success]
    log("Failed to get BTRFS UUID: #{output[:stderr]}")
    raise "Failed to get BTRFS UUID: #{output[:stderr]}"
  end
  output[:stdout].lines.each do |line|
    if line.includes?("uuid:")
      return line.split("uuid:")[1].strip
    end
  end
  log("BTRFS UUID not found")
  raise "BTRFS UUID not found"
end

def update_bootloader_entries(deployment : String)
  good_deployments = get_deployments.select do |dep|
    meta = read_meta(dep)
    ["ready", "booted"].includes?(meta["status"]? || "unknown")
  end.sort_by do |dep|
    Time.parse_rfc3339(read_meta(dep)["created"]? || "1970-01-01T00:00:00Z")
  end.reverse[0...5] # Limit to last 5 good deployments
  entries = [] of String
  uuid = get_fs_uuid
  good_deployments.each do |dep|
    name = File.basename(dep)
    meta = read_meta(dep)
    kernel = meta["kernel"]? || next
    entry = <<-ENTRY
menuentry 'HammerOS (#{name})' --class gnu-linux --class gnu --class os $menuentry_id_option 'gnulinux-#{name}-advanced-#{uuid}' {
  insmod gzio
  insmod part_gpt
  insmod btrfs
  search --no-floppy --fs-uuid --set=root #{uuid}
  echo 'Loading Linux #{kernel} ...'
  linux /deployments/#{name}/boot/vmlinuz-#{kernel} root=UUID=#{uuid} rw rootflags=subvol=deployments/#{name} quiet splash $vt_handoff
  echo 'Loading initial ramdisk ...'
  initrd /deployments/#{name}/boot/initrd.img-#{kernel}
}
ENTRY
    entries << entry
  end
  script_content = <<-SCRIPT
#!/bin/sh
exec tail -n +3 $0
# This file provides HammerOS deployment entries
#{entries.join("\n")}
SCRIPT
  grub_file = "#{deployment}/etc/grub.d/25_hammer_entries"
  File.write(grub_file, script_content)
  File.chmod(grub_file, 0o755)
end

def get_deployments : Array(String)
  Dir.entries(DEPLOYMENTS_DIR).select(&.starts_with?("hammer-")).map { |f| File.join(DEPLOYMENTS_DIR, f) }
rescue ex : Exception
  log("Failed to list deployments: #{ex.message}")
  raise "Failed to list deployments: #{ex.message}"
end

if ARGV.empty?
  puts "No subcommand provided. Use 'update'."
  exit(1)
end

subcommand = ARGV.shift
begin
  case subcommand
  when "update"
    atomic_update
  else
    puts "Unknown subcommand: #{subcommand}"
    exit(1)
  end
rescue ex : Exception
  log("Error in subcommand #{subcommand}: #{ex.message}")
  STDERR.puts "Error: #{ex.message}"
  exit(1)
end
