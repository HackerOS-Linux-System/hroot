require "option_parser"
require "file_utils"

if LibC.getuid != 0
  puts "This tool must be run as root."
  exit(1)
end

CONTAINER_IMAGE = "debian:stable"
BINARY_MAP = {
  "golang" => "go",
}

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

def parse_package(args : Array(String)) : String
  package = ""
  parser = OptionParser.new do |p|
    p.banner = "Usage: [subcommand] package"
    p.unknown_args do |uargs|
      package = uargs[0] if uargs.size > 0
    end
  end
  parser.parse(args)
  if package.empty?
    STDERR.puts "Package name required."
    exit(1)
  end
  package
end

def container_install(package : String)
  user = ENV["SUDO_USER"]? || begin
    puts "This command must be run using sudo by a non-root user."
    exit(1)
  end
  container_name = "hammer-container"
  # Ensure distrobox exists
  check_container = run_as_user(user, "distrobox list | grep -q '#{container_name}'")
  if !check_container[:success]
    create_output = run_as_user(user, "distrobox create --name #{container_name} --image #{CONTAINER_IMAGE} --yes")
    raise "Failed to create distrobox container: #{create_output[:stderr]}" unless create_output[:success]
    puts "Created distrobox container: #{container_name}"
  end
  # Check if already installed
  check_output = run_as_user(user, "distrobox enter #{container_name} -- dpkg -s #{package}")
  if check_output[:success]
    puts "Package #{package} is already installed in the distrobox."
    return
  end
  # Setup sources and architecture
  sources_setup_cmd = "sudo bash -c 'if [ ! -f /etc/apt/sources.list ]; then echo \\\"deb http://deb.debian.org/debian stable main contrib non-free non-free-firmware\\\" > /etc/apt/sources.list; else sed -i \\\"s/main$/main contrib non-free non-free-firmware/g\\\" /etc/apt/sources.list; fi'"
  sources_setup = run_as_user(user, "distrobox enter #{container_name} -- #{sources_setup_cmd}")
  if !sources_setup[:success]
    puts "Warning: Failed to setup sources: #{sources_setup[:stderr]}"
  end
  arch_setup = run_as_user(user, "distrobox enter #{container_name} -- sudo dpkg --add-architecture i386")
  if !arch_setup[:success]
    puts "Warning: Failed to add i386 architecture: #{arch_setup[:stderr]}"
  end
  # Update
  update_output = run_as_user(user, "distrobox enter #{container_name} -- sudo apt update")
  raise "Failed to update in distrobox: #{update_output[:stderr]}" unless update_output[:success]
  # Install
  install_output = run_as_user(user, "distrobox enter #{container_name} -- sudo apt install -y #{package}")
  raise "Failed to install package in distrobox: #{install_output[:stderr]}" unless install_output[:success]
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
        puts "Exported app: #{app}"
      else
        puts "Warning: Failed to export app #{app}: #{export_output[:stderr]}"
      end
    end
    if app_names.empty?
      binary = BINARY_MAP[package]? || package
      export_output = run_as_user(user, "distrobox enter #{container_name} -- distrobox-export --bin /usr/bin/#{binary}")
      if export_output[:success]
        puts "Exported binary: #{binary}"
      else
        puts "Warning: Failed to export binary #{binary}: #{export_output[:stderr]}"
      end
    end
  else
    puts "Warning: Failed to list package files for export: #{files_output[:stderr]}"
  end
end

def container_remove(package : String)
  user = ENV["SUDO_USER"]? || begin
    puts "This command must be run using sudo by a non-root user."
    exit(1)
  end
  container_name = "hammer-container"
  # Check if container exists
  check_container = run_as_user(user, "distrobox list | grep -q '#{container_name}'")
  if !check_container[:success]
    puts "Package #{package} is not installed in the distrobox."
    return
  end
  # Check if installed
  check_output = run_as_user(user, "distrobox enter #{container_name} -- dpkg -s #{package}")
  unless check_output[:success]
    puts "Package #{package} is not installed in the distrobox."
    return
  end
  # Get .desktop files or binary for unexport
  files_output = run_as_user(user, "distrobox enter #{container_name} -- dpkg -L #{package}")
  app_names = [] of String
  if files_output[:success]
    files = files_output[:stdout].lines.map(&.strip)
    desktop_files = files.select { |f| f.ends_with?(".desktop") && f.starts_with?("/usr/share/applications/") }
    app_names = desktop_files.map { |df| File.basename(df, ".desktop") }
  else
    puts "Warning: Failed to list package files for unexport: #{files_output[:stderr]}"
  end
  # Unexport
  if !app_names.empty?
    app_names.each do |app|
      unexport_output = run_as_user(user, "distrobox enter #{container_name} -- distrobox-export --app #{app} --delete")
      if unexport_output[:success]
        puts "Unexported app: #{app}"
      else
        puts "Warning: Failed to unexport app #{app}: #{unexport_output[:stderr]}"
      end
    end
  else
    binary = BINARY_MAP[package]? || package
    unexport_output = run_as_user(user, "distrobox enter #{container_name} -- distrobox-export --bin /usr/bin/#{binary} --delete")
    if unexport_output[:success]
      puts "Unexported binary: #{binary}"
    else
      puts "Warning: Failed to unexport binary #{binary}: #{unexport_output[:stderr]}"
    end
  end
  # Remove package
  remove_output = run_as_user(user, "distrobox enter #{container_name} -- sudo apt remove -y #{package}")
  raise "Failed to remove package from distrobox: #{remove_output[:stderr]}" unless remove_output[:success]
  puts "Package #{package} removed from distrobox successfully."
end

if ARGV.empty?
  puts "No subcommand was used"
else
  subcommand = ARGV.shift
  begin
    case subcommand
    when "install"
      package = parse_package(ARGV)
      container_install(package)
    when "remove"
      package = parse_package(ARGV)
      container_remove(package)
    else
      puts "Unknown subcommand: #{subcommand}"
    end
  rescue ex : Exception
    STDERR.puts "Error: #{ex.message}"
    exit(1)
  end
end
