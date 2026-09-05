package omarchy

import (
	"context"
	"os"
	"path/filepath"
	"testing"
	"time"

	"cosmicmail/internal/models"
)

const colors = `accent = "#ff0000"
foreground = "#eeeeee"
background = "#111111"
cursor = "#00ff00"
selection_foreground = "#010101"
selection_background = "#020202"
color0 = "#000000"
color1 = "#111111"
color15 = "#ffffff"
`

func writeTheme(t *testing.T, dir, name, body string) {
	t.Helper()
	themeDir := filepath.Join(dir, "theme")
	if err := os.MkdirAll(themeDir, 0o755); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(filepath.Join(themeDir, "colors.toml"), []byte(body), 0o644); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(filepath.Join(dir, "theme.name"), []byte(name+"\n"), 0o644); err != nil {
		t.Fatal(err)
	}
}

func TestReadThemeFromFillsMissingColorsFromFallback(t *testing.T) {
	dir := t.TempDir()
	writeTheme(t, dir, "test-theme", colors)
	theme, err := ReadThemeFrom(dir)
	if err != nil {
		t.Fatal(err)
	}
	if theme.Name != "test-theme" || theme.Accent != "#ff0000" || theme.Palette[0] != "#000000" || theme.Palette[15] != "#ffffff" {
		t.Fatalf("%+v", theme)
	}
	if theme.Palette[4] != KanagawaFallback().Palette[4] || len(theme.Palette) != 16 {
		t.Fatalf("fallback palette: %+v", theme.Palette)
	}
}

func TestReadThemeFallsBackWhenMissing(t *testing.T) {
	t.Setenv("XDG_CONFIG_HOME", t.TempDir())
	if got := ReadTheme(); !Equal(got, KanagawaFallback()) {
		t.Fatalf("%+v", got)
	}
}

func TestWatchDirEmitsOnlyRealChanges(t *testing.T) {
	dir := t.TempDir()
	writeTheme(t, dir, "one", colors)
	ctx, cancel := context.WithCancel(context.Background())
	defer cancel()
	emitted := make(chan models.OmarchyTheme, 4)
	done := make(chan error, 1)
	go func() { done <- WatchDir(ctx, dir, func(th models.OmarchyTheme) { emitted <- th }) }()
	time.Sleep(100 * time.Millisecond)

	// Unrelated churn: a file appears in the dir but the theme is unchanged.
	os.WriteFile(filepath.Join(dir, "background"), []byte("x"), 0o644)
	select {
	case th := <-emitted:
		t.Fatalf("unexpected emit: %+v", th)
	case <-time.After(2 * Debounce):
	}

	// A real switch: theme.name changes.
	os.WriteFile(filepath.Join(dir, "theme.name"), []byte("two\n"), 0o644)
	select {
	case th := <-emitted:
		if th.Name != "two" {
			t.Fatalf("%+v", th)
		}
	case <-time.After(3 * time.Second):
		t.Fatal("no emit after theme switch")
	}
	cancel()
	if err := <-done; err != nil {
		t.Fatal(err)
	}
}
