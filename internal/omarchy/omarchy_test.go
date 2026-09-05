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

// namedColors is the current omarchy colors.toml dialect (a trimmed copy of
// /usr/share/omarchy/themes/gruvbox/colors.toml).
const namedColors = `mode = "dark"

accent = "#7daea3"
selection = "#504945"
muted = "#665c54"

background = "#282828"
dark_background = "#1e1e1e"
darker_background = "#161616"
lighter_background = "#3c3836"

foreground = "#d4be98"
dark_foreground = "#7c6f64"
light_foreground = "#bdae93"
bright_foreground = "#fbf1c7"

red = "#ea6962"
yellow = "#d8a657"
orange = "#e1875c"
green = "#a9b665"
cyan = "#89b482"
blue = "#7daea3"
magenta = "#d3869b"
brown = "#70432e"

bright_red = "#fb4934"
bright_yellow = "#fabd2f"
bright_green = "#b8bb26"
bright_cyan = "#8ec07c"
bright_blue = "#83a598"
bright_magenta = "#d3869b"
`

func TestReadThemeFromMapsNamedColorsOntoPalette(t *testing.T) {
	dir := t.TempDir()
	writeTheme(t, dir, "gruvbox", namedColors)
	theme, err := ReadThemeFrom(dir)
	if err != nil {
		t.Fatal(err)
	}
	want := models.OmarchyTheme{
		Name: "gruvbox", Accent: "#7daea3", Foreground: "#d4be98", Background: "#282828", Cursor: "#d4be98",
		SelectionForeground: "#d4be98", SelectionBackground: "#504945",
		Palette: []string{
			"#161616", "#ea6962", "#a9b665", "#d8a657", "#7daea3", "#d3869b", "#89b482", "#d4be98",
			"#665c54", "#fb4934", "#b8bb26", "#fabd2f", "#83a598", "#d3869b", "#8ec07c", "#fbf1c7",
		},
	}
	if !Equal(theme, want) {
		t.Fatalf("got  %+v\nwant %+v", theme, want)
	}
}

func TestReadThemeFromNamedColorsWithoutDarkerBackgroundUsesBackground(t *testing.T) {
	dir := t.TempDir()
	writeTheme(t, dir, "flat", "background = \"#282828\"\nforeground = \"#d4be98\"\n")
	theme, err := ReadThemeFrom(dir)
	if err != nil {
		t.Fatal(err)
	}
	if theme.Palette[0] != "#282828" || theme.Palette[7] != "#d4be98" {
		t.Fatalf("%+v", theme.Palette)
	}
}

func TestReadThemeFromPrefersNamedColorsOverLegacyKeys(t *testing.T) {
	dir := t.TempDir()
	writeTheme(t, dir, "mixed", "red = \"#aa0000\"\ncolor1 = \"#bb0000\"\nselection = \"#cc0000\"\nselection_background = \"#dd0000\"\n")
	theme, err := ReadThemeFrom(dir)
	if err != nil {
		t.Fatal(err)
	}
	if theme.Palette[1] != "#aa0000" || theme.SelectionBackground != "#cc0000" {
		t.Fatalf("%+v", theme)
	}
}

func TestReadThemeFallsBackWhenMissing(t *testing.T) {
	t.Setenv("XDG_CONFIG_HOME", t.TempDir())
	t.Setenv("XDG_STATE_HOME", t.TempDir())
	if got := ReadTheme(); !Equal(got, KanagawaFallback()) {
		t.Fatalf("%+v", got)
	}
}

func TestCurrentDirPrefersStateDirThenConfigDir(t *testing.T) {
	state, cfg := t.TempDir(), t.TempDir()
	t.Setenv("XDG_STATE_HOME", state)
	t.Setenv("XDG_CONFIG_HOME", cfg)
	stateCur := filepath.Join(state, "omarchy", "current")
	cfgCur := filepath.Join(cfg, "omarchy", "current")

	// Neither exists: report the current (state) location.
	if got, err := CurrentDir(); err != nil || got != stateCur {
		t.Fatalf("got %q, %v; want %q", got, err, stateCur)
	}
	// Only the old config-dir location exists: fall back to it.
	if err := os.MkdirAll(cfgCur, 0o755); err != nil {
		t.Fatal(err)
	}
	if got, err := CurrentDir(); err != nil || got != cfgCur {
		t.Fatalf("got %q, %v; want %q", got, err, cfgCur)
	}
	// Both exist: the state dir wins.
	if err := os.MkdirAll(stateCur, 0o755); err != nil {
		t.Fatal(err)
	}
	if got, err := CurrentDir(); err != nil || got != stateCur {
		t.Fatalf("got %q, %v; want %q", got, err, stateCur)
	}
}

func TestReadThemeReadsStateDir(t *testing.T) {
	state := t.TempDir()
	t.Setenv("XDG_STATE_HOME", state)
	t.Setenv("XDG_CONFIG_HOME", t.TempDir())
	writeTheme(t, filepath.Join(state, "omarchy", "current"), "gruvbox", namedColors)
	if got := ReadTheme(); got.Name != "gruvbox" || got.Accent != "#7daea3" {
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
