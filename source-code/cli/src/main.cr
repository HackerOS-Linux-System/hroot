require "option_parser"

module Hammer
  VERSION = "0.2"
  HAMMER_PATH = "#{ENV["HOME"]? || "/home/user"}/.hackeros/hammer"

  def self.main
    command = ARGV.shift? || ""
    args = ARGV.dup

    case command
    when "install"
      if args.size != 1
        puts "Usage: hammer install <package>"
        exit(1)
      end
      run_core("install", args)
    when "remove"
      if args.size != 1
        puts "Usage: hammer remove <package>"
        exit(1)
      end
      run_core("remove", args)
    when "update"
      if args.size != 0
        puts "Usage: hammer update"
        exit(1)
      end
      run_updater("update", args)
    when "clean"
      if args.size != 0
        puts "Usage: hammer clean"
        exit(1)
      end
      run_core("clean", args)
    when "refresh"
      if args.size != 0
        puts "Usage: hammer refresh"
        exit(1)
      end
      run_core("refresh", args)
    when "build"
      if args.size != 0
        puts "Usage: hammer build"
        exit(1)
      end
      run_builder("build", args)
    when "back"
      if args.size != 0
        puts "Usage: hammer back"
        exit(1)
      end
      run_core("back", args)
    when "snapshot"
      if args.size != 0
        puts "Usage: hammer snapshot"
        exit(1)
      end
      run_core("snapshot", args)
    when "build-init", "build init"
      if args.size != 0
        puts "Usage: hammer build init"
        exit(1)
      end
      run_builder("init", args)
    when "about"
      if args.size != 0
        puts "Usage: hammer about"
        exit(1)
      end
      about
    else
      usage
      exit(1)
    end
  end

  private def self.run_core(subcommand : String, args : Array(String))
    binary = "#{HAMMER_PATH}/hammer-core"
    Process.run(binary, [subcommand] + args, output: Process::Redirect::Inherit, error: Process::Redirect::Inherit)
  end

  private def self.run_updater(subcommand : String, args : Array(String))
    binary = "#{HAMMER_PATH}/hammer-updater"
    Process.run(binary, [subcommand] + args, output: Process::Redirect::Inherit, error: Process::Redirect::Inherit)
  end

  private def self.run_builder(subcommand : String, args : Array(String))
    binary = "#{HAMMER_PATH}/hammer-builder"
    Process.run(binary, [subcommand] + args, output: Process::Redirect::Inherit, error: Process::Redirect::Inherit)
  end

  private def self.about
    puts "Hammer CLI Tool for HackerOS Atomic"
    puts "Version: #{VERSION}"
    puts "Description: Tool for managing atomic installations, updates, and builds inspired by apx and rpm-ostree."
    puts "Components:"
    puts "- hammer-core: Core operations in Rust"
    puts "- hammer-updater: System updater in Crystal"
    puts "- hammer-builder: ISO builder in Go"
    puts "Location: #{HAMMER_PATH}"
  end

  private def self.usage
    puts "Usage: hammer <command> [options]"
    puts ""
    puts "Commands:"
    puts "  install <package>    Install a package in container"
    puts "  remove <package>     Remove a package from container"
    puts "  update               Update the system (using snapshot)"
    puts "  clean                Clean up unused resources"
    puts "  refresh              Refresh repositories"
    puts "  build                Build atomic ISO (must be in project dir)"
    puts "  back                 Rollback to previous system version"
    puts "  snapshot             Force create a snapshot"
    puts "  build init           Initialize build project"
    puts "  about                Show tool information"
  end
end

Hammer.main
