require "option_parser"
require "file_utils"
require "time"
require "json"
require "digest/sha256"

if LibC.getuid != 0
  puts "This tool must be run as root."
  exit(1)
end

CONTAINER_TOOL = "podman"
CONTAINER_NAME_PREFIX = "hammer-container-"
CONTAINER_IMAGE = "debian:stable"
BINARY_MAP = {
  "golang" => "go",
}

LOG_DIR = "/usr/lib/HackerOS/hammer/logs"
LOG_FILE = "#{LOG_DIR}/hammer-containers.log"

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

def run_as_user(user : String, cmd : String) : {success: Bool, stdout: String, stderr: String}
  stdout = IO::Memory.new
  stderr = IO::Memory.new
  status = Process.run("su", args: ["-", user, "-c", cmd], output: stdout, error: stderr)
  {success: status.success?, stdout: stdout.to_s, stderr: stderr.to_s}
end

def parse_install_remove(args : Array(String)) : {package: String, gui: Bool}
  gui = false
  package = ""
  parser = OptionParser.new do |p|
    p.banner = "Usage: [subcommand] [options] package"
    p.on("--gui", "Install as GUI application") { gui = true }
    p.invalid_option do |flag|
      STDERR.puts "Invalid option: #{flag}."
      exit(1)
    end
    p.missing_option do |flag|
      STDERR.puts "Missing option for #{flag}."
      exit(1)
    end
    p.unknown_args do |uargs|
      package = uargs[0] if uargs.size > 0
    end
  end
  parser.parse(args)
  if package.empty?
    STDERR.puts "Package name required."
    exit(1)
  end
  {package: package, gui: gui}
end

def install_package(package : String, gui : Bool)
  log("Installing package: #{package} (gui: #{gui})")
  puts "Installing package: #{package} (gui: #{gui})"
  container_install(package, gui)
end

def remove_package(package : String, gui : Bool)
  log("Removing package: #{package} (gui: #{gui})")
  puts "Removing package: #{package} (gui: #{gui})"
  container_remove(package, gui)
end

def container_install(package : String, gui : Bool)
  if gui
    user = ENV["SUDO_USER"]? || begin
      puts "This command with --gui must be run using sudo by a non-root user."
      log("Error: --gui requires sudo by non-root user")
      exit(1)
    end
    container_name = "hammer-gui"
    # Ensure distrobox exists
    check_container = run_as_user(user, "distrobox list | grep -q '#{container_name}'")
    if !check_container[:success]
      create_output = run_as_user(user, "distrobox create --name #{container_name} --image #{CONTAINER_IMAGE} --yes")
      unless create_output[:success]
        log("Failed to create distrobox container: #{create_output[:stderr]}")
        raise "Failed to create distrobox container: #{create_output[:stderr]}"
      end
      log("Created distrobox container: #{container_name}")
      puts "Created distrobox container: #{container_name}"
    end
    # Check if already installed
    check_output = run_as_user(user, "distrobox enter #{container_name} -- dpkg -s #{package}")
    if check_output[:success]
      log("Package #{package} already installed in distrobox")
      puts "Package #{package} is already installed in the distrobox."
      return
    end
    # Setup sources and architecture
    sources_setup_cmd = "sudo bash -c 'if [ ! -f /etc/apt/sources.list ]; then echo \\\"deb http://deb.debian.org/debian stable main contrib non-free non-free-firmware\\\" > /etc/apt/sources.list; else sed -i \\\"s/main$/main contrib non-free non-free-firmware/g\\\" /etc/apt/sources.list; fi'"
    sources_setup = run_as_user(user, "distrobox enter #{container_name} -- #{sources_setup_cmd}")
    if !sources_setup[:success]
      log("Warning: Failed to setup sources: #{sources_setup[:stderr]}")
      puts "Warning: Failed to setup sources: #{sources_setup[:stderr]}"
    end
    arch_setup = run_as_user(user, "distrobox enter #{container_name} -- sudo dpkg --add-architecture i386")
    if !arch_setup[:success]
      log("Warning: Failed to add i386 architecture: #{arch_setup[:stderr]}")
      puts "Warning: Failed to add i386 architecture: #{arch_setup[:stderr]}"
    end
    # Update
    update_output = run_as_user(user, "distrobox enter #{container_name} -- sudo apt update")
    unless update_output[:success]
      log("Failed to update in distrobox: #{update_output[:stderr]}")
      raise "Failed to update in distrobox: #{update_output[:stderr]}"
    end
    # Install
    install_output = run_as_user(user, "distrobox enter #{container_name} -- sudo apt install -y #{package}")
    unless install_output[:success]
      log("Failed to install package in distrobox: #{install_output[:stderr]}")
      raise "Failed to install package in distrobox: #{install_output[:stderr]}"
    end
    log("Package #{package} installed in distrobox successfully")
    puts "Package #{package} installed in distrobox successfully."
    # Find .desktop files and export
    files_output = run_as_user(user, "distrobox enter #{container_name} -- dpkg -L #{package}")
    if files_output[:success]
      files = files_output[:stdout].lines.map(&.strip)
      desktop_files = files.select { |f| f.ends_with?(".desktop") && f.starts_with?("/usr/share/applications/") }
      app_names = desktop_files.map { |df| File.basename(df, ".desktop") }
      app_names.each do |app|
        export_output = run_as_user(user, "distrobox enter #{container_name} -- distrobox-export --app #{app}")
        if export_output[:success]
          log("Exported app: #{app}")
          puts "Exported app: #{app}"
        else
          log("Warning: Failed to export app #{app}: #{export_output[:stderr]}")
          puts "Warning: Failed to export app #{app}: #{export_output[:stderr]}"
        end
      end
      if app_names.empty?
        log("No .desktop files found for export")
        puts "No .desktop files found for export. If this is a GUI app, check the package."
      end
    else
      log("Warning: Failed to list package files for export: #{files_output[:stderr]}")
      puts "Warning: Failed to list package files for export: #{files_output[:stderr]}"
    end
  else
    binary = BINARY_MAP[package]? || package
    container_name = CONTAINER_NAME_PREFIX + "default"
    ensure_container_exists(container_name)
    # Check if already installed
    check_output = run_command(CONTAINER_TOOL, ["exec", container_name, "dpkg", "-s", package])
    if check_output[:success]
      log("Package #{package} already installed in container")
      puts "Package #{package} is already installed in the container."
      return
    end
    update_output = run_command(CONTAINER_TOOL, ["exec", container_name, "apt", "update"])
    unless update_output[:success]
      log("Failed to update in container: #{update_output[:stderr]}")
      raise "Failed to update in container: #{update_output[:stderr]}"
    end
    install_output = run_command(CONTAINER_TOOL, ["exec", container_name, "apt", "install", "-y", package])
    unless install_output[:success]
      log("Failed to install package in container: #{install_output[:stderr]}")
      raise "Failed to install package in container: #{install_output[:stderr]}"
    end
    log("Package #{package} installed in container successfully")
    puts "Package #{package} installed in container successfully."
    # Assume CLI, create wrapper in /usr/bin
    wrapper_path = "/usr/bin/#{binary}"
    wrapper_content = <<-WRAPPER
