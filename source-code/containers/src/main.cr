require "option_parser"
require "file_utils"
require "process"

if LibC.getuid != 0
  puts "This tool must be run as root."
  exit(1)
end

COLOR_RESET = "\033[0m"
COLOR_RED = "\033[31m"
COLOR_GREEN = "\033[32m"
COLOR_YELLOW = "\033[33m"
COLOR_BLUE = "\033[34m"
COLOR_BOLD = "\033[1m"

HAMMER_PATH = "/usr/lib/HackerOS/hammer/bin"
LOG_DIR = "/usr/lib/HackerOS/hammer/logs"
CONTAINER_TOOL = "podman"
CONTAINER_NAME_PREFIX = "hammer-container-"
CONTAINER_IMAGE = "debian:stable"

BINARY_MAP = {
  "golang" => "go",
}

def log_message(message : String)
  Dir.mkdir_p(LOG_DIR)
  File.open("#{LOG_DIR}/hammer-container.log", "a") do |f|
    f.puts "[#{Time.local}] #{message}"
  end
end

def run_command(cmd : String, args : Array(String)) : {success: Bool, stdout: String, stderr: String}
  stdout = IO::Memory.new
  stderr = IO::Memory.new
  status = Process.run(cmd, args: args, output: stdout, error: stderr)
  {success: status.success?, stdout: stdout.to_s, stderr: stderr.to_s}
end

def start_progress(total_steps : Int32) : Process
  progress_bin = "#{HAMMER_PATH}/hammer-progress-bar"
  process = Process.new(progress_bin, input: Process::Redirect::Pipe, output: Process::Redirect::Pipe, error: Process::Redirect::Pipe)
  sleep(0.1.seconds)
  if !process.exists?
    raise "Failed to start hammer-progress-bar"
  end
  process.input.puts "set_total #{total_steps}"
  process.input.flush
  process
end

def update_progress(process : Process, msg : String)
  process.input.puts "msg #{msg}"
  process.input.puts "update"
  process.input.flush
end

def finish_progress(process : Process, msg : String = "done")
  process.input.puts "msg #{msg}"
  process.input.puts "done"
  process.input.flush
  process.wait
end

def ensure_container_exists(container_name : String)
  exists_output = run_command(CONTAINER_TOOL, ["container", "exists", container_name])
  newly_created = false
  if !exists_output[:success]
    create_output = run_command(CONTAINER_TOOL, ["run", "-d", "--name", container_name, CONTAINER_IMAGE, "sleep", "infinity"])
    if !create_output[:success]
      log_message("Failed to create container: #{create_output[:stderr]}")
      raise "Failed to create container: #{create_output[:stderr]}"
    end
    newly_created = true
  end
  if newly_created
    sed_args = ["exec", container_name, "sed", "-i", "s/main$/main contrib non-free non-free-firmware/g", "/etc/apt/sources.list"]
    setup_output = run_command(CONTAINER_TOOL, sed_args)
    unless setup_output[:success]
      log_message("Warning: Failed to setup apt sources: #{setup_output[:stderr]}")
      puts "#{COLOR_YELLOW}Warning: Failed to setup apt sources: #{setup_output[:stderr]}#{COLOR_RESET}"
    end
    update_output = run_command(CONTAINER_TOOL, ["exec", container_name, "apt", "update"])
    unless update_output[:success]
      log_message("Failed to initial apt update in container: #{update_output[:stderr]}")
      raise "Failed to initial apt update in container: #{update_output[:stderr]}"
    end
    sudoers_path = "/etc/sudoers.d/hammer-podman"
    sudoers_content = "%sudo ALL=(ALL) NOPASSWD: /usr/bin/podman start #{container_name}, /usr/bin/podman exec #{container_name} *, /usr/bin/podman ps --filter name=^#{container_name}$ --filter status=running -q\n"
    File.write(sudoers_path, sudoers_content)
    File.chmod(sudoers_path, 0o440)
  end
  running_output = run_command(CONTAINER_TOOL, ["ps", "-q", "-f", "name=^#{container_name}$"])
  if running_output[:stdout].strip.empty?
    start_output = run_command(CONTAINER_TOOL, ["start", container_name])
    if !start_output[:success]
      log_message("Failed to start container: #{start_output[:stderr]}")
      raise "Failed to start container: #{start_output[:stderr]}"
    end
  end
end

