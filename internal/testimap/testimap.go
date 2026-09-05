// Package testimap runs a throwaway IMAPS server (go-imap's in-memory
// server) for hermetic tests and Docker-free end-to-end runs. It generates a
// self-signed certificate on start and hands the PEM to callers, which point
// the development build's COSMIC_MAIL_EXTRA_CA hook at it. Nothing here is
// compiled into the application.
package testimap

import (
	"bytes"
	"crypto/ecdsa"
	"crypto/elliptic"
	"crypto/rand"
	"crypto/tls"
	"crypto/x509"
	"crypto/x509/pkix"
	"encoding/pem"
	"fmt"
	"math/big"
	"net"
	"time"

	goimap "github.com/emersion/go-imap/v2"
	"github.com/emersion/go-imap/v2/imapserver"
	"github.com/emersion/go-imap/v2/imapserver/imapmemserver"
)

// Server is a running in-memory IMAPS server.
type Server struct {
	// Addr is the listening address (host:port).
	Addr string
	// CAPEM is the self-signed certificate, PEM-encoded, for the client's
	// trust store.
	CAPEM    []byte
	listener net.Listener
	server   *imapserver.Server
	mem      *imapmemserver.Server
	users    map[string]*imapmemserver.User
}

// Start serves IMAPS on addr ("127.0.0.1:0" for an ephemeral port) with one
// user per (username → password) entry. Each user gets INBOX plus Trash and
// Archive folders carrying SPECIAL-USE attributes.
func Start(addr string, users map[string]string) (*Server, error) {
	cert, caPEM, err := selfSigned()
	if err != nil {
		return nil, err
	}
	ln, err := tls.Listen("tcp", addr, &tls.Config{Certificates: []tls.Certificate{cert}, MinVersion: tls.VersionTLS12})
	if err != nil {
		return nil, fmt.Errorf("listening on %s: %w", addr, err)
	}
	mem := imapmemserver.New()
	s := &Server{Addr: ln.Addr().String(), CAPEM: caPEM, listener: ln, mem: mem, users: map[string]*imapmemserver.User{}}
	for name, password := range users {
		u := imapmemserver.NewUser(name, password)
		if err := u.Create("INBOX", nil); err != nil {
			return nil, err
		}
		if err := u.Create("Trash", &goimap.CreateOptions{SpecialUse: []goimap.MailboxAttr{goimap.MailboxAttrTrash}}); err != nil {
			return nil, err
		}
		if err := u.Create("Archive", &goimap.CreateOptions{SpecialUse: []goimap.MailboxAttr{goimap.MailboxAttrArchive}}); err != nil {
			return nil, err
		}
		mem.AddUser(u)
		s.users[name] = u
	}
	s.server = imapserver.New(&imapserver.Options{
		NewSession: func(*imapserver.Conn) (imapserver.Session, *imapserver.GreetingData, error) {
			return mem.NewSession(), nil, nil
		},
		Caps:         goimap.CapSet{goimap.CapIMAP4rev1: {}, goimap.CapIdle: {}, goimap.CapMove: {}, goimap.CapUIDPlus: {}, goimap.CapSpecialUse: {}},
		InsecureAuth: true,
	})
	go func() { _ = s.server.Serve(ln) }()
	return s, nil
}

// Close stops the server.
func (s *Server) Close() {
	_ = s.server.Close()
	_ = s.listener.Close()
}

// Port is the listening TCP port.
func (s *Server) Port() int { return s.listener.Addr().(*net.TCPAddr).Port }

type literal struct {
	*bytes.Reader
}

func (l literal) Size() int64 { return int64(l.Len()) }

// Seed appends a raw RFC 822 message to a user's mailbox.
func (s *Server) Seed(user, mailbox string, raw []byte) error {
	u, ok := s.users[user]
	if !ok {
		return fmt.Errorf("no such user %q", user)
	}
	_, err := u.Append(mailbox, literal{bytes.NewReader(raw)}, &goimap.AppendOptions{Time: time.Now()})
	return err
}

// selfSigned mints a certificate for localhost / 127.0.0.1.
func selfSigned() (tls.Certificate, []byte, error) {
	key, err := ecdsa.GenerateKey(elliptic.P256(), rand.Reader)
	if err != nil {
		return tls.Certificate{}, nil, err
	}
	template := &x509.Certificate{
		SerialNumber:          big.NewInt(time.Now().UnixNano()),
		Subject:               pkix.Name{CommonName: "localhost"},
		NotBefore:             time.Now().Add(-time.Hour),
		NotAfter:              time.Now().Add(24 * time.Hour),
		KeyUsage:              x509.KeyUsageDigitalSignature | x509.KeyUsageKeyEncipherment | x509.KeyUsageCertSign,
		ExtKeyUsage:           []x509.ExtKeyUsage{x509.ExtKeyUsageServerAuth},
		DNSNames:              []string{"localhost"},
		IPAddresses:           []net.IP{net.ParseIP("127.0.0.1")},
		IsCA:                  true,
		BasicConstraintsValid: true,
	}
	der, err := x509.CreateCertificate(rand.Reader, template, template, &key.PublicKey, key)
	if err != nil {
		return tls.Certificate{}, nil, err
	}
	certPEM := pem.EncodeToMemory(&pem.Block{Type: "CERTIFICATE", Bytes: der})
	keyDER, err := x509.MarshalECPrivateKey(key)
	if err != nil {
		return tls.Certificate{}, nil, err
	}
	keyPEM := pem.EncodeToMemory(&pem.Block{Type: "EC PRIVATE KEY", Bytes: keyDER})
	cert, err := tls.X509KeyPair(certPEM, keyPEM)
	if err != nil {
		return tls.Certificate{}, nil, err
	}
	return cert, certPEM, nil
}
