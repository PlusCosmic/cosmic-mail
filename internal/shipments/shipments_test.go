package shipments

import (
	"strings"
	"testing"
)

func s(v string) *string { return &v }

func TestUPSChecksum(t *testing.T) {
	for _, ok := range []string{"1Z5R89390357567127", "1Z999AA10123456784", "1ZT3STSHIP12345679"} {
		if !upsChecksumValid(ok) {
			t.Errorf("%s should validate", ok)
		}
	}
	for _, bad := range []string{"1Z5R89390357567120", "1Z999AA10123456780", "1Z5R8939035756712", "1Z5R893903575671277"} {
		if upsChecksumValid(bad) {
			t.Errorf("%s should not validate", bad)
		}
	}
}

func TestS10AndUSPSChecksums(t *testing.T) {
	if !s10ChecksumValid("47312482", 9) || s10ChecksumValid("47312482", 0) {
		t.Fatal("s10")
	}
	if !uspsMod10Valid("12345678901234567890") || uspsMod10Valid("12345678901234567891") {
		t.Fatal("usps 20")
	}
	if !uspsMod10Valid("1234567890123456789017") || uspsMod10Valid("1234567890123456789010") || !uspsMod10Valid("9400000000000000000009") {
		t.Fatal("usps 22")
	}
	var valid []int
	for d := 0; d < 10; d++ {
		if upsChecksumValid("1Z999AA1012345678" + string(rune('0'+d))) {
			valid = append(valid, d)
		}
	}
	if len(valid) != 1 || valid[0] != 4 {
		t.Fatalf("ups unique: %v", valid)
	}
	valid = nil
	for d := 0; d < 10; d++ {
		if s10ChecksumValid("47312482", d) {
			valid = append(valid, d)
		}
	}
	if len(valid) != 1 || valid[0] != 9 {
		t.Fatalf("s10 unique: %v", valid)
	}
}

func TestDetectsUPSNumberAndLinkTogether(t *testing.T) {
	text := "Your package is on the way!\nTracking number: 1Z999AA10123456784\nTrack your shipment: https://www.ups.com/track?trackingNumber=1Z999AA10123456784"
	got := Extract(s(text), nil)
	if len(got) != 1 || got[0].Carrier != UPS || got[0].TrackingNumber == nil || *got[0].TrackingNumber != "1Z999AA10123456784" || got[0].TrackingURL == nil || !strings.Contains(*got[0].TrackingURL, "ups.com") {
		t.Fatalf("%+v", got)
	}
}

func TestUPSEmailWithTransactionReferenceYieldsOnlyTheUPSShipment(t *testing.T) {
	html := "<p>Your parcel delivery has been scheduled.</p><p>Tracking Number: 1Z17X3X96850653952</p>" +
		"<a href=\"https://www.ups.com/track?loc=en_US&tracknum=1Z17X3X96850653952&requester=ST/trackdetails\">Change delivery</a>" +
		"<table><tr><td>Transaction Reference Number:</td><td>4066509478</td></tr></table>" +
		"<p>Thank you for shipping with UPS. Your package delivery is on its way.</p>"
	got := Extract(nil, s(html))
	if len(got) != 1 || got[0].Carrier != UPS || *got[0].TrackingNumber != "1Z17X3X96850653952" {
		t.Fatalf("%+v", got)
	}
}

func TestUPSShapedNumberWithBadChecksumIsDropped(t *testing.T) {
	if got := Extract(s("Your UPS tracking number is 1Z999AA10123456780 — track your shipment now."), nil); len(got) != 0 {
		t.Fatalf("%+v", got)
	}
}

func TestDetectsFedExNumberWithContextAndLink(t *testing.T) {
	text := "Good news — your order has shipped via FedEx.\nTracking Number: 799340540586\nTrack it here: https://www.fedex.com/fedextrack/?trknbr=799340540586"
	got := Extract(s(text), nil)
	if len(got) != 1 || got[0].Carrier != FedEx || *got[0].TrackingNumber != "799340540586" || *got[0].TrackingURL != "https://www.fedex.com/fedextrack/?trknbr=799340540586" {
		t.Fatalf("%+v", got)
	}
}

func TestBareNumbersWithoutContextAreIgnored(t *testing.T) {
	if got := Extract(s("Your invoice reference is 799340540586. Please retain this for your records."), nil); len(got) != 0 {
		t.Fatalf("12-digit: %+v", got)
	}
	if got := Extract(s("Questions about your reservation? Call us at 5551234567 any time."), nil); len(got) != 0 {
		t.Fatalf("phone: %+v", got)
	}
}

