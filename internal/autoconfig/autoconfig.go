// Package autoconfig is Thunderbird-style mail settings autodiscovery.
//
// Given only an email address, Discover resolves IMAP/SMTP connection
// settings by trying, in order (first hit wins, ~12s total budget):
//
//  1. https://autoconfig.{domain}/mail/config-v1.1.xml?emailaddress={email}
//  2. https://{domain}/.well-known/autoconfig/mail/config-v1.1.xml
//  3. Thunderbird ISPDB: https://autoconfig.thunderbird.net/v1.1/{domain}
//  4. MX lookup → registrable domain of the MX host → that provider's own
//     autoconfig endpoint, then the ISPDB again
//  5. RFC 6186 SRV records (_imaps._tcp, _submission._tcp)
//  6. Heuristic guess imap.{domain}:993 / smtp.{domain}:587 (unconfident)
//
// Discovery never connects to the mail servers themselves; AddImapAccount
// still validates by connecting.
package autoconfig

import (
	"context"
	"errors"
	"fmt"
	"io"
	"net"
	"net/http"
	"net/url"
	"strconv"
	"strings"
	"time"

	"cosmicmail/internal/models"
	"cosmicmail/internal/xmldom"
)

const (
	httpTimeout  = 5 * time.Second
	totalTimeout = 12 * time.Second
	userAgent    = "CosmicMail/0.1.0 (autoconfig)"
)

// ErrInvalidEmail is returned for an address without exactly one `@` and
// non-empty local/domain parts.
var ErrInvalidEmail = errors.New("invalid email address")

// parsedConfig is a parsed server config before source/kind is decided.
type parsedConfig struct {
	imapHost string
	imapPort int
	smtpHost string
	smtpPort int
	username string
}

// splitEmail splits an email into (local, domain), lower-casing the domain.
func splitEmail(email string) (string, string, error) {
	email = strings.TrimSpace(email)
	local, domain, ok := strings.Cut(email, "@")
	if !ok || local == "" || domain == "" || strings.Contains(domain, "@") || strings.Contains(domain, " ") {
		return "", "", ErrInvalidEmail
	}
	return local, strings.ToLower(domain), nil
}

// isGmail detects whether an email/MX/host combination is Google-hosted.
func isGmail(domain string, imapHost, mxHost *string) bool {
	domain = strings.ToLower(domain)
	if domain == "gmail.com" || domain == "googlemail.com" {
		return true
	}
	if imapHost != nil && strings.EqualFold(*imapHost, "imap.gmail.com") {
		return true
	}
	if mxHost != nil {
		mx := strings.ToLower(strings.TrimSuffix(*mxHost, "."))
		if strings.HasSuffix(mx, ".google.com") || strings.HasSuffix(mx, ".googlemail.com") || mx == "google.com" {
			return true
		}
	}
	return false
}

// twoPartSuffixes is a tiny, best-effort list of two-label public suffixes.
var twoPartSuffixes = map[string]bool{
	"co.uk": true, "org.uk": true, "me.uk": true, "gov.uk": true, "ac.uk": true, "co.nz": true,
	"com.au": true, "net.au": true, "org.au": true, "co.za": true, "com.br": true, "co.jp": true,
	"co.in": true, "co.kr": true,
}

// registrableDomain returns the eTLD+1 of a host with a last-two-labels
// heuristic plus the small suffix list above.
func registrableDomain(host string) string {
	host = strings.ToLower(strings.TrimSuffix(host, "."))
	var labels []string
	for _, l := range strings.Split(host, ".") {
		if l != "" {
			labels = append(labels, l)
		}
	}
	if len(labels) <= 2 {
		return strings.Join(labels, ".")
	}
	lastTwo := labels[len(labels)-2] + "." + labels[len(labels)-1]
	if twoPartSuffixes[lastTwo] && len(labels) >= 3 {
		return strings.Join(labels[len(labels)-3:], ".")
	}
	return lastTwo
}

func resolvePlaceholders(value, email, localPart string) string {
	return strings.ReplaceAll(strings.ReplaceAll(value, "%EMAILADDRESS%", email), "%EMAILLOCALPART%", localPart)
}

func childText(n *xmldom.Node, tag string) string {
	c := n.Child(tag)
	if c == nil {
		return ""
	}
	return c.Text()
}

