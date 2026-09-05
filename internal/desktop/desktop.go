// Package desktop covers the process and window lifecycle.
//
// The application process owns background sync for the whole graphical
// session. Its main window is only a view onto that process: closing the
// view hides it, while launcher and notification activations show the same
// window.
package desktop

import (
	"log/slog"
	"os"
	"os/exec"

	"github.com/wailsapp/wails/v3/pkg/application"
)

const (
	// BackgroundArg requests a silent, background-only startup (the systemd
	// user service passes it).
	BackgroundArg = "--background"
	// MainWindow is the name of the main webview window.
	MainWindow = "main"
	// WindowClass is the GTK program name / Wayland app id, matched by the
	// desktop entry's StartupWMClass and the Hyprland focus fallback.
	WindowClass = "cosmic-mail"
)

// IsBackgroundLaunch reports whether argv requests a background-only startup.
func IsBackgroundLaunch(args []string) bool {
	for _, a := range args {
		if a == BackgroundArg {
			return true
		}
	}
	return false
}

// ActivateMainWindow shows, unminimises, and focuses the main window. Shared
// by launcher re-entry, the tray, and notification actions so all explicit
// user activations follow the same Wayland/Hyprland behaviour.
func ActivateMainWindow(app *application.App) {
	window, ok := app.Window.GetByName(MainWindow)
	if !ok || window == nil {
		slog.Warn("main window is unavailable for activation")
		return
	}
	application.InvokeSync(func() {
		window.UnMinimise()
		window.Show()
		window.Focus()
	})
	// Wayland compositors may reject focus requests from a background
	// client. Cosmic Mail targets Omarchy/Hyprland, whose IPC honours an
	// explicit user activation. Fixed arguments, no shell.
	if os.Getenv("HYPRLAND_INSTANCE_SIGNATURE") != "" {
		slog.Debug("requesting application focus through hyprctl")
		if err := hyprlandFocus(); err != nil {
			slog.Warn("hyprctl application focus failed", "error", err)
		}
	}
}

// hyprlandFocus asks Hyprland to focus the app window by class. Hyprland
// 0.56+ takes Lua dispatches (`hl.dsp.focus`); the classic
// `focuswindow class:…` form is tried second for older releases — the same
// pair omarchy's own launch-or-focus helper uses. Never through a shell, and
// no notification/message content is ever interpolated.
func hyprlandFocus() error {
	lua := exec.Command("hyprctl", "dispatch", `hl.dsp.focus({ window = "class:`+WindowClass+`" })`)
	if err := lua.Run(); err == nil {
		return nil
	}
	return exec.Command("hyprctl", "dispatch", "focuswindow", "class:^("+WindowClass+")$").Run()
}
