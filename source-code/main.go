package main

import (
	"fmt"
	"os"
	"os/exec"
	"path/filepath"
	"strings"
	"time"
)

const (
	mountPoint     = "/mnt/hroot"
	snapshotPrefix = "-pre-update-" // teraz używamy tej stałej
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
		rollbackCmd(os.Args[2:])
	case "install":
		installCmd(os.Args[2:])
	case "remove":
		removeCmd(os.Args[2:])
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
  snapshot Create a read-only snapshot of the current root
  update   Create and update a new snapshot offline
  switch   Switch to the updated snapshot (@update → default)
  rollback <name> Rollback to a specific snapshot (e.g. @pre-update-20251130-2013)
  install <pkg>... Install package(s) in the current root (non-atomic)
  remove <pkg>...  Remove package(s) from the current root (non-atomic)
  clean    Clean apt cache (snapshots must be deleted manually)
  status   List available snapshots`)
}

func runCommand(name string, args ...string) error {
	cmd := exec.Command(name, args...)
	cmd.Stdout = os.Stdout
	cmd.Stderr = os.Stderr
	return cmd.Run()
}

func getSnapshotName() string {
	return rootSubvolume + snapshotPrefix + time.Now().Format("20060102-1504")
}

// Pobiera Subvolume ID dla podanej względnej ścieżki (np. @update, @pre-update-...)
func getSubvolumeID(subvol string) (string, error) {
	path := "/" + subvol // /@update, /@pre-update-20251130-2013 itd.
	output, err := exec.Command("btrfs", "subvolume", "show", path).CombinedOutput()
	if err != nil {
		return "", fmt.Errorf("btrfs subvolume show %s failed: %v\n%s", path, err, output)
	}

	for _, line := range strings.Split(string(output), "\n") {
		line = strings.TrimSpace(line)
		if strings.HasPrefix(line, "Subvolume ID:") {
			parts := strings.Fields(line)
			if len(parts) >= 3 {
				return parts[2], nil
			}
		}
	}
	return "", fmt.Errorf("Subvolume ID not found for %s", path)
}

func snapshotCmd() {
	snapshotName := getSnapshotName()
	fmt.Printf("Creating read-only snapshot: %s\n", snapshotName)
	if err := runCommand("btrfs", "subvolume", "snapshot", "-r", "/", snapshotName); err != nil {
		fmt.Fprintf(os.Stderr, "Error creating snapshot: %v\n", err)
		os.Exit(1)
	}
	fmt.Println("Snapshot created successfully.")
}

func updateCmd() {
	// Krok 1: Tworzymy writable snapshot do aktualizacji
	fmt.Println("Creating update snapshot:", updateSnapshot)
	if err := runCommand("btrfs", "subvolume", "snapshot", "/", updateSnapshot); err != nil {
		fmt.Fprintf(os.Stderr, "Error creating update snapshot: %v\n", err)
		os.Exit(1)
	}

	// Krok 2: Montujemy snapshot
	os.MkdirAll(mountPoint, 0755)
	if err := runCommand("mount", btrfsDevice, mountPoint, "-o", "subvol="+updateSnapshot); err != nil {
		fmt.Fprintf(os.Stderr, "Error mounting update snapshot: %v\n", err)
		os.Exit(1)
	}
	defer runCommand("umount", mountPoint)

	// Bind mount niezbędnych systemów plików
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

	// Krok 3: Chroot + aktualizacja
	fmt.Println("Performing system update in chroot...")
	if err := runCommand("chroot", mountPoint, "apt", "update"); err != nil {
		fmt.Fprintf(os.Stderr, "Error running apt update: %v\n", err)
		os.Exit(1)
	}
	if err := runCommand("chroot", mountPoint, "apt", "upgrade", "-y"); err != nil {
		fmt.Fprintf(os.Stderr, "Error running apt upgrade: %v\n", err)
		os.Exit(1)
	}

	fmt.Println("Update completed successfully in snapshot:", updateSnapshot)
	fmt.Println("Run 'hroot switch' and reboot to apply.")
}

func switchCmd() {
	id, err := getSubvolumeID(updateSnapshot)
	if err != nil {
		fmt.Fprintf(os.Stderr, "Cannot get subvolume ID for %s: %v\n", updateSnapshot, err)
		os.Exit(1)
	}

	fmt.Printf("Setting default subvolume to %s (ID: %s)\n", updateSnapshot, id)
	if err := runCommand("btrfs", "subvolume", "set-default", id, "/"); err != nil {
		fmt.Fprintf(os.Stderr, "Error setting default subvolume: %v\n", err)
		os.Exit(1)
	}
	fmt.Println("Default subvolume changed. Reboot required.")
}

func rollbackCmd(args []string) {
	if len(args) == 0 {
		fmt.Fprintf(os.Stderr, "Usage: hroot rollback <snapshot-name>\n")
		os.Exit(1)
	}

	snapshotName := args[0]
	id, err := getSubvolumeID(snapshotName)
	if err != nil {
		fmt.Fprintf(os.Stderr, "Cannot get subvolume ID for %s: %v\n", snapshotName, err)
		os.Exit(1)
	}

	fmt.Printf("Rolling back to %s (ID: %s)\n", snapshotName, id)
	if err := runCommand("btrfs", "subvolume", "set-default", id, "/"); err != nil {
		fmt.Fprintf(os.Stderr, "Error setting default subvolume: %v\n", err)
		os.Exit(1)
	}
	fmt.Println("Rollback successful. Reboot required.")
}

func installCmd(pkgs []string) {
	if len(pkgs) == 0 {
		fmt.Fprintf(os.Stderr, "Usage: hroot install <package>...\n")
		os.Exit(1)
	}
	fmt.Printf("Installing packages (live system): %v\n", pkgs)
	args := append([]string{"install", "-y"}, pkgs...)
	if err := runCommand("apt", args...); err != nil {
		fmt.Fprintf(os.Stderr, "Error installing packages: %v\n", err)
		os.Exit(1)
	}
}

func removeCmd(pkgs []string) {
	if len(pkgs) == 0 {
		fmt.Fprintf(os.Stderr, "Usage: hroot remove <package>...\n")
		os.Exit(1)
	}
	fmt.Printf("Removing packages (live system): %v\n", pkgs)
	args := append([]string{"remove", "-y"}, pkgs...)
	if err := runCommand("apt", args...); err != nil {
		fmt.Fprintf(os.Stderr, "Error removing packages: %v\n", err)
		os.Exit(1)
	}
}

func cleanCmd() {
	fmt.Println("Cleaning apt cache...")
	if err := runCommand("apt", "clean"); err != nil {
		fmt.Fprintf(os.Stderr, "Error cleaning apt cache: %v\n", err)
	}
	fmt.Println("Done. Delete old snapshots manually with 'btrfs subvolume delete /<name>'")
}

func statusCmd() {
	output, err := exec.Command("btrfs", "subvolume", "list", "-p", "/").CombinedOutput()
	if err != nil {
		fmt.Fprintf(os.Stderr, "Error listing subvolumes: %v\n", err)
		os.Exit(1)
	}

	fmt.Println("Available snapshots:")
	lines := strings.Split(string(output), "\n")
	for _, line := range lines {
		if strings.Contains(line, rootSubvolume) || strings.Contains(line, updateSnapshot) {
			fields := strings.Fields(line)
			if len(fields) >= 9 {
				id := fields[1]
				path := fields[len(fields)-1]
				fmt.Printf("  ID %-6s → %s\n", id, path)
			}
		}
	}

	defOutput, err := exec.Command("btrfs", "subvolume", "get-default", "/").CombinedOutput()
	if err == nil {
		fmt.Printf("\nCurrent default: %s", defOutput)
	}
}