func TestDetectsDHLNumberWithContext(t *testing.T) {
	got := Extract(s("Your parcel has been dispatched with DHL. Tracking: 1234567890."), nil)
	if len(got) != 1 || got[0].Carrier != DHL || *got[0].TrackingNumber != "1234567890" {
		t.Fatalf("%+v", got)
	}
}

func TestUSPS22DigitViaChecksum(t *testing.T) {
	got := Extract(s("Reference code for your records: 9400000000000000000009."), nil)
	if len(got) != 1 || got[0].Carrier != USPS || *got[0].TrackingNumber != "9400000000000000000009" {
		t.Fatalf("%+v", got)
	}
	if got := Extract(s("Reference code for your records: 1234567890123456789017."), nil); len(got) != 0 {
		t.Fatalf("non-9 prefix: %+v", got)
	}
}

func TestAmbiguous20DigitNeedsSpecificCarrierName(t *testing.T) {
	if got := Extract(s("Your shipment reference is 12345678901234567891."), nil); len(got) != 0 {
		t.Fatalf("%+v", got)
	}
	got := Extract(s("Your FedEx shipment reference is 12345678901234567891."), nil)
	if len(got) != 1 || got[0].Carrier != FedEx {
		t.Fatalf("%+v", got)
	}
}

func TestDetectsRoyalMailAndUSPSS10(t *testing.T) {
	html := "<p>Your item RR473124829GB has been dispatched.</p><a href=\"https://www.royalmail.com/track-your-item#/tracking-results/RR473124829GB\">Track</a>"
	got := Extract(nil, s(html))
	if len(got) != 1 || got[0].Carrier != RoyalMail || *got[0].TrackingNumber != "RR473124829GB" || !strings.Contains(*got[0].TrackingURL, "royalmail.com") {
		t.Fatalf("%+v", got)
	}
	got = Extract(s("Your international item EC123456706US is on its way."), nil)
	if len(got) != 1 || got[0].Carrier != USPS || *got[0].TrackingNumber != "EC123456706US" {
		t.Fatalf("%+v", got)
	}
}

func TestAmazon(t *testing.T) {
	html := "<p>Hi, your Amazon.com order #123-4567890-1234567 has shipped.</p><a href=\"https://www.amazon.com/progress-tracker/package/ref=abc\">Track package</a>"
	got := Extract(nil, s(html))
	if len(got) != 1 || got[0].Carrier != Amazon || got[0].TrackingNumber != nil || got[0].OrderID == nil || *got[0].OrderID != "123-4567890-1234567" || !strings.Contains(*got[0].TrackingURL, "progress-tracker") {
		t.Fatalf("%+v", got)
	}
	if got := Extract(s("Your reference number is 123-4567890-1234567 for this transaction."), nil); len(got) != 0 {
		t.Fatalf("no amazon mention: %+v", got)
	}
}

func TestLinkCarrierIsPreferredOverAmbiguousGuess(t *testing.T) {
	html := "<p>Your order has shipped. Reference: 123456789012345.</p><a href=\"https://www.dhl.com/us-en/home/tracking.html?tracking-id=123\">Track with DHL</a>"
	got := Extract(nil, s(html))
	found := false
	for _, sh := range got {
		if sh.Carrier == DHL && sh.TrackingNumber != nil && *sh.TrackingNumber == "123456789012345" {
			found = true
		}
	}
	if !found {
		t.Fatalf("%+v", got)
	}
}

func TestNoBodyYieldsNoShipments(t *testing.T) {
	if got := Extract(nil, nil); len(got) != 0 || got == nil {
		t.Fatal("nil/nil")
	}
	if got := Extract(s(""), s("")); len(got) != 0 {
		t.Fatal("empty")
	}
	if got := Extract(s("just a normal email with no shipping content at all"), nil); len(got) != 0 {
		t.Fatal("prose")
	}
}

func TestHelpers(t *testing.T) {
	hrefs := extractHrefs(`<a href="https://example.com/a">x</a><a href='https://example.com/b'>y</a>`)
	if len(hrefs) != 2 || hrefs[0] != "https://example.com/a" || hrefs[1] != "https://example.com/b" {
		t.Fatalf("%v", hrefs)
	}
	urls := extractBareURLs("See https://example.com/track?x=1, or https://example.com/y.")
	if len(urls) != 2 || urls[0] != "https://example.com/track?x=1" || urls[1] != "https://example.com/y" {
		t.Fatalf("%v", urls)
	}
	if got := stripHTMLTags("<p>Order &amp; shipment <b>update</b>&nbsp;here</p>"); got != "Order & shipment update here" {
		t.Fatal(got)
	}
	if c, ok := FromDB("royal_mail"); !ok || c != RoyalMail {
		t.Fatal("FromDB")
	}
	if _, ok := FromDB("pigeon"); ok {
		t.Fatal("FromDB unknown")
	}
}
