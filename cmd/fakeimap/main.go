// fakeimap serves the e2e fixture mailbox over IMAPS without Docker: go-imap's
// in-memory server with the `test` user, seeded from e2e/fixtures/mail, on
// 127.0.0.1:3993 like the GreenMail fixture. It writes its self-signed
// certificate to the -ca-out path for COSMIC_MAIL_EXTRA_CA. Development
// tooling only; not part of the application.
//
//	go run ./cmd/fakeimap -ca-out /tmp/fakeimap-ca.pem
package main

import (
	"flag"
	"fmt"
	"os"
	"os/signal"
	"path/filepath"
	"syscall"

	"cosmicmail/internal/testimap"
)

func main() {
	addr := flag.String("addr", "127.0.0.1:3993", "IMAPS listen address")
	caOut := flag.String("ca-out", "", "write the server certificate (PEM) here")
	mailDir := flag.String("mail", filepath.Join("e2e", "fixtures", "mail"), "directory of .eml fixtures to seed into INBOX")
	user := flag.String("user", "test", "IMAP username")
	password := flag.String("password", "test-pass", "IMAP password")
	flag.Parse()

	server, err := testimap.Start(*addr, map[string]string{*user: *password})
	if err != nil {
		fmt.Fprintln(os.Stderr, err)
		os.Exit(1)
	}
	defer server.Close()
	if *caOut != "" {
		if err := os.WriteFile(*caOut, server.CAPEM, 0o644); err != nil {
			fmt.Fprintln(os.Stderr, err)
			os.Exit(1)
		}
	}
	files, _ := filepath.Glob(filepath.Join(*mailDir, "*.eml"))
	for _, f := range files {
		raw, err := os.ReadFile(f)
		if err != nil {
			fmt.Fprintln(os.Stderr, err)
			os.Exit(1)
		}
		if err := server.Seed(*user, "INBOX", raw); err != nil {
			fmt.Fprintln(os.Stderr, err)
			os.Exit(1)
		}
	}
	fmt.Printf("fakeimap: IMAPS on %s, %d fixture messages seeded for %s\n", server.Addr, len(files), *user)
	sig := make(chan os.Signal, 1)
	signal.Notify(sig, os.Interrupt, syscall.SIGTERM)
	<-sig
}
