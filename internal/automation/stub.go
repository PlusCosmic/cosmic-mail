//go:build production

// Package automation is compiled out of production builds: every entry
// point is a no-op and the bridge never appears in a shipped binary.
package automation

import "github.com/wailsapp/wails/v3/pkg/application"

// Enabled reports whether the bridge is compiled in.
const Enabled = false

// Spawn does nothing in production builds.
func Spawn(*application.App) {}

// HandleRawMessage does nothing in production builds.
func HandleRawMessage(application.Window, string, *application.OriginInfo) {}
