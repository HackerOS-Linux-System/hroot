require "option_parser"
require "file_utils"
require "time"

CONTAINER_TOOL = "podman"
CONTAINER_NAME_PREFIX = "hammer-container-"
CONTAINER_IMAGE = "debian:stable"
BTRFS_TOP = "/btrfs-root"
DEPLOYMENTS_DIR = "/btrfs-root/deployments"
CURRENT_SYMLINK = "/btrfs-root/current"

def run_command(cmd : String, args : Array(String)) : {success: Bool, stdout: String, stderr: String}
stdout = IO::Memory.new
stderr = IO::Memory.new
status = Process.run(cmd, args: args, output: stdout, error: stderr)
{success: status.success?, stdout: stdout.to_s, stderr: stderr.to_s}
end

def parse_install_remove(args : Array(String)) : {package: String, atomic: Bool}
atomic = false
package = ""
parser = OptionParser.new do |p|
p.banner = "Usage: [subcommand] [options] package"
p.on("--atomic", "Atomic operation") { atomic = true }
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
    {package: package, atomic: atomic}
    end

    def parse_switch(args : Array(String)) : String?
    deployment = nil
    parser = OptionParser.new do |p|
    p.unknown_args do |uargs|
    deployment = uargs[0] if uargs.size > 0
    end
    end
    parser.parse(args)
    deployment
    end

    def install_package(package : String, atomic : Bool)
    puts "Installing package: #{package} (atomic: #{atomic})"
    if atomic
        atomic_install(package)
        else
            container_install(package)
            end
            end

            def remove_package(package : String, atomic : Bool)
            puts "Removing package: #{package} (atomic: #{atomic})"
            if atomic
                atomic_remove(package)
                else
                    container_remove(package)
                    end
                    end

                    def container_install(package : String)
                    container_name = CONTAINER_NAME_PREFIX + "default"
                    ensure_container_exists(container_name)
                    update_output = run_command(CONTAINER_TOOL, ["exec", "-it", container_name, "apt", "update", "-y"])
                    raise "Failed to update in container: #{update_output[:stderr]}" unless update_output[:success]
                    install_output = run_command(CONTAINER_TOOL, ["exec", "-it", container_name, "apt", "install", "-y", package])
                    raise "Failed to install package in container: #{install_output[:stderr]}" unless install_output[:success]
                    export_binaries_from_container(container_name, package)
                    puts "Package #{package} installed in container successfully."
                    end

                    def container_remove(package : String)
                    container_name = CONTAINER_NAME_PREFIX + "default"
                    ensure_container_exists(container_name)
                    output = run_command(CONTAINER_TOOL, ["exec", "-it", container_name, "apt", "remove", "-y", package])
                    raise "Failed to remove package from container: #{output[:stderr]}" unless output[:success]
                    puts "Package #{package} removed from container successfully."
                    end

                    def atomic_install(package : String)
                    puts "Performing atomic install of #{package}..."
                    new_deployment = create_deployment(true)
                    bind_mounts_for_chroot(new_deployment, true)
                    chroot_cmd = "chroot #{new_deployment} /bin/bash -c 'apt update && apt install -y #{package} && apt autoremove -y'"
                    output = run_command("/bin/bash", ["-c", chroot_cmd])
                    if !output[:success]
                        bind_mounts_for_chroot(new_deployment, false)
                        raise "Failed to install in chroot: #{output[:stderr]}"
                        end
                        bind_mounts_for_chroot(new_deployment, false)
                        set_subvolume_readonly(new_deployment, true)
                        switch_to_deployment(new_deployment)
                            puts "Atomic install completed. Reboot to apply."
                            end

                            def atomic_remove(package : String)
                            puts "Performing atomic remove of #{package}..."
                            new_deployment = create_deployment(true)
                            bind_mounts_for_chroot(new_deployment, true)
                            chroot_cmd = "chroot #{new_deployment} /bin/bash -c 'apt remove -y #{package} && apt autoremove -y'"
                            output = run_command("/bin/bash", ["-c", chroot_cmd])
                            if !output[:success]
                                bind_mounts_for_chroot(new_deployment, false)
                                raise "Failed to remove in chroot: #{output[:stderr]}"
                                end
                                bind_mounts_for_chroot(new_deployment, false)
                                set_subvolume_readonly(new_deployment, true)
                                switch_to_deployment(new_deployment)
                                    puts "Atomic remove completed. Reboot to apply."
                                    end

                                    def create_deployment(writable : Bool) : String
                                    puts "Creating new deployment..."
                                    Dir.mkdir_p(DEPLOYMENTS_DIR)
                                    current = File.readlink(CURRENT_SYMLINK)
                                    timestamp = Time.local.to_s("%Y-%m-%d")
                                    new_deployment = "#{DEPLOYMENTS_DIR}/hammer-#{timestamp}"
                                    args = ["subvolume", "snapshot"]
                                    args << "-r" unless writable
                                    args << current
                                    args << new_deployment
                                    output = run_command("btrfs", args)
                                    raise "Failed to create deployment: #{output[:stderr]}" unless output[:success]
                                    set_subvolume_readonly(new_deployment, false) if writable
                                    puts "Deployment created at: #{new_deployment}"
                                    new_deployment
                                    end

                                    def switch_deployment(deployment : String?)
                                    puts "Switching deployment..."
                                    target = if deployment
                                    "#{DEPLOYMENTS_DIR}/#{deployment}"
                                    else
                                        deployments = get_deployments
                                        raise "Not enough deployments for rollback." if deployments.size < 2
                                        deployments.sort[deployments.size - 2]
                                        end
                                        raise "Deployment #{target} does not exist." unless File.exists?(target)
                                        switch_to_deployment(target)
                                            puts "Switched to deployment: #{target}. Reboot to apply."
                                            end

                                            def switch_to_deployment(deployment : String)
                                            id = get_subvol_id(deployment)
                                            output = run_command("btrfs", ["subvolume", "set-default", id, "/"])
                                            raise "Failed to set default subvolume: #{output[:stderr]}" unless output[:success]
                                            File.delete(CURRENT_SYMLINK) if File.exists?(CURRENT_SYMLINK)
                                            File.symlink(deployment, CURRENT_SYMLINK)
                                            end

                                            def clean_up
                                            puts "Cleaning up unused resources..."
                                            run_command(CONTAINER_TOOL, ["system", "prune", "-f"])
                                            deployments = get_deployments.sort
                                            if deployments.size > 5
                                                deployments[0...(deployments.size - 5)].each do |dep|
                                                output = run_command("btrfs", ["subvolume", "delete", dep])
                                                STDERR.puts "Failed to delete deployment #{dep}: #{output[:stderr]}" unless output[:success]
                                                end
                                                end
                                                puts "Clean up completed."
                                                end

                                                def refresh
                                                puts "Refreshing container metadata..."
                                                container_name = CONTAINER_NAME_PREFIX + "default"
                                                ensure_container_exists(container_name)
                                                output = run_command(CONTAINER_TOOL, ["exec", "-it", container_name, "apt", "update", "-y"])
                                                raise "Failed to refresh: #{output[:stderr]}" unless output[:success]
                                                puts "Refresh completed."
                                                end

                                                def ensure_container_exists(container_name : String)
                                                output = run_command(CONTAINER_TOOL, ["ps", "-a", "-f", "name=#{container_name}"])
                                                if output[:stdout].empty?
                                                    create_output = run_command(CONTAINER_TOOL, ["run", "-d", "--name", container_name, CONTAINER_IMAGE, "sleep", "infinity"])
                                                    raise "Failed to create container: #{create_output[:stderr]}" unless create_output[:success]
                                                    end
                                                    end

                                                    def export_binaries_from_container(container_name : String, package : String)
                                                    host_bin_dir = "/home/user/.local/bin"
                                                    Dir.mkdir_p(host_bin_dir)
                                                    bin_path = "/usr/bin/#{package}"
                                                    run_command(CONTAINER_TOOL, ["cp", "#{container_name}:#{bin_path}", host_bin_dir])
                                                    end

                                                    def get_deployments : Array(String)
                                                    Dir.entries(DEPLOYMENTS_DIR).select(&.starts_with?("hammer-")).map { |f| File.join(DEPLOYMENTS_DIR, f) }
                                                    rescue ex : Exception
                                                    raise "Failed to list deployments: #{ex.message}"
                                                    end

                                                    def get_subvol_id(path : String) : String
                                                    output = run_command("btrfs", ["subvolume", "show", path])
                                                    raise "Failed to get subvolume ID." unless output[:success]
                                                    output[:stdout].lines.each do |line|
                                                    if line.includes?("Subvolume ID:")
                                                        parts = line.split(":")
                                                        return parts[1].strip if parts.size > 1
                                                        end
                                                        end
                                                        raise "Subvolume ID not found."
                                                        end

                                                        def set_subvolume_readonly(path : String, readonly : Bool)
                                                        value = readonly ? "true" : "false"
                                                        output = run_command("btrfs", ["property", "set", "-ts", path, "ro", value])
                                                        raise "Failed to set readonly #{value}: #{output[:stderr]}" unless output[:success]
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
                                                                raise "Failed to #{mount ? "mount" : "umount"} #{dir}: #{output[:stderr]}" unless output[:success]
                                                                end
                                                                end

                                                                if ARGV.empty?
                                                                    puts "No subcommand was used"
                                                                    else
                                                                        subcommand = ARGV.shift
                                                                        case subcommand
                                                                        when "install"
                                                                        matches = parse_install_remove(ARGV)
                                                                        install_package(matches[:package], matches[:atomic])
                                                                        when "remove"
                                                                        matches = parse_install_remove(ARGV)
                                                                        remove_package(matches[:package], matches[:atomic])
                                                                        when "deploy"
                                                                        create_deployment(false)
                                                                        when "switch"
                                                                        deployment = parse_switch(ARGV)
                                                                        switch_deployment(deployment)
                                                                            when "clean"
                                                                            clean_up
                                                                            when "refresh"
                                                                            refresh
                                                                            else
                                                                                puts "No subcommand was used"
                                                                                end
                                                                                end
