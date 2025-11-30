package main

import (
	"flag"
	"fmt"
	"os"
	"os/exec"
	"path/filepath"
	"strconv"
	"strings"
	"time"
)

const (
	mountPoint    = "/mnt/hroot"
	snapshotPrefix = "@pre-update-"
	updateSnapshot = "@update"
	btrfsDevice    = "/dev/sda1" // TODO: Detect or configure the Btrfs device
	rootSubvolume  = "@"
)

func main() {
	if len(os.Args) < 2 {
		usage()
		os.Exit(1)
	}

	cmd := os.Args[1]
	switch cmd {
	case "snapshot":
		snapshotCmd()
	case "update":
		updateCmd()
	case "switch":
		switchCmd()
	case "rollback":
		rollbackCmd(flag.Args()[1:])
	case "install":
		installCmd(flag.Args()[1:])
	case "remove":
		removeCmd(flag.Args()[1:])
	case "clean":
		cleanCmd()
	case "status":
		statusCmd()
	default:
		usage()
		os.Exit(1)
	}
}

func usage() {
	fmt.Println(`HROOT - HackerOS Root
Usage: hroot <command> [args]

Commands:
  snapshot          Create a read-only snapshot of the current root
  update            Create and update a new snapshot offline
  switch            Switch to the updated snapshot
  rollback <name>   Rollback to a specific snapshot by name (e.g., @pre-update-20251130-2013)
  install <pkg>     Install a package in the current root (non-atomic, for simplicity)
  remove <pkg>      Remove a package from the current root (non-atomic)
  clean             Clean up temporary files and unused snapshots
  status            List available snapshots`)
}

func runCommand(name string, args ...string) error {
	cmd := exec.Command(name, args...)
	cmd.Stdout = os.Stdout
	cmd.Stderr = os.Stderr
	return cmd.Run()
}

func getSnapshotName() string {
	return rootSubvolume + "pre-update-" + time.Now().Format("20060102-1504")
}

func snapshotCmd() {
	snapshotName := getSnapshotName()
	fmt.Printf("Creating snapshot: %s\n", snapshotName)
	if err := runCommand("btrfs", "subvolume", "snapshot", "-r", "/", snapshotName); err != nil {
		fmt.Fprintf(os.Stderr, "Error creating snapshot: %v\n", err)
		os.Exit(1)
	}
	fmt.Println("Snapshot created successfully.")
}

func updateCmd() {
	// Step 1: Create writable snapshot for update
	fmt.Println("Creating update snapshot:", updateSnapshot)
	if err := runCommand("btrfs", "subvolume", "snapshot", "/", updateSnapshot); err != nil {
		fmt.Fprintf(os.Stderr, "Error creating update snapshot: %v\n", err)
		os.Exit(1)
	}

	// Step 2: Mount the snapshot
	os.MkdirAll(mountPoint, 0755)
	if err := runCommand("mount", btrfsDevice, mountPoint, "-o", "subvol="+updateSnapshot); err != nil {
		fmt.Fprintf(os.Stderr, "Error mounting update snapshot: %v\n", err)
		os.Exit(1)
	}
	defer runCommand("umount", mountPoint) // Clean up on exit

	// Bind mount necessary filesystems
	bindMounts := []string{"/proc", "/sys", "/dev", "/run"}
	for _, m := range bindMounts {
		target := filepath.Join(mountPoint, m[1:])
		os.MkdirAll(target, 0755)
		if err := runCommand("mount", "--bind", m, target); err != nil {
			fmt.Fprintf(os.Stderr, "Error bind mounting %s: %v\n", m, err)
			os.Exit(1)
		}
		defer runCommand("umount", target)
	}

	// Step 3: Chroot and perform update
	fmt.Println("Performing system update in chroot...")
	chrootCmd := []string{"chroot", mountPoint, "apt", "update"}
	if err := runCommand(chrootCmd[0], chrootCmd[1:]...); err != nil {
		fmt.Fprintf(os.Stderr, "Error running apt update: %v\n", err)
		os.Exit(1)
	}
	chrootCmd = []string{"chroot", mountPoint, "apt", "upgrade", "-y"}
	if err := runCommand(chrootCmd[0], chrootCmd[1:]...); err != nil {
		fmt.Fprintf(os.Stderr, "Error running apt upgrade: %v\n", err)
		os.Exit(1)
	}

	fmt.Println("Update completed in snapshot.")
}

