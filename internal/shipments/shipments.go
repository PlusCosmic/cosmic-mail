// Package shipments extracts shipments / tracking numbers from cached
// message bodies.
//
// Pure, dependency-free heuristics — no network calls, no I/O. Given the
// parsed text/html body of a message, Extract returns zero or more detected
// shipments. A false positive means a bogus shipment card on an unrelated
// email, which is worse than missing a real one, so every heuristic is
// deliberately conservative:
//
//   - Formats with a published check digit (UPS `1Z`, and the UPU S10 postal
//     standard used by Royal Mail and USPS's 13-character international
//     format) are only kept when the checksum validates; a format-shaped
//     number with a failing checksum is dropped outright.
//   - Formats with no checksum (FedEx 12/15/20-digit, DHL 10-digit) are only
//     kept when a shipping keyword (or the carrier's own name) appears
//     within a small window of the match.
//   - A 20/22-digit numeric token is ambiguous between USPS and FedEx by
//     shape alone; it is only deterministic USPS when the Mod-10 checksum
//     validates *and* it starts with `9` (every real IMpb service prefix).
//   - Tracking links in the body disambiguate the carrier for free and are
//     preferred over a guessed carrier when both are present.
package shipments

import (
	"strings"
)

// Carrier is a carrier or marketplace a shipment was attributed to.
type Carrier string

const (
	UPS       Carrier = "ups"
	FedEx     Carrier = "fedex"
	USPS      Carrier = "usps"
	DHL       Carrier = "dhl"
	RoyalMail Carrier = "royal_mail"
	Amazon    Carrier = "amazon"
)

// FromDB parses a stored carrier code.
func FromDB(s string) (Carrier, bool) {
	switch Carrier(s) {
	case UPS, FedEx, USPS, DHL, RoyalMail, Amazon:
		return Carrier(s), true
	}
	return "", false
}

// fallbackURL is the public carrier tracking-page template, used when the
// email itself carried no tracking link. Amazon has no public template.
func (c Carrier) fallbackURL(trackingNumber string) *string {
	var u string
	switch c {
	case UPS:
		u = "https://www.ups.com/track?tracknum=" + trackingNumber
	case FedEx:
		u = "https://www.fedex.com/fedextrack/?trknbr=" + trackingNumber
	case USPS:
		u = "https://tools.usps.com/go/TrackConfirmAction?tLabels=" + trackingNumber
	case DHL:
		u = "https://www.dhl.com/global-en/home/tracking.html?tracking-id=" + trackingNumber
	case RoyalMail:
		u = "https://www.royalmail.com/track-your-item#/tracking-results/" + trackingNumber
	default:
		return nil
	}
	return &u
}

// Shipment is a shipment detected in a message body, ready to persist.
type Shipment struct {
	Carrier        Carrier
	TrackingNumber *string
	// Captured from the email when present, otherwise a synthesised carrier
	// tracking-page URL (never set for Amazon).
	TrackingURL *string
	OrderID     *string
}

func (s Shipment) equal(o Shipment) bool {
	return s.Carrier == o.Carrier && eqStr(s.TrackingNumber, o.TrackingNumber) && eqStr(s.TrackingURL, o.TrackingURL) && eqStr(s.OrderID, o.OrderID)
}

func eqStr(a, b *string) bool {
	if a == nil || b == nil {
		return a == nil && b == nil
	}
	return *a == *b
}

// contextWindow is how many characters of surrounding (lowercased) body
// text count as "nearby" when looking for shipping context.
const contextWindow = 120

var contextKeywords = []string{
	"tracking", "track your", "track my", "track this", "shipment", "shipped", "has shipped",
	"on its way", "out for delivery", "delivery", "dispatched", "courier", "parcel", "package",
	"consignment", "carrier",
}