// parseAutoconfigXML parses a Thunderbird config-v1.1.xml document. It
// returns false unless there is an `incomingServer type="imap"` with
// `socketType SSL` (STARTTLS IMAP is unsupported by our connector). The
// SMTP server prefers a STARTTLS entry then an SSL entry.
func parseAutoconfigXML(xml, email string) (parsedConfig, bool) {
	localPart, domain, ok := strings.Cut(email, "@")
	if !ok || localPart == "" || domain == "" {
		return parsedConfig{}, false
	}
	if i := strings.LastIndex(email, "@"); i > 0 {
		localPart = email[:i]
	}
	doc, err := xmldom.Parse(xml)
	if err != nil {
		return parsedConfig{}, false
	}
	var imapHost, imapUser string
	var imapPort int
	found := false
	for _, node := range doc.Descendants("incomingServer") {
		if node.Attr("type") != "imap" || childText(node, "socketType") != "SSL" {
			continue
		}
		host := childText(node, "hostname")
		port, err := strconv.Atoi(childText(node, "port"))
		if host == "" || err != nil {
			return parsedConfig{}, false
		}
		user := childText(node, "username")
		if user == "" {
			user = "%EMAILADDRESS%"
		}
		imapHost, imapPort, imapUser, found = host, port, user, true
		break
	}
	if !found {
		return parsedConfig{}, false
	}
	var starttlsHost, sslHost string
	var starttlsPort, sslPort int
	for _, node := range doc.Descendants("outgoingServer") {
		if node.Attr("type") != "smtp" {
			continue
		}
		host := childText(node, "hostname")
		port, err := strconv.Atoi(childText(node, "port"))
		if host == "" || err != nil {
			continue
		}
		switch childText(node, "socketType") {
		case "STARTTLS":
			if starttlsHost == "" {
				starttlsHost, starttlsPort = host, port
			}
		case "SSL":
			if sslHost == "" {
				sslHost, sslPort = host, port
			}
		}
	}
	smtpHost, smtpPort := starttlsHost, starttlsPort
	if smtpHost == "" {
		smtpHost, smtpPort = sslHost, sslPort
	}
	if smtpHost == "" {
		return parsedConfig{}, false
	}
	return parsedConfig{
		imapHost: imapHost, imapPort: imapPort, smtpHost: smtpHost, smtpPort: smtpPort,
		username: resolvePlaceholders(imapUser, email, localPart),
	}, true
}

// --- network chain -----------------------------------------------------------

// Discover runs the full discovery chain. It errors only on an invalid
// address; otherwise it always returns a config, falling through to an
// unconfident guess.
func Discover(ctx context.Context, email string) (models.DiscoveredConfig, error) {
	localPart, domain, err := splitEmail(email)
	if err != nil {
		return models.DiscoveredConfig{}, err
	}
	email = strings.TrimSpace(email)
	ctx, cancel := context.WithTimeout(ctx, totalTimeout)
	defer cancel()
	if cfg, ok := runChain(ctx, email, localPart, domain); ok {
		return cfg, nil
	}
	return guess(domain, email), nil
}

func newClient() *http.Client {
	return &http.Client{Timeout: httpTimeout}
}

// Resolver is the DNS resolver used for MX/SRV lookups; swappable in tests.
var Resolver = net.DefaultResolver

func runChain(ctx context.Context, email, _ string, domain string) (models.DiscoveredConfig, bool) {
	client := newClient()

	// 1. Provider autoconfig subdomain.
	u := fmt.Sprintf("https://autoconfig.%s/mail/config-v1.1.xml?emailaddress=%s", domain, url.QueryEscape(email))
	if p, ok := fetchAndParse(ctx, client, u, email); ok {
		return finalize(p, domain, models.SourceAutoconfig, nil), true
	}
	// 2. .well-known.
	u = fmt.Sprintf("https://%s/.well-known/autoconfig/mail/config-v1.1.xml", domain)
	if p, ok := fetchAndParse(ctx, client, u, email); ok {
		return finalize(p, domain, models.SourceAutoconfig, nil), true
	}
	// 3. Thunderbird ISPDB.
	if p, ok := fetchISPDB(ctx, client, domain, email); ok {
		return finalize(p, domain, models.SourceIspdb, nil), true
	}
	// 4. MX → registrable domain → provider autoconfig, then ISPDB.
	if mxHost, ok := mxLookup(ctx, domain); ok {
		if isGmail(domain, nil, &mxHost) {
			return gmailFields(email), true
		}
		if reg := registrableDomain(mxHost); reg != "" && reg != domain {
			u = fmt.Sprintf("https://autoconfig.%s/mail/config-v1.1.xml?emailaddress=%s", reg, url.QueryEscape(email))
			if p, ok := fetchAndParse(ctx, client, u, email); ok {
				return finalize(p, domain, models.SourceMx, &mxHost), true
			}
			if p, ok := fetchISPDB(ctx, client, reg, email); ok {
				return finalize(p, domain, models.SourceMx, &mxHost), true
			}
		}
	}
	// 5. RFC 6186 SRV.
	if p, ok := srvLookup(ctx, domain, email); ok {
		return finalize(p, domain, models.SourceSrv, nil), true
	}
	return models.DiscoveredConfig{}, false
}

