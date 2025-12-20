package main

import (
	"flag"
	"fmt"
	"os"
	"os/exec"
	"path/filepath"
	"strings"
)

const (
	defaultSuite = "trixie" // Default to testing, adjust as needed
)

func main() {
	if len(os.Args) < 2 {
		usage()
		os.Exit(1)
	}

	subcommand := os.Args[1]
	args := os.Args[2:]

	switch subcommand {
		case "init":
			initProject(args)
		case "build":
			buildISO(args)
		default:
			usage()
			os.Exit(1)
	}
}

func initProject(args []string) {
	fs := flag.NewFlagSet("init", flag.ExitOnError)
	suite := fs.String("suite", defaultSuite, "Debian suite: stable, testing, sid, or codename")
	atomic := fs.Bool("atomic", true, "Enable atomic features (BTRFS, deployments)")
	fs.Parse(args)

	// Map common names to codenames
	actualSuite := *suite
	switch *suite {
		case "stable":
			actualSuite = "bookworm" // Update to current stable
		case "testing":
			actualSuite = "trixie"
		case "sid":
			actualSuite = "sid"
	}

	fmt.Printf("Initializing live-build project with suite: %s (atomic: %v)\n", actualSuite, *atomic)

	// Check if config exists
	if _, err := os.Stat("config"); err == nil {
		fmt.Println("Project already initialized.")
		os.Exit(1)
	}

	// Run lb config with more options for installer
	cmd := exec.Command("lb", "config",
			    "--distribution", actualSuite,
		     "--architectures", "amd64",
		     "--bootappend-live", "boot=live components username=hacker",
		     "--debian-installer", "live", // Enable installer
	)
	cmd.Stdout = os.Stdout
	cmd.Stderr = os.Stderr
	if err := cmd.Run(); err != nil {
		fmt.Printf("Failed to initialize: %v\n", err)
		os.Exit(1)
	}

	// Create package lists
	pkgListsDir := filepath.Join("config", "package-lists")
	if err := os.MkdirAll(pkgListsDir, 0755); err != nil {
		fmt.Printf("Failed to create package-lists dir: %v\n", err)
		os.Exit(1)
	}

	// Base packages for atomic system
	atomicPkgs := []string{
		"btrfs-progs",
		"podman",
		"distrobox", // For container management
		"grub-efi-amd64", // For booting
		"calamares", // Installer, assuming we use Calamares for custom installation
		"rsync",
		"curl",
		"wget",
		"git",
		// Add more as needed
	}
	pkgContent := strings.Join(atomicPkgs, "\n") + "\n"
	pkgFile := filepath.Join(pkgListsDir, "atomic.list.chroot")
	if err := os.WriteFile(pkgFile, []byte(pkgContent), 0644); err != nil {
		fmt.Printf("Failed to write package list: %v\n", err)
		os.Exit(1)
	}

	// Create hooks dir
	hooksDir := filepath.Join("config", "includes.chroot_after_packages")
	if err := os.MkdirAll(hooksDir, 0755); err != nil {
		fmt.Printf("Failed to create hooks dir: %v\n", err)
		os.Exit(1)
	}

	// Hook for BTRFS and atomic setup
	hookFile := filepath.Join(hooksDir, "0100-setup-atomic.hook.chroot")
	hookContent := `#!/bin/sh
	set -e

	echo "Setting up atomic features..."

	# Install additional tools if needed (though in package list)

	# Configure podman for rootless
	echo "Configuring podman..."
	podman system migrate || true

	# Set up directories for deployments
	mkdir -p /btrfs-root/deployments

	# Placeholder for hammer installation
	# Assume hammer binaries are copied via includes.binary or something
	# For now, echo setup
	echo "Hammer tools will be installed in /usr/local/bin/hammer"

	# Configure fstab template or installer scripts
	# Since installer will handle BTRFS setup, add Calamares config if using Calamares

	# Install Calamares settings if present
	if [ -d /usr/share/calamares ]; then
		echo "Configuring Calamares for atomic BTRFS..."
		# Add custom module for BTRFS subvolumes
		mkdir -p /etc/calamares/modules
		cat << EOF > /etc/calamares/modules/atomicbtrfs.yaml
		---
		# Example config for custom partitioning
		EOF
		fi

		# Make /usr read-only in concept, but since it's chroot, note for installer

		echo "Atomic setup completed."
		`
		if err := os.WriteFile(hookFile, []byte(hookContent), 0755); err != nil {
			fmt.Printf("Failed to write hook: %v\n", err)
			os.Exit(1)
		}

		// Add includes for hammer binaries
		// Assume the project has a 'hammer-bins' dir with compiled binaries
		hammerDir := filepath.Join("config", "includes.chroot/usr/local/bin")
		if err := os.MkdirAll(hammerDir, 0755); err != nil {
			fmt.Printf("Failed to create hammer dir: %v\n", err)
			os.Exit(1)
		}

		// Placeholder: copy binaries if exist in current dir
		for _, bin := range []string{"hammer-core", "hammer-updater", "hammer-cli", "hammer-builder"} {
			src := bin // Assume in current dir
			if _, err := os.Stat(src); err == nil {
				dst := filepath.Join(hammerDir, bin)
				data, err := os.ReadFile(src)
				if err != nil {
					fmt.Printf("Failed to read %s: %v\n", bin, err)
					continue
				}
				if err := os.WriteFile(dst, data, 0755); err != nil {
					fmt.Printf("Failed to write %s: %v\n", bin, err)
				}
			} else {
				fmt.Printf("Warning: %s not found, skipping.\n", bin)
			}
		}

		// Add hook for symlink or something
		// More hooks if needed

		fmt.Println("Project initialized. Edit config/ as needed.")
		fmt.Println("To include hammer binaries, place them in the current directory before init.")
}

func buildISO(args []string) {
	fs := flag.NewFlagSet("build", flag.ExitOnError)
	fs.Parse(args)

	// Check if in project dir
	if _, err := os.Stat("config"); os.IsNotExist(err) {
		fmt.Println("Not in a live-build project directory. Run 'hammer build init' first.")
		os.Exit(1)
	}

	fmt.Println("Building ISO...")

	// Run lb clean first to ensure clean build
	cleanCmd := exec.Command("lb", "clean")
	cleanCmd.Stdout = os.Stdout
	cleanCmd.Stderr = os.Stderr
	if err := cleanCmd.Run(); err != nil {
		fmt.Printf("Failed to clean: %v\n", err)
		// Continue or exit?
	}

	// Run lb build
	buildCmd := exec.Command("lb", "build")
	buildCmd.Stdout = os.Stdout
	buildCmd.Stderr = os.Stderr
	if err := buildCmd.Run(); err != nil {
		fmt.Printf("Failed to build: %v\n", err)
		os.Exit(1)
	}

	fmt.Println("ISO built successfully. Find it as live-image-amd64.hybrid.iso or similar.")
}

func usage() {
	fmt.Println("Usage: hammer-builder <command> [options]")
	fmt.Println("")
	fmt.Println("Commands:")
	fmt.Println(" init [--suite <suite>] [--atomic]   Initialize live-build project")
	fmt.Println(" build                               Build the atomic ISO")
}
