// Package xdg resolves the per-user directories the Rust build used through
// the `dirs` crate, so an upgraded install finds its data where it was left:
// $XDG_CONFIG_HOME (~/.config), $XDG_DATA_HOME (~/.local/share) and the XDG
// downloads directory. Every function reads the environment at call time so
// tests can point the app at a scratch profile with t.Setenv.
package xdg

import (
	"bufio"
	"errors"
	"os"
	"path/filepath"
	"strings"
)

// ConfigDir is $XDG_CONFIG_HOME, falling back to ~/.config.
func ConfigDir() (string, error) {
	if v := os.Getenv("XDG_CONFIG_HOME"); filepath.IsAbs(v) {
		return v, nil
	}
	home, err := os.UserHomeDir()
	if err != nil {
		return "", errors.New("could not determine config dir (XDG_CONFIG_HOME)")
	}
	return filepath.Join(home, ".config"), nil
}

// DataDir is $XDG_DATA_HOME, falling back to ~/.local/share.
func DataDir() (string, error) {
	if v := os.Getenv("XDG_DATA_HOME"); filepath.IsAbs(v) {
		return v, nil
	}
	home, err := os.UserHomeDir()
	if err != nil {
		return "", errors.New("could not determine data dir (XDG_DATA_HOME)")
	}
	return filepath.Join(home, ".local", "share"), nil
}

// AppConfigDir is `$XDG_CONFIG_HOME/cosmic-mail`.
func AppConfigDir() (string, error) {
	dir, err := ConfigDir()
	if err != nil {
		return "", err
	}
	return filepath.Join(dir, "cosmic-mail"), nil
}

// AppDataDir is `$XDG_DATA_HOME/cosmic-mail`.
func AppDataDir() (string, error) {
	dir, err := DataDir()
	if err != nil {
		return "", err
	}
	return filepath.Join(dir, "cosmic-mail"), nil
}

// DownloadDir mirrors `dirs::download_dir()`: the XDG_DOWNLOAD_DIR entry of
// `user-dirs.dirs`, or ~/Downloads. The second return is false when neither
// can be resolved.
func DownloadDir() (string, bool) {
	home, err := os.UserHomeDir()
	if err != nil {
		return "", false
	}
	if cfg, err := ConfigDir(); err == nil {
		if dir := userDirsEntry(filepath.Join(cfg, "user-dirs.dirs"), "XDG_DOWNLOAD_DIR", home); dir != "" {
			return dir, true
		}
	}
	return filepath.Join(home, "Downloads"), true
}

// userDirsEntry parses one `KEY="$HOME/..."` line from user-dirs.dirs.
func userDirsEntry(path, key, home string) string {
	f, err := os.Open(path)
	if err != nil {
		return ""
	}
	defer f.Close()
	sc := bufio.NewScanner(f)
	for sc.Scan() {
		line := strings.TrimSpace(sc.Text())
		if !strings.HasPrefix(line, key+"=") {
			continue
		}
		val := strings.Trim(strings.TrimPrefix(line, key+"="), "\"")
		val = strings.ReplaceAll(val, "$HOME", home)
		if val == "" || !filepath.IsAbs(val) {
			return ""
		}
		return val
	}
	return ""
}
