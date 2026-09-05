// Package notify sends desktop notifications over the freedesktop D-Bus
// interface (rendered by mako on omarchy).
//
// Notifications carry the app-name "Cosmic Mail" (so users can theme/script
// per app-name in mako) and a `dev.pluscosmic.mail` desktop-entry hint. Each
// notification exposes a single `default` action; activating it focuses the
// main window. The Wails notification service cannot set the app name or the
// desktop-entry hint, so this speaks D-Bus directly.
package notify

import (
	"context"
	"fmt"
	"log/slog"
	"sync"

	"github.com/godbus/dbus/v5"

	mailsync "cosmicmail/internal/sync"
)

const (
	AppName      = "Cosmic Mail"
	DesktopEntry = "dev.pluscosmic.mail"

	dbusInterface = "org.freedesktop.Notifications"
	dbusPath      = "/org/freedesktop/Notifications"
)

// Notifier sends notifications and dispatches their default action.
type Notifier struct {
	onActivate func()
	mu         sync.Mutex
	conn       *dbus.Conn
	pending    map[uint32]struct{}
}

// New creates a notifier whose default action calls onActivate.
func New(onActivate func()) *Notifier {
	return &Notifier{onActivate: onActivate, pending: map[uint32]struct{}{}}
}

// Batch renders one sync pass's new inbox messages as (summary, body)
// pairs, per the contract: 1..=3 messages produce one notification each;
// more than 3 collapse into a single summary notification.
func Batch(accountEmail string, mail []mailsync.NewMail) [][2]string {
	if len(mail) == 0 {
		return nil
	}
	if len(mail) > 3 {
		return [][2]string{{fmt.Sprintf("%d new messages", len(mail)), "in " + accountEmail}}
	}
	out := make([][2]string, 0, len(mail))
	for _, m := range mail {
		out = append(out, [2]string{"New mail — " + m.FromName, m.Subject})
	}
	return out
}

// NotifyNewMail implements sync.Notifier.
func (n *Notifier) NotifyNewMail(accountEmail string, mail []mailsync.NewMail) {
	for _, item := range Batch(accountEmail, mail) {
		n.Show(item[0], item[1])
	}
}

// Test sends a sample notification (the test_notification command).
func (n *Notifier) Test() {
	n.Show("Cosmic Mail", "Notifications are working. Click to focus the window.")
}

func (n *Notifier) connect() (*dbus.Conn, error) {
	n.mu.Lock()
	defer n.mu.Unlock()
	if n.conn != nil {
		return n.conn, nil
	}
	conn, err := dbus.ConnectSessionBus()
	if err != nil {
		return nil, err
	}
	if err := conn.AddMatchSignal(dbus.WithMatchInterface(dbusInterface), dbus.WithMatchMember("ActionInvoked")); err != nil {
		conn.Close()
		return nil, err
	}
	if err := conn.AddMatchSignal(dbus.WithMatchInterface(dbusInterface), dbus.WithMatchMember("NotificationClosed")); err != nil {
		conn.Close()
		return nil, err
	}
	signals := make(chan *dbus.Signal, 16)
	conn.Signal(signals)
	go n.handleSignals(signals)
	n.conn = conn
	return conn, nil
}

func (n *Notifier) handleSignals(signals <-chan *dbus.Signal) {
	for sig := range signals {
		if len(sig.Body) < 1 {
			continue
		}
		id, ok := sig.Body[0].(uint32)
		if !ok {
			continue
		}
		n.mu.Lock()
		_, ours := n.pending[id]
		if ours {
			delete(n.pending, id)
		}
		n.mu.Unlock()
		if !ours {
			continue
		}
		switch sig.Name {
		case dbusInterface + ".ActionInvoked":
			slog.Debug("notification action received", "id", id)
			if n.onActivate != nil {
				n.onActivate()
			}
		case dbusInterface + ".NotificationClosed":
			slog.Debug("notification closed", "id", id)
		}
	}
}

// Show builds, shows, and wires up the default action for a notification.
func (n *Notifier) Show(summary, body string) {
	conn, err := n.connect()
	if err != nil {
		slog.Warn("failed to connect to the session bus for notifications", "error", err)
		return
	}
	hints := map[string]dbus.Variant{
		"desktop-entry": dbus.MakeVariant(DesktopEntry),
	}
	obj := conn.Object(dbusInterface, dbusPath)
	var id uint32
	err = obj.CallWithContext(context.Background(), dbusInterface+".Notify", 0,
		AppName, uint32(0), DesktopEntry, summary, body, []string{"default", "Open"}, hints, int32(-1)).Store(&id)
	if err != nil {
		slog.Warn("failed to show notification", "error", err)
		return
	}
	n.mu.Lock()
	n.pending[id] = struct{}{}
	n.mu.Unlock()
}