func switchCmd() {
	// Get ID of update snapshot
	out, err := exec.Command("btrfs", "subvolume", "find-new", "/", "9999999").CombinedOutput() // Hack to get current ID, but better to list
	if err != nil {
		fmt.Fprintf(os.Stderr, "Error finding subvolume ID: %v\n", err)
		os.Exit(1)
	}
	// Parse ID from output - this is simplistic, assume we know path
	fmt.Println("Switching default subvolume to", updateSnapshot)
	if err := runCommand("btrfs", "subvolume", "set-default", updateSnapshot); err != nil {
		fmt.Fprintf(os.Stderr, "Error setting default subvolume: %v\n", err)
		os.Exit(1)
	}
	fmt.Println("Default subvolume switched. Reboot to apply.")
}

func rollbackCmd(args []string) {
	if len(args) == 0 {
		fmt.Fprintf(os.Stderr, "Rollback requires snapshot name\n")
		os.Exit(1)
	}
	snapshotName := args[0]
	fmt.Printf("Rolling back to %s\n", snapshotName)
	if err := runCommand("btrfs", "subvolume", "set-default", snapshotName); err != nil {
		fmt.Fprintf(os.Stderr, "Error setting default subvolume: %v\n", err)
		os.Exit(1)
	}
	fmt.Println("Rollback set. Reboot to apply.")
}

func installCmd(packages []string) {
	if len(packages) == 0 {
		fmt.Fprintf(os.Stderr, "Install requires package names\n")
		os.Exit(1)
	}
	fmt.Printf("Installing packages: %v\n", packages)
	args := append([]string{"install", "-y"}, packages...)
	if err := runCommand("apt", args...); err != nil {
		fmt.Fprintf(os.Stderr, "Error installing packages: %v\n", err)
		os.Exit(1)
	}
}

func removeCmd(packages []string) {
	if len(packages) == 0 {
		fmt.Fprintf(os.Stderr, "Remove requires package names\n")
		os.Exit(1)
	}
	fmt.Printf("Removing packages: %v\n", packages)
	args := append([]string{"remove", "-y"}, packages...)
	if err := runCommand("apt", args...); err != nil {
		fmt.Fprintf(os.Stderr, "Error removing packages: %v\n", err)
		os.Exit(1)
	}
}

func cleanCmd() {
	fmt.Println("Cleaning temporary files...")
	if err := runCommand("apt", "clean"); err != nil {
		fmt.Fprintf(os.Stderr, "Error cleaning apt cache: %v\n", err)
	}
	// Optionally delete old snapshots - manual for now
	fmt.Println("List snapshots with 'hroot status' and delete manually if needed.")
}

func statusCmd() {
	out, err := exec.Command("btrfs", "subvolume", "list", "-p", "/").CombinedOutput()
	if err != nil {
		fmt.Fprintf(os.Stderr, "Error listing subvolumes: %v\n", err)
		os.Exit(1)
	}
	lines := strings.Split(string(out), "\n")
	fmt.Println("Available snapshots:")
	for _, line := range lines {
		if strings.Contains(line, rootSubvolume) {
			fields := strings.Fields(line)
			if len(fields) > 8 {
				id := fields[1]
				path := fields[8]
				if strings.HasPrefix(path, rootSubvolume) {
					fmt.Printf("ID: %s, Path: %s\n", id, path)
				}
			}
		}
	}

	// Get default
	defOut, defErr := exec.Command("btrfs", "subvolume", "get-default", "/").CombinedOutput()
	if defErr == nil {
		defFields := strings.Fields(string(defOut))
		if len(defFields) > 1 {
			defID := defFields[1]
			fmt.Printf("Current default ID: %s\n", defID)
		}
	}
}
