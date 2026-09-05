package settings

import (
	"os"
	"path/filepath"
	"strings"
	"testing"

	"cosmicmail/internal/models"
)

func TestDefaultsWhenFileMissing(t *testing.T) {
	path := filepath.Join(t.TempDir(), "settings.json")
	if got := loadFrom(path); got != (models.Settings{}) || got.AlwaysDownloadRemoteImages {
		t.Fatalf("got %+v", got)
	}
}

func TestDefaultsWhenMalformed(t *testing.T) {
	path := filepath.Join(t.TempDir(), "settings.json")
	if err := os.WriteFile(path, []byte("{ this is not valid json"), 0o644); err != nil {
		t.Fatal(err)
	}
	if got := loadFrom(path); got != (models.Settings{}) {
		t.Fatalf("got %+v", got)
	}
}

func TestToleratesUnknownAndMissingKeys(t *testing.T) {
	path := filepath.Join(t.TempDir(), "settings.json")
	if err := os.WriteFile(path, []byte(`{"somethingNew": 42}`), 0o644); err != nil {
		t.Fatal(err)
	}
	if got := loadFrom(path); got != (models.Settings{}) {
		t.Fatalf("got %+v", got)
	}
}

func TestRoundTrip(t *testing.T) {
	path := filepath.Join(t.TempDir(), "settings.json")
	want := models.Settings{AlwaysDownloadRemoteImages: true}
	if err := saveTo(path, want); err != nil {
		t.Fatal(err)
	}
	if got := loadFrom(path); got != want {
		t.Fatalf("got %+v", got)
	}
}

func TestPersistsCamelCaseKeyOnDisk(t *testing.T) {
	path := filepath.Join(t.TempDir(), "settings.json")
	if err := saveTo(path, models.Settings{AlwaysDownloadRemoteImages: true}); err != nil {
		t.Fatal(err)
	}
	raw, err := os.ReadFile(path)
	if err != nil {
		t.Fatal(err)
	}
	if !strings.Contains(string(raw), "alwaysDownloadRemoteImages") || strings.Contains(string(raw), "always_download_remote_images") {
		t.Fatalf("raw: %s", raw)
	}
}

func TestLoadAndSaveUseTheXdgProfile(t *testing.T) {
	t.Setenv("XDG_CONFIG_HOME", t.TempDir())
	if got := Load(); got != (models.Settings{}) {
		t.Fatalf("fresh profile should be defaults: %+v", got)
	}
	if err := Save(models.Settings{AlwaysDownloadRemoteImages: true}); err != nil {
		t.Fatal(err)
	}
	if got := Load(); !got.AlwaysDownloadRemoteImages {
		t.Fatalf("not persisted: %+v", got)
	}
}