// fetchAndParse fetches an autoconfig XML URL and parses it. The parser is
// the gatekeeper: a 200 with an HTML catch-all page yields no config.
func fetchAndParse(ctx context.Context, client *http.Client, u, email string) (parsedConfig, bool) {
	if ctx.Err() != nil {
		return parsedConfig{}, false
	}
	req, err := http.NewRequestWithContext(ctx, http.MethodGet, u, nil)
	if err != nil {
		return parsedConfig{}, false
	}
	req.Header.Set("User-Agent", userAgent)
	resp, err := client.Do(req)
	if err != nil {
		return parsedConfig{}, false
	}
	defer resp.Body.Close()
	if resp.StatusCode < 200 || resp.StatusCode >= 300 {
		return parsedConfig{}, false
	}
	body, err := io.ReadAll(io.LimitReader(resp.Body, 1<<20))
	if err != nil {
		return parsedConfig{}, false
	}
	return parseAutoconfigXML(string(body), email)
}

func fetchISPDB(ctx context.Context, client *http.Client, domain, email string) (parsedConfig, bool) {
	return fetchAndParse(ctx, client, "https://autoconfig.thunderbird.net/v1.1/"+domain, email)
}

// mxLookup returns the lowest-preference MX host for a domain.
func mxLookup(ctx context.Context, domain string) (string, bool) {
	records, err := Resolver.LookupMX(ctx, domain)
	if err != nil {
		return "", false
	}
	best := ""
	bestPref := uint16(0)
	for _, mx := range records {
		host := strings.TrimSuffix(mx.Host, ".")
		if host == "" {
			continue
		}
		if best == "" || mx.Pref < bestPref {
			best, bestPref = host, mx.Pref
		}
	}
	return best, best != ""
}

// srvLookup does RFC 6186 SRV discovery for IMAPS + submission, ignoring
// "." targets ("service not offered") and picking the lowest priority.
func srvLookup(ctx context.Context, domain, email string) (parsedConfig, bool) {
	imapHost, imapPort, ok := srvPick(ctx, "imaps", domain)
	if !ok {
		return parsedConfig{}, false
	}
	smtpHost, smtpPort, ok := srvPick(ctx, "submission", domain)
	if !ok {
		return parsedConfig{}, false
	}
	return parsedConfig{imapHost: imapHost, imapPort: imapPort, smtpHost: smtpHost, smtpPort: smtpPort, username: email}, true
}

func srvPick(ctx context.Context, service, domain string) (string, int, bool) {
	_, records, err := Resolver.LookupSRV(ctx, service, "tcp", domain)
	if err != nil {
		return "", 0, false
	}
	var best *net.SRV
	for _, r := range records {
		target := strings.TrimSuffix(r.Target, ".")
		if target == "" {
			continue
		}
		if best == nil || r.Priority < best.Priority {
			best = r
		}
	}
	if best == nil {
		return "", 0, false
	}
	return strings.TrimSuffix(best.Target, "."), int(best.Port), true
}

func finalize(p parsedConfig, domain string, source models.DiscoverySource, mxHost *string) models.DiscoveredConfig {
	if isGmail(domain, &p.imapHost, mxHost) {
		g := gmailFields(p.username)
		g.Source = source
		return g
	}
	return models.DiscoveredConfig{
		Kind: models.AccountKindImap, ImapHost: p.imapHost, ImapPort: p.imapPort, SmtpHost: p.smtpHost, SmtpPort: p.smtpPort,
		Username: p.username, Source: source, Confident: true,
	}
}

func gmailFields(username string) models.DiscoveredConfig {
	return models.DiscoveredConfig{
		Kind: models.AccountKindGmail, ImapHost: "imap.gmail.com", ImapPort: 993, SmtpHost: "smtp.gmail.com", SmtpPort: 587,
		Username: username, Source: models.SourceMx, Confident: true,
	}
}

// guess is the heuristic fallback.
func guess(domain, email string) models.DiscoveredConfig {
	if isGmail(domain, nil, nil) {
		g := gmailFields(email)
		g.Source = models.SourceGuess
		return g
	}
	return models.DiscoveredConfig{
		Kind: models.AccountKindImap, ImapHost: "imap." + domain, ImapPort: 993, SmtpHost: "smtp." + domain, SmtpPort: 587,
		Username: email, Source: models.SourceGuess, Confident: false,
	}
}
