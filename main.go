// Cosmic Mail — a native Linux mail client for omarchy, built with Wails 3.
//
// main wires the pieces together: the SQLite store, the per-account sync
// engine, omarchy theme watching, mako notifications, the tray, the
// single-instance owner, and the App service the frontend calls.
package main

import (
	"context"
	"embed"
	"io/fs"
	"log/slog"
	"os"

	"github.com/wailsapp/wails/v3/pkg/application"
	"github.com/wailsapp/wails/v3/pkg/events"

	"cosmicmail/internal/automation"
	"cosmicmail/internal/desktop"
	"cosmicmail/internal/imap"
	"cosmicmail/internal/mailparse"
	"cosmicmail/internal/models"
	"cosmicmail/internal/notify"
	"cosmicmail/internal/oauth"
	"cosmicmail/internal/omarchy"
	"cosmicmail/internal/send"
	"cosmicmail/internal/store"
	mailsync "cosmicmail/internal/sync"
)

// The SvelteKit static build (frontend/dist/app) is embedded into the binary.
// The pattern is rooted at frontend/dist, whose tracked .gitkeep keeps it
// matching on a checkout that has not built the frontend yet, so go vet /
// go test never fail on "no matching files".
//
//go:embed all:frontend/dist
var assets embed.FS

//go:embed build/appicon.png
var appIcon []byte

// The events the backend emits (docs/ARCHITECTURE.md "Events"). Registered
// with constant names so `wails3 generate bindings` types them for the
// frontend.
func init() {
	application.RegisterEvent[models.OmarchyTheme]("omarchy:theme-changed")
	application.RegisterEvent[models.NewMessagesEvent]("mail:new-messages")
	application.RegisterEvent[models.MessagesUpdatedEvent]("mail:messages-updated")
	application.RegisterEvent[models.SyncStateEvent]("mail:sync-state")
}

// wailsEmitter forwards backend events to the frontend through the Wails
// event manager.
type wailsEmitter struct{}

func (wailsEmitter) Emit(name string, data any) {
	if app := application.Get(); app != nil {
		app.Event.Emit(name, data)
	}
}

