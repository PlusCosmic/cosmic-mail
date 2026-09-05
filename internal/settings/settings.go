// Package settings persists the global, non-secret preferences to
// `$XDG_CONFIG_HOME/cosmic-mail/settings.json`.
//
// Mirrors the config-dir JSON persistence pattern in package accounts. The
// read path never fails: a missing or malformed file yields the defaults so
// a corrupt config can never brick the UI.
package settings

import (
	"encoding/json"
	"fmt"
	"log/slog"
	"os"
	"path/filepath"

	"cosmicmail/internal/models"
	"cosmicmail/internal/xdg"
)

// Path is `$XDG_CONFIG_HOME/cosmic-mail/settings.json`.
func Path() (string, error) {
	dir, err := xdg.AppConfigDir()
	if err != nil {
		return "", err
	}
	return filepath.Join(dir, "settings.json"), nil
}

// loadFrom reads settings from path, falling back to defaults on any read or
// parse problem. Unknown keys are ignored and missing keys take their zero
// value, so older and newer configs both load cleanly.
func loadFrom(path string) models.Settings {
	data, err := os.ReadFile(path)
	if err != nil {
		return models.Settings{}
	}
	var s models.Settings
	if err := json.Unmarshal(data, &s); err != nil {
		return models.Settings{}
	}
	return s
}

// saveTo atomically persists settings to path.
func saveTo(path string, s models.Settings) error {
	dir := filepath.Dir(path)
	if err := os.MkdirAll(dir, 0o755); err != nil {
		return fmt.Errorf("creating %s: %w", dir, err)
	}
	data, err := json.MarshalIndent(s, "", "  ")
	if err != nil {
		return fmt.Errorf("serializing settings: %w", err)
	}
	return WriteFileAtomic(path, data)
}

// WriteFileAtomic writes data to a sibling temp file and renames it into
// place, so a crash mid-write cannot corrupt the target.
func WriteFileAtomic(path string, data []byte) error {
	dir := filepath.Dir(path)
	tmp, err := os.CreateTemp(dir, "."+filepath.Base(path)+".*")
	if err != nil {
		return fmt.Errorf("creating temp file in %s: %w", dir, err)
	}
	tmpName := tmp.Name()
	if _, err := tmp.Write(data); err != nil {
		tmp.Close()
		os.Remove(tmpName)
		return fmt.Errorf("writing %s: %w", tmpName, err)
	}
	if err := tmp.Sync(); err != nil {
		tmp.Close()
		os.Remove(tmpName)
		return fmt.Errorf("flushing %s: %w", tmpName, err)
	}
	if err := tmp.Close(); err != nil {
		os.Remove(tmpName)
		return err
	}
	if err := os.Chmod(tmpName, 0o644); err != nil {
		os.Remove(tmpName)
		return err
	}
	if err := os.Rename(tmpName, path); err != nil {
		os.Remove(tmpName)
		return fmt.Errorf("persisting %s: %w", path, err)
	}
	return nil
}

// Load returns the global settings, or the defaults if the file is absent or
// malformed. It never errors: settings are non-critical preferences.
func Load() models.Settings {
	path, err := Path()
	if err != nil {
		slog.Warn("could not resolve settings path; using defaults", "error", err)
		return models.Settings{}
	}
	return loadFrom(path)
}

// Save persists the global settings to disk.
func Save(s models.Settings) error {
	path, err := Path()
	if err != nil {
		return err
	}
	return saveTo(path, s)
}