var carrierKeywords = []struct {
	kw string
	c  Carrier
}{
	{"ups", UPS}, {"united parcel service", UPS},
	{"fedex", FedEx}, {"federal express", FedEx},
	{"usps", USPS}, {"postal service", USPS},
	{"dhl", DHL},
	{"royal mail", RoyalMail},
	{"amazon", Amazon},
}

type link struct {
	carrier Carrier
	url     string
}

// Extract detects shipments in a message's text and/or HTML body. Either
// part may be nil; both are scanned when present.
func Extract(text, html *string) []Shipment {
	var combined strings.Builder
	if text != nil {
		combined.WriteString(*text)
		combined.WriteByte('\n')
	}
	if html != nil {
		combined.WriteString(stripHTMLTags(*html))
		combined.WriteByte('\n')
	}
	all := combined.String()
	if strings.TrimSpace(all) == "" {
		return []Shipment{}
	}

	chars := []rune(all)
	lower := make([]rune, len(chars))
	for i, c := range chars {
		if c >= 'A' && c <= 'Z' {
			c += 32
		}
		lower[i] = c
	}
	lowerAll := string(lower)

	seenURLs := map[string]bool{}
	var links []link
	if html != nil {
		for _, u := range extractHrefs(*html) {
			if c, ok := carrierForURL(u); ok && !seenURLs[u] {
				seenURLs[u] = true
				links = append(links, link{c, u})
			}
		}
	}
	for _, u := range extractBareURLs(all) {
		if c, ok := carrierForURL(u); ok && !seenURLs[u] {
			seenURLs[u] = true
			links = append(links, link{c, u})
		}
	}

	numbers := findNumberCandidates(chars, lower)
	orderIDs := findAmazonOrderIDs(chars)
	mentionsAmazon := strings.Contains(lowerAll, "amazon")

	shipments := []Shipment{}
	used := make([]bool, len(numbers))
	push := func(s Shipment) {
		for _, x := range shipments {
			if x.equal(s) {
				return
			}
		}
		shipments = append(shipments, s)
	}
	captured := func(c Carrier) *string {
		for _, l := range links {
			if l.carrier == c {
				u := l.url
				return &u
			}
		}
		return nil
	}

	// 1. Checksum-verified numbers are certain; a format-shaped-but-invalid
	// match is dropped outright rather than falling back to a guess.
	for i, nm := range numbers {
		if !nm.deterministic {
			continue
		}
		used[i] = true
		if !nm.checksumValid {
			continue
		}
		n := nm.number
		push(Shipment{Carrier: nm.carrier, TrackingNumber: &n, TrackingURL: resolveTrackingURL(nm.carrier, n, captured(nm.carrier))})
	}

	// 2. Context-gated guesses — only when step 1 found nothing certain: a
	// real shipping email is saturated with generic keywords, so a second,
	// weaker number-shaped token is almost always something else.
	if len(shipments) == 0 {
		for i, nm := range numbers {
			if used[i] {
				continue
			}
			specific := specificCarrierKeywordNear(lower, nm.start, nm.end, nm.carrier)
			generic := genericKeywordNear(lower, nm.start, nm.end)
			if !specific && !generic {
				continue
			}
			carrier := nm.carrier
			if !specific && len(links) > 0 {
				carrier = links[0].carrier
			}
			n := nm.number
			push(Shipment{Carrier: carrier, TrackingNumber: &n, TrackingURL: resolveTrackingURL(carrier, n, captured(carrier))})
		}
	}

	// 3. Links with no attached number become their own number-less shipment.
	for _, l := range links {
		if l.carrier == Amazon {
			continue
		}
		dup := false
		for _, s := range shipments {
			if s.Carrier == l.carrier {
				dup = true
				break
			}
		}
		if dup {
			continue
		}
		u := l.url
		push(Shipment{Carrier: l.carrier, TrackingURL: &u})
	}

	// 4. Amazon: link and/or order id, gated on the message mentioning Amazon.
	if mentionsAmazon {
		amazonLink := captured(Amazon)
		var orderID *string
		if len(orderIDs) > 0 {
			id := orderIDs[0]
			orderID = &id
		}
		if amazonLink != nil || orderID != nil {
			push(Shipment{Carrier: Amazon, TrackingURL: amazonLink, OrderID: orderID})
		}
	}
	return shipments
}

