require "option_parser"

module Hammer
  VERSION = "0.3" # Updated version for expansions
  HAMMER_PATH = "#{ENV["HOME"]? || "/home/user"}/.hackeros/hammer"

  def self.main
    return usage if ARGV.empty?
    command = ARGV.shift
    case command
    when "install"
      install_command(ARGV)
    when "remove"
      remove_command(ARGV)
    when "update"
      update_command(ARGV)
    when "clean"
      clean_command(ARGV)
    when "refresh"
      refresh_command(ARGV)
    when "build"
      build_command(ARGV)
    when "switch"
      switch_command(ARGV)
    when "deploy"
      deploy_command(ARGV)
    when "build-init", "build init"
      build_init_command(ARGV)
    when "about"
      about_command(ARGV)
    else
      usage
      exit(1)
    end
  end

  private def self.install_command(args : Array(String))
    parser = OptionParser.new do |parser|
      parser.banner = "Usage: hammer install [options] <package>"
      parser.on("--atomic", "Install atomically in the system") { }
      parser.unknown_args do |unknown_args|
        if unknown_args.size != 1
          puts parser
          exit(1)
        end
      end
    end
    parser.parse(args.dup)
    package = args[-1]? || ""
    atomic_flag = args.includes?("--atomic") ? ["--atomic"] : [] of String
    if package.empty?
      puts parser
      exit(1)
    end
    run_core("install", atomic_flag + [package])
  end

  private def self.remove_command(args : Array(String))
    parser = OptionParser.new do |parser|
      parser.banner = "Usage: hammer remove [options] <package>"
      parser.on("--atomic", "Remove atomically from the system") { }
      parser.unknown_args do |unknown_args|
        if unknown_args.size != 1
          puts parser
          exit(1)
        end
      end
    end
    parser.parse(args.dup)
    package = args[-1]? || ""
    atomic_flag = args.includes?("--atomic") ? ["--atomic"] : [] of String
    if package.empty?
      puts parser
      exit(1)
    end
    run_core("remove", atomic_flag + [package])
  end

  private def self.update_command(args : Array(String))
    if args.size != 0
      puts "Usage: hammer update"
      exit(1)
    end
    run_updater("update", args)
  end

  private def self.clean_command(args : Array(String))
    if args.size != 0
      puts "Usage: hammer clean"
      exit(1)
    end
    run_core("clean", args)
  end

  private def self.refresh_command(args : Array(String))
    if args.size != 0
      puts "Usage: hammer refresh"
      exit(1)
    end
    run_core("refresh", args)
  end

  private def self.build_command(args : Array(String))
    if args.size != 0
      puts "Usage: hammer build"
      exit(1)
    end
    run_builder("build", args)
  end

  private def self.switch_command(args : Array(String))
    parser = OptionParser.new do |parser|
      parser.banner = "Usage: hammer switch [deployment]"
      parser.unknown_args do |unknown_args|
        if unknown_args.size > 1
          puts parser
          exit(1)
        end
      end
    end
    parser.parse(args.dup)
    deployment = args[0]? || ""
    run_args = deployment.empty? ? [] of String : [deployment]
    run_core("switch", run_args)
  end

  private def self.deploy_command(args : Array(String))
    if args.size != 0
      puts "Usage: hammer deploy"
      exit(1)
    end
    run_core("deploy", args)
  end

  private def self.build_init_command(args : Array(String))
    if args.size != 0
      puts "Usage: hammer build init"
      exit(1)
    end
    run_builder("init", args)
  end

  private def self.about_command(args : Array(String))
    if args.size != 0
      puts "Usage: hammer about"
      exit(1)
    end
    about
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
    puts " install [--atomic] <package> Install a package (optionally atomically)"
    puts " remove [--atomic] <package> Remove a package (optionally atomically)"
    puts " update Update the system atomically"
    puts " clean Clean up unused resources"
    puts " refresh Refresh repositories"
    puts " build Build atomic ISO (must be in project dir)"
    puts " switch [deployment] Switch to a deployment (rollback if no arg)"
    puts " deploy Create a new deployment"
    puts " build init Initialize build project"
    puts " about Show tool information"
  end
end

Hammer.main