#!/bin/sh
sudo #{CONTAINER_TOOL} ps --filter name=^#{container_name}$ --filter status=running -q | grep -q . || sudo #{CONTAINER_TOOL} start #{container_name}
sudo #{CONTAINER_TOOL} exec #{container_name} #{binary} "$@"
WRAPPER
    File.write(wrapper_path, wrapper_content)
    File.chmod(wrapper_path, 0o755)
    log("Created CLI wrapper: #{wrapper_path}")
    puts "Created CLI wrapper: #{wrapper_path}"
    puts "To run manually: sudo #{CONTAINER_TOOL} exec -it #{container_name} #{binary}"
  end
end

def container_remove(package : String, gui : Bool)
  if gui
    user = ENV["SUDO_USER"]? || begin
      puts "This command with --gui must be run using sudo by a non-root user."
      log("Error: --gui requires sudo by non-root user")
      exit(1)
    end
    container_name = "hammer-gui"
    # Check if container exists
    check_container = run_as_user(user, "distrobox list | grep -q '#{container_name}'")
    if !check_container[:success]
      log("Distrobox container #{container_name} does not exist")
      puts "Package #{package} is not installed in the distrobox."
      return
    end
    # Check if installed
    check_output = run_as_user(user, "distrobox enter #{container_name} -- dpkg -s #{package}")
    unless check_output[:success]
      log("Package #{package} not installed in distrobox")
      puts "Package #{package} is not installed in the distrobox."
      return
    end
    # Get .desktop files before remove
    files_output = run_as_user(user, "distrobox enter #{container_name} -- dpkg -L #{package}")
    app_names = [] of String
    if files_output[:success]
      files = files_output[:stdout].lines.map(&.strip)
      desktop_files = files.select { |f| f.ends_with?(".desktop") && f.starts_with?("/usr/share/applications/") }
      app_names = desktop_files.map { |df| File.basename(df, ".desktop") }
    else
      log("Warning: Failed to list package files for unexport: #{files_output[:stderr]}")
      puts "Warning: Failed to list package files for unexport: #{files_output[:stderr]}"
    end
    # Unexport apps
    app_names.each do |app|
      unexport_output = run_as_user(user, "distrobox enter #{container_name} -- distrobox-export --app #{app} --delete")
      if unexport_output[:success]
        log("Unexported app: #{app}")
        puts "Unexported app: #{app}"
      else
        log("Warning: Failed to unexport app #{app}: #{unexport_output[:stderr]}")
        puts "Warning: Failed to unexport app #{app}: #{unexport_output[:stderr]}"
      end
    end
    # Remove package
    remove_output = run_as_user(user, "distrobox enter #{container_name} -- sudo apt remove -y #{package}")
    unless remove_output[:success]
      log("Failed to remove package from distrobox: #{remove_output[:stderr]}")
      raise "Failed to remove package from distrobox: #{remove_output[:stderr]}"
    end
    log("Package #{package} removed from distrobox successfully")
    puts "Package #{package} removed from distrobox successfully."
  else
    binary = BINARY_MAP[package]? || package
    container_name = CONTAINER_NAME_PREFIX + "default"
    ensure_container_exists(container_name)
    # Check if installed
    check_output = run_command(CONTAINER_TOOL, ["exec", container_name, "dpkg", "-s", package])
    unless check_output[:success]
      log("Package #{package} not installed in container")
      puts "Package #{package} is not installed in the container."
      return
    end
    # Remove package
    remove_output = run_command(CONTAINER_TOOL, ["exec", container_name, "apt", "remove", "-y", package])
    unless remove_output[:success]
      log("Failed to remove package from container: #{remove_output[:stderr]}")
      raise "Failed to remove package from container: #{remove_output[:stderr]}"
    end
    log("Package #{package} removed from container successfully")
    puts "Package #{package} removed from container successfully."
    # Remove CLI wrapper
    wrapper_path = "/usr/bin/#{binary}"
    if File.exists?(wrapper_path)
      File.delete(wrapper_path)
      log("Removed CLI wrapper: #{wrapper_path}")
      puts "Removed CLI wrapper: #{wrapper_path}"
    end
  end