def container_install(package : String)
  progress : Process? = nil
  time_start = Time.monotonic
  begin
    log_message("Starting container install of #{package}")
    puts "#{COLOR_BLUE}Installing #{package} in container...#{COLOR_RESET}"
    progress = start_progress(5)
    binary = BINARY_MAP[package]? || package
    container_name = CONTAINER_NAME_PREFIX + "default"
    update_progress(progress, "Ensuring container exists")
    ensure_container_exists(container_name)
    update_progress(progress, "Checking if installed")
    check_output = run_command(CONTAINER_TOOL, ["exec", container_name, "dpkg", "-s", package])
    if check_output[:success]
      puts "#{COLOR_YELLOW}Package #{package} is already installed in the container.#{COLOR_RESET}"
      finish_progress(progress, "Already installed")
      return
    end
    update_progress(progress, "Updating apt")
    update_output = run_command(CONTAINER_TOOL, ["exec", container_name, "apt", "update"])
    if !update_output[:success]
      log_message("Failed to update in container: #{update_output[:stderr]}")
      raise "Failed to update in container: #{update_output[:stderr]}"
    end
    update_progress(progress, "Installing package")
    install_output = run_command(CONTAINER_TOOL, ["exec", container_name, "apt", "install", "-y", package])
    if !install_output[:success]
      log_message("Failed to install package in container: #{install_output[:stderr]}")
      raise "Failed to install package in container: #{install_output[:stderr]}"
    end
    puts "#{COLOR_GREEN}Package #{package} installed in container successfully.#{COLOR_RESET}"
    update_progress(progress, "Creating wrapper")
    wrapper_path = "/usr/bin/#{binary}"
    wrapper_content = <<-WRAPPER
#!/bin/sh
sudo #{CONTAINER_TOOL} ps --filter name=^#{container_name}$ --filter status=running -q | grep -q . || sudo #{CONTAINER_TOOL} start #{container_name}
sudo #{CONTAINER_TOOL} exec #{container_name} #{binary} "$@"
WRAPPER
    File.write(wrapper_path, wrapper_content)
    File.chmod(wrapper_path, 0o755)
    puts "Created CLI wrapper: #{wrapper_path}"
    puts "To run manually: sudo #{CONTAINER_TOOL} exec -it #{container_name} #{binary}"
    finish_progress(progress, "Completed")
  rescue ex : Exception
    log_message("Error in container install: #{ex.message}")
    puts "#{COLOR_RED}Error: #{ex.message}#{COLOR_RESET}"
    if progress
      finish_progress(progress, "Error: #{ex.message}")
    end
    exit(1)
  ensure
    duration = Time.monotonic - time_start
    log_message("Container install completed, duration: #{duration.total_seconds} seconds")
  end
end

def container_remove(package : String)
  progress : Process? = nil
  time_start = Time.monotonic
  begin
    log_message("Starting container remove of #{package}")
    puts "#{COLOR_BLUE}Removing #{package} from container...#{COLOR_RESET}"
    progress = start_progress(4)
    binary = BINARY_MAP[package]? || package
    container_name = CONTAINER_NAME_PREFIX + "default"
    update_progress(progress, "Ensuring container exists")
    ensure_container_exists(container_name)
    update_progress(progress, "Checking if installed")
    check_output = run_command(CONTAINER_TOOL, ["exec", container_name, "dpkg", "-s", package])
    unless check_output[:success]
      puts "#{COLOR_YELLOW}Package #{package} is not installed in the container.#{COLOR_RESET}"
      finish_progress(progress, "Not installed")
      return
    end
    update_progress(progress, "Removing package")
    remove_output = run_command(CONTAINER_TOOL, ["exec", container_name, "apt", "remove", "-y", package])
    if !remove_output[:success]
      log_message("Failed to remove package from container: #{remove_output[:stderr]}")
      raise "Failed to remove package from container: #{remove_output[:stderr]}"
    end
    puts "#{COLOR_GREEN}Package #{package} removed from container successfully.#{COLOR_RESET}"
    update_progress(progress, "Removing wrapper")
    wrapper_path = "/usr/bin/#{binary}"
    File.delete(wrapper_path) if File.exists?(wrapper_path)
    puts "Removed CLI wrapper: #{wrapper_path}"
    finish_progress(progress, "Completed")
  rescue ex : Exception
    log_message("Error in container remove: #{ex.message}")
    puts "#{COLOR_RED}Error: #{ex.message}#{COLOR_RESET}"
    if progress
      finish_progress(progress, "Error: #{ex.message}")
    end
    exit(1)
  ensure
    duration = Time.monotonic - time_start
    log_message("Container remove completed, duration: #{duration.total_seconds} seconds")
  end
end

if ARGV.empty?
  puts "Usage: hammer-container <install|remove> <package>"
  exit(1)
end

subcommand = ARGV.shift
package = ARGV.shift || ""
if package.empty?
  puts "Package name required."
  exit(1)
end

case subcommand
when "install"
  container_install(package)
when "remove"
  container_remove(package)
else
  puts "Unknown subcommand: #{subcommand}"
  exit(1)
end