func resolveTrackingURL(c Carrier, trackingNumber string, captured *string) *string {
	if captured != nil {
		return captured
	}
	return c.fallbackURL(trackingNumber)
}

// --- Number-format candidates -------------------------------------------------

type numberCandidate struct {
	carrier       Carrier
	number        string
	deterministic bool
	checksumValid bool
	start, end    int
}

func isAlnum(c rune) bool {
	return (c >= '0' && c <= '9') || (c >= 'a' && c <= 'z') || (c >= 'A' && c <= 'Z')
}

func isDigit(c rune) bool { return c >= '0' && c <= '9' }

func isAlpha(c rune) bool { return (c >= 'a' && c <= 'z') || (c >= 'A' && c <= 'Z') }

func findNumberCandidates(chars, lower []rune) []numberCandidate {
	var out []numberCandidate
	i := 0
	for i < len(chars) {
		if !isAlnum(chars[i]) {
			i++
			continue
		}
		start := i
		for i < len(chars) && isAlnum(chars[i]) {
			i++
		}
		if cand, ok := classifyToken(string(chars[start:i]), start, i, lower); ok {
			out = append(out, cand)
		}
	}
	return out
}

func classifyToken(token string, start, end int, lower []rune) (numberCandidate, bool) {
	n := len(token)
	upper := strings.ToUpper(token)

	// UPS: "1Z" + 16 alphanumeric (18 total), Mod-10 check digit.
	if n == 18 && strings.HasPrefix(upper, "1Z") {
		return numberCandidate{UPS, upper, true, upsChecksumValid(upper), start, end}, true
	}

	// S10 (UPU standard): 2 letters + 9 digits + 2-letter country code.
	if n == 13 {
		cs := []rune(upper)
		shapeOK := isAlpha(cs[0]) && isAlpha(cs[1]) && isAlpha(cs[11]) && isAlpha(cs[12])
		for _, c := range cs[2:11] {
			if !isDigit(c) {
				shapeOK = false
			}
		}
		if shapeOK {
			var carrier Carrier
			switch string(cs[11:13]) {
			case "GB":
				carrier = RoyalMail
			case "US":
				carrier = USPS
			default:
				return numberCandidate{}, false
			}
			valid := s10ChecksumValid(string(cs[2:10]), int(cs[10]-'0'))
			return numberCandidate{carrier, upper, true, valid, start, end}, true
		}
		return numberCandidate{}, false
	}

	allDigits := true
	for _, c := range token {
		if !isDigit(c) {
			allDigits = false
			break
		}
	}
	if !allDigits {
		return numberCandidate{}, false
	}
	switch n {
	case 10:
		return numberCandidate{DHL, token, false, false, start, end}, true
	case 12, 15:
		return numberCandidate{FedEx, token, false, false, start, end}, true
	case 20, 22:
		if uspsMod10Valid(token) && strings.HasPrefix(token, "9") {
			return numberCandidate{USPS, token, true, true, start, end}, true
		}
		near := windowText(lower, start, end, contextWindow)
		fedexNear := containsAny(near, []string{"fedex", "federal express"})
		uspsNear := containsAny(near, []string{"usps", "postal service"})
		switch {
		case fedexNear && !uspsNear:
			return numberCandidate{FedEx, token, false, false, start, end}, true
		case uspsNear && !fedexNear:
			return numberCandidate{USPS, token, false, false, start, end}, true
		}
	}
	return numberCandidate{}, false
}

// --- Context / keyword windows -----------------------------------------------