func main() {
	if os.Getenv("WEBKIT_DISABLE_DMABUF_RENDERER") == "" {
		// WebKitGTK's DMA-BUF renderer crashes the Wayland connection ("Gdk
		// Error 71 (Protocol error)") on NVIDIA + wlroots compositors like
		// Hyprland. Wails only sets this when it detects an NVIDIA GPU; set it
		// unconditionally like the Tauri build did, so hybrid setups are
		// covered too. Must be set before GTK/WebKit initialise.
		_ = os.Setenv("WEBKIT_DISABLE_DMABUF_RENDERER", "1")
	}
	setupLogging()

	st, err := store.Open()
	if err != nil {
		slog.Error("could not open the mail database", "error", err)
		os.Exit(1)
	}
	// Heal preview snippets of already-cached bodies with the current snippet
	// logic, synchronously — before any sync task (it must heal even when
	// sync can't run) and before the webview fetches its first pages.
	switch count, err := st.HealCachedSnippets(mailparse.SnippetForBody); {
	case err != nil:
		slog.Warn("snippet healing failed", "error", err)
	case count > 0:
		slog.Info("healed stale message snippets", "count", count)
	}

	// The SMTP client shares the IMAP TLS configuration (and its debug
	// extra-CA hook).
	send.TLSConfig = imap.TLSConfig

	var app *application.App
	activate := func() {
		if app != nil {
			desktop.ActivateMainWindow(app)
		}
	}
	notifier := notify.New(activate)
	emitter := wailsEmitter{}
	syncManager := mailsync.NewManager(st, emitter, notifier)
	service := NewApp(st, syncManager, emitter, notifier)
	background := desktop.IsBackgroundLaunch(os.Args[1:])

	app = application.New(application.Options{
		Name:        "Cosmic Mail",
		Description: "A native Linux mail client for omarchy",
		Icon:        appIcon,
		Services: []application.Service{
			application.NewService(service),
		},
		Assets: application.AssetOptions{
			Handler: application.AssetFileServerFS(frontendAssets()),
		},
		Linux: application.LinuxOptions{
			ProgramName:                   desktop.WindowClass,
			DisableQuitOnLastWindowClosed: true,
		},
		// Exactly one process/sync-engine owner per desktop session: a second
		// launch forwards its argv to the owner and exits. A normal launcher
		// invocation activates the existing window; a second --background
		// invocation (a service restart) is silent so it cannot steal focus.
		SingleInstance: &application.SingleInstanceOptions{
			UniqueID: "dev.pluscosmic.mail",
			OnSecondInstanceLaunch: func(data application.SecondInstanceData) {
				if !desktop.IsBackgroundLaunch(data.Args) {
					activate()
				}
			},
		},
		// The development-only automation bridge posts eval results back
		// through the webview's raw message channel. A no-op in production.
		RawMessageHandler: automation.HandleRawMessage,
		OnShutdown: func() {
			syncManager.StopAll()
			_ = st.Close()
		},
	})
	oauth.OpenURL = app.Browser.OpenURL

	// The window is created hidden to avoid a login flash. Only an
	// interactive first launch reveals it; --background is used by the
	// graphical-session service.
	window := app.Window.NewWithOptions(application.WebviewWindowOptions{
		Name:             desktop.MainWindow,
		Title:            "Cosmic Mail",
		Width:            1280,
		Height:           800,
		MinWidth:         900,
		MinHeight:        560,
		Frameless:        true,
		Hidden:           true,
		BackgroundColour: application.NewRGB(31, 31, 40),
		URL:              "/",
	})
	// Closing the main window hides it; the process and its sync tasks keep
	// running. Session shutdown / systemd stop remains an actual exit.
	window.RegisterHook(events.Common.WindowClosing, func(e *application.WindowEvent) {
		e.Cancel()
		window.Hide()
	})

	setupTray(app, service, activate)

	if !background {
		app.Event.OnApplicationEvent(events.Common.ApplicationStarted, func(*application.ApplicationEvent) {
			activate()
		})
	}

	ctx, cancel := context.WithCancel(context.Background())
	defer cancel()
	go func() {
		if err := omarchy.Watch(ctx, func(theme models.OmarchyTheme) {
			emitter.Emit(models.EventThemeChanged, theme)
		}); err != nil {
			slog.Warn("omarchy watcher stopped", "error", err)
		}
	}()
	automation.Spawn(app)
	service.startConfiguredAccounts()

	if err := app.Run(); err != nil {
		slog.Error("application failed", "error", err)
		os.Exit(1)
	}
}

// frontendAssets is the embedded SvelteKit bundle. On a checkout with no
// frontend build it is empty, which only matters to a binary someone runs
// without `npm run build` — every asset request then 404s.
func frontendAssets() fs.FS {
	sub, err := fs.Sub(assets, "frontend/dist/app")
	if err != nil {
		slog.Warn("frontend bundle missing from the binary; run npm run build in frontend/", "error", err)
		return assets
	}
	return sub
}

// setupTray keeps one tray icon and its menu attached for the lifetime of
// the process, including background-only launches. Linux tray interaction
// is menu-driven: Open Cosmic Mail, Sync now, Quit.
func setupTray(app *application.App, service *App, activate func()) {
	tray := app.SystemTray.New()
	tray.SetIcon(appIcon)
	tray.SetTooltip("Cosmic Mail")
	menu := app.NewMenu()
	menu.Add("Open Cosmic Mail").OnClick(func(*application.Context) { activate() })
	menu.Add("Sync now").OnClick(func(*application.Context) {
		count, err := service.syncAllAccounts()
		if err != nil {
			slog.Warn("tray sync request failed", "error", err)
			return
		}
		slog.Info("tray requested sync", "accounts", count)
	})
	menu.AddSeparator()
	menu.Add("Quit").OnClick(func(*application.Context) {
		slog.Info("tray requested application exit")
		app.Quit()
	})
	tray.SetMenu(menu)
}

// setupLogging honours COSMIC_MAIL_LOG (debug|info|warn|error), defaulting
// to info, on stderr.
func setupLogging() {
	level := slog.LevelInfo
	switch os.Getenv("COSMIC_MAIL_LOG") {
	case "debug":
		level = slog.LevelDebug
	case "warn":
		level = slog.LevelWarn
	case "error":
		level = slog.LevelError
	}
	slog.SetDefault(slog.New(slog.NewTextHandler(os.Stderr, &slog.HandlerOptions{Level: level})))
}
