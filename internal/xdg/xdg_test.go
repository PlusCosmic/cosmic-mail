package xdg

import (
	"os"
	"path/filepath"
	"testing"
)

func TestEnvOverridesWin(t *testing.T) {
	t.Setenv("XDG_CONFIG_HOME", "/tmp/cfg")
	t.Setenv("XDG_DATA_HOME", "/tmp/data")
	if d, _ := AppConfigDir(); d != "/tmp/cfg/cosmic-mail" {
		t.Fatalf("config: %s", d)
	}
	if d, _ := AppDataDir(); d != "/tmp/data/cosmic-mail" {
		t.Fatalf("data: %s", d)
	}
}

func TestRelativeEnvIsIgnored(t *testing.T) {
	t.Setenv("XDG_CONFIG_HOME", "relative")
	home, _ := os.UserHomeDir()
	if d, _ := ConfigDir(); d != filepath.Join(home, ".config") {
		t.Fatalf("got %s", d)
	}
}

func TestDownloadDirReadsUserDirs(t *testing.T) {
	cfg := t.TempDir()
	t.Setenv("XDG_CONFIG_HOME", cfg)
	home, _ := os.UserHomeDir()
	if err := os.WriteFile(filepath.Join(cfg, "user-dirs.dirs"), []byte("XDG_DESKTOP_DIR=\"$HOME/Desktop\"\nXDG_DOWNLOAD_DIR=\"$HOME/Incoming\"\n"), 0o644); err != nil {
		t.Fatal(err)
	}
	if d, ok := DownloadDir(); !ok || d != filepath.Join(home, "Incoming") {
		t.Fatalf("got %s %v", d, ok)
	}
	if err := os.Remove(filepath.Join(cfg, "user-dirs.dirs")); err != nil {
		t.Fatal(err)
	}
	if d, ok := DownloadDir(); !ok || d != filepath.Join(home, "Downloads") {
		t.Fatalf("fallback: got %s %v", d, ok)
	}
}