func windowText(lower []rune, start, end, radius int) string {
	s := start - radius
	if s < 0 {
		s = 0
	}
	e := end + radius
	if e > len(lower) {
		e = len(lower)
	}
	return string(lower[s:e])
}

func containsAny(haystack string, needles []string) bool {
	for _, n := range needles {
		if strings.Contains(haystack, n) {
			return true
		}
	}
	return false
}

func genericKeywordNear(lower []rune, start, end int) bool {
	return containsAny(windowText(lower, start, end, contextWindow), contextKeywords)
}

func specificCarrierKeywordNear(lower []rune, start, end int, carrier Carrier) bool {
	w := windowText(lower, start, end, contextWindow)
	for _, k := range carrierKeywords {
		if k.c == carrier && strings.Contains(w, k.kw) {
			return true
		}
	}
	return false
}

// --- Checksums -----------------------------------------------------------------

// upsChecksumValid implements the UPS `1Z` Mod-10 check digit: letters map
// to (ascii - 3) % 10, each of the 15 body characters is multiplied by 1
// (even index) or 2 (odd index) with no digit-sum reduction, and the check
// digit is (10 - sum % 10) % 10.
func upsChecksumValid(upper18 string) bool {
	chars := []rune(upper18)
	if len(chars) != 18 {
		return false
	}
	serial, check := chars[2:17], chars[17]
	if !isDigit(check) {
		return false
	}
	sum := 0
	for i, c := range serial {
		var value int
		switch {
		case isDigit(c):
			value = int(c - '0')
		case c >= 'A' && c <= 'Z':
			value = (int(c) - 3) % 10
		default:
			return false
		}
		if i%2 == 1 {
			value *= 2
		}
		sum += value
	}
	expected := (10 - sum%10) % 10
	return expected == int(check-'0')
}

// s10ChecksumValid implements the UPU S10 check digit: weights
// [8,6,4,2,3,5,9,7] over the 8 serial digits, sum % 11, check = 0 when the
// remainder is 1, 5 when it is 0, otherwise 11 - remainder.
func s10ChecksumValid(serial8 string, checkDigit int) bool {
	weights := [8]int{8, 6, 4, 2, 3, 5, 9, 7}
	digits := []rune(serial8)
	if len(digits) != 8 {
		return false
	}
	sum := 0
	for i, c := range digits {
		if !isDigit(c) {
			return false
		}
		sum += int(c-'0') * weights[i]
	}
	var expected int
	switch rem := sum % 11; rem {
	case 1:
		expected = 0
	case 0:
		expected = 5
	default:
		expected = 11 - rem
	}
	return expected == checkDigit
}

// uspsMod10Valid implements the USPS Mod-10 check digit: weights 3 (even
// index) / 1 (odd index) from the left, check = (10 - sum % 10) % 10.
func uspsMod10Valid(allDigits string) bool {
	chars := []rune(allDigits)
	if len(chars) < 2 {
		return false
	}
	serial, check := chars[:len(chars)-1], chars[len(chars)-1]
	if !isDigit(check) {
		return false
	}
	sum := 0
	for i, c := range serial {
		if !isDigit(c) {
			return false
		}
		d := int(c - '0')
		if i%2 == 0 {
			d *= 3
		}
		sum += d
	}
	expected := (10 - sum%10) % 10
	return expected == int(check-'0')
}

// --- HTML / link scanning -------------------------------------------------------

// stripHTMLTags recovers visible text for tokenising/keyword search. A
// removed tag becomes a space so adjacent inline content can't glue into one
// bogus token; whitespace is collapsed after entity decoding.
func stripHTMLTags(html string) string {
	var out strings.Builder
	out.Grow(len(html))
	inTag := false
	for _, c := range html {
		switch {
		case c == '<':
			inTag = true
		case c == '>':
			inTag = false
			out.WriteByte(' ')
		case !inTag:
			out.WriteRune(c)
		}
	}
	decoded := strings.NewReplacer("&amp;", "&", "&nbsp;", " ", "&lt;", "<", "&gt;", ">", "&quot;", "\"", "&#39;", "'").Replace(out.String())
	return strings.Join(strings.Fields(decoded), " ")
}