end

def ensure_container_exists(container_name : String)
  log("Ensuring container #{container_name} exists")
  exists_output = run_command(CONTAINER_TOOL, ["container", "exists", container_name])
  newly_created = false
  if !exists_output[:success]
    create_output = run_command(CONTAINER_TOOL, ["run", "-d", "--name", container_name, CONTAINER_IMAGE, "sleep", "infinity"])
    unless create_output[:success]
      log("Failed to create container: #{create_output[:stderr]}")
      raise "Failed to create container: #{create_output[:stderr]}"
    end
    newly_created = true
    log("Created container: #{container_name}")
  end
  # Setup apt sources if newly created
  if newly_created
    sed_args = ["exec", container_name, "sed", "-i", "s/main$/main contrib non-free non-free-firmware/g", "/etc/apt/sources.list"]
    setup_output = run_command(CONTAINER_TOOL, sed_args)
    unless setup_output[:success]
      log("Warning: Failed to setup apt sources: #{setup_output[:stderr]}")
      puts "Warning: Failed to setup apt sources: #{setup_output[:stderr]}"
    end
    update_output = run_command(CONTAINER_TOOL, ["exec", container_name, "apt", "update"])
    unless update_output[:success]
      log("Failed to initial apt update in container: #{update_output[:stderr]}")
      raise "Failed to initial apt update in container: #{update_output[:stderr]}"
    end
    # Set up sudoers for podman commands
    sudoers_path = "/etc/sudoers.d/hammer-podman"
    sudoers_content = <<-SUDOERS
%sudo ALL=(ALL) NOPASSWD: /usr/bin/podman start #{container_name}, /usr/bin/podman exec #{container_name} *, /usr/bin/podman ps --filter name=^#{container_name}$ --filter status=running -q
SUDOERS
    File.write(sudoers_path, sudoers_content)
    File.chmod(sudoers_path, 0o440)
    log("Set up sudoers for #{container_name}")
  end
  # Check if running
  running_output = run_command(CONTAINER_TOOL, ["ps", "-q", "-f", "name=^#{container_name}$"])
  if running_output[:stdout].strip.empty?
    start_output = run_command(CONTAINER_TOOL, ["start", container_name])
    unless start_output[:success]
      log("Failed to start container: #{start_output[:stderr]}")
      raise "Failed to start container: #{start_output[:stderr]}"
    end
    log("Started container: #{container_name}")
  end
end

def clean_up
  log("Cleaning up unused resources")
  puts "Cleaning up unused resources..."
  output = run_command(CONTAINER_TOOL, ["system", "prune", "-f"])
  if output[:success]
    log("Clean up completed: #{output[:stdout]}")
    puts "Clean up completed."
  else
    log("Failed to clean up: #{output[:stderr]}")
    STDERR.puts "Failed to clean up: #{output[:stderr]}"
  end
end

def refresh
  log("Refreshing container metadata")
  puts "Refreshing container metadata..."
  container_name = CONTAINER_NAME_PREFIX + "default"
  ensure_container_exists(container_name)
  output = run_command(CONTAINER_TOOL, ["exec", container_name, "apt", "update"])
  if output[:success]
    log("Refresh completed")
    puts "Refresh completed."
  else
    log("Failed to refresh: #{output[:stderr]}")
    raise "Failed to refresh: #{output[:stderr]}"
  end
end

if ARGV.empty?
  puts "No subcommand was used"
else
  subcommand = ARGV.shift
  begin
    case subcommand
    when "install"
      matches = parse_install_remove(ARGV)
      install_package(matches[:package], matches[:gui])
    when "remove"
      matches = parse_install_remove(ARGV)
      remove_package(matches[:package], matches[:gui])
    when "refresh"
      refresh
    when "clean"
      clean_up
    else
      puts "Unknown subcommand: #{subcommand}"
    end
  rescue ex : Exception
    log("Error: #{ex.message}")
    STDERR.puts "Error: #{ex.message}"
    exit(1)
  end
end