// extractHrefs extracts href="..." / href='...' values from raw HTML,
// preserving case.
func extractHrefs(html string) []string {
	hay := asciiLower(html)
	var out []string
	from := 0
	for {
		rel := strings.Index(hay[from:], "href=")
		if rel < 0 {
			break
		}
		pos := from + rel + 5
		if pos >= len(html) {
			break
		}
		quote := html[pos]
		if quote != '"' && quote != '\'' {
			from = pos
			continue
		}
		valStart := pos + 1
		relEnd := strings.IndexByte(html[valStart:], quote)
		if relEnd < 0 {
			break
		}
		out = append(out, html[valStart:valStart+relEnd])
		from = valStart + relEnd + 1
	}
	return out
}

func asciiLower(s string) string {
	b := []byte(s)
	for i, c := range b {
		if c >= 'A' && c <= 'Z' {
			b[i] = c + 32
		}
	}
	return string(b)
}

// extractBareURLs extracts bare http(s) URLs from plain text.
func extractBareURLs(text string) []string {
	lower := asciiLower(text)
	var out []string
	for _, scheme := range []string{"https://", "http://"} {
		from := 0
		for {
			rel := strings.Index(lower[from:], scheme)
			if rel < 0 {
				break
			}
			start := from + rel
			end := start
			for end < len(text) {
				b := text[end]
				if b == ' ' || b == '\t' || b == '\n' || b == '\r' || b == '\f' || b == '\v' || strings.IndexByte("\"'<>)]}", b) >= 0 {
					break
				}
				end++
			}
			trimmed := end
			for trimmed > start && strings.IndexByte(".,;:!", text[trimmed-1]) >= 0 {
				trimmed--
			}
			if trimmed > start {
				out = append(out, text[start:trimmed])
			}
			if end > start+1 {
				from = end
			} else {
				from = start + 1
			}
		}
	}
	return out
}

func carrierForURL(url string) (Carrier, bool) {
	l := asciiLower(url)
	track := strings.Contains(l, "track")
	switch {
	case strings.Contains(l, "ups.com") && track:
		return UPS, true
	case strings.Contains(l, "fedex.com") && track:
		return FedEx, true
	case strings.Contains(l, "usps.com") && track:
		return USPS, true
	case strings.Contains(l, "dhl.com") && track:
		return DHL, true
	case strings.Contains(l, "royalmail.com") && track:
		return RoyalMail, true
	case strings.Contains(l, "amazon.") && (track || strings.Contains(l, "progress-tracker") || strings.Contains(l, "ship-track")):
		return Amazon, true
	}
	return "", false
}

// findAmazonOrderIDs finds `\d{3}-\d{7}-\d{7}` occurrences not glued to
// other alphanumerics.
func findAmazonOrderIDs(chars []rune) []string {
	var out []string
	n := len(chars)
	for i := 0; i+19 <= n; {
		if isDigitRun(chars, i, 3) && chars[i+3] == '-' && isDigitRun(chars, i+4, 7) && chars[i+11] == '-' && isDigitRun(chars, i+12, 7) {
			beforeOK := i == 0 || !isAlnum(chars[i-1])
			afterOK := i+19 >= n || !isAlnum(chars[i+19])
			if beforeOK && afterOK {
				out = append(out, string(chars[i:i+19]))
				i += 19
				continue
			}
		}
		i++
	}
	return out
}

func isDigitRun(chars []rune, start, n int) bool {
	if start+n > len(chars) {
		return false
	}
	for _, c := range chars[start : start+n] {
		if !isDigit(c) {
			return false
		}
	}
	return true
}
