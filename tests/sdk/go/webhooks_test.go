package petstore_test

import (
	"errors"
	"net/http"
	"strconv"
	"testing"
	"time"

	"github.com/petstore/petstore-go"
)

const (
	webhookSecret  = "whsec_MfKQ9r8GKYqrTwjUPD8ILPZIo2LaLaSw"
	webhookPayload = `{"test": 2432232314}`
)

func webhookHeaders(prefix, signature string, timestamp time.Time) http.Header {
	h := http.Header{}
	h.Set(prefix+"-id", "msg_1")
	h.Set(prefix+"-signature", signature)
	h.Set(prefix+"-timestamp", strconv.FormatInt(timestamp.Unix(), 10))
	return h
}

func TestWebhookSignsLikeTheStandardTestVector(t *testing.T) {
	wh, err := petstore.NewWebhook(webhookSecret)
	if err != nil {
		t.Fatal(err)
	}
	got := wh.Sign("msg_p5jXN8AQM9LWM0D4loKWxJek", time.Unix(1614265330, 0), []byte(webhookPayload))
	if want := "v1,g0hM9SsE+OTPJTGt/tmIKtSyZlE3uFJELVlNIOLJ1OE="; got != want {
		t.Fatalf("got %s, want %s", got, want)
	}
}

func TestWebhookVerify(t *testing.T) {
	wh, _ := petstore.NewWebhook(webhookSecret)
	payload := []byte(webhookPayload)
	now := time.Now()
	good := wh.Sign("msg_1", now, payload)

	for _, prefix := range []string{"webhook", "svix"} {
		if err := wh.Verify(payload, webhookHeaders(prefix, good, now)); err != nil {
			t.Fatalf("%s headers: %v", prefix, err)
		}
	}

	mixed := webhookHeaders("svix", good, now)
	for name, values := range webhookHeaders("webhook", "v1,AAAA", now) {
		mixed[name] = values
	}
	if err := wh.Verify(payload, mixed); !errors.Is(err, petstore.ErrWebhookNoMatchingSignature) {
		t.Fatalf("webhook headers must win, got %v", err)
	}
	if err := wh.Verify(payload, webhookHeaders("webhook", "v1,AAAA v2,"+good+" "+good, now)); err != nil {
		t.Fatalf("rotated signatures: %v", err)
	}

	old, future := now.Add(-time.Hour), now.Add(time.Hour)
	cases := []struct {
		name    string
		payload []byte
		headers http.Header
		want    error
	}{
		{"tampered", []byte("tampered"), webhookHeaders("webhook", good, now), petstore.ErrWebhookNoMatchingSignature},
		{"missing", payload, http.Header{}, petstore.ErrWebhookMissingHeaders},
		{"old", payload, webhookHeaders("webhook", wh.Sign("msg_1", old, payload), old), petstore.ErrWebhookTimestampTooOld},
		{"future", payload, webhookHeaders("webhook", wh.Sign("msg_1", future, payload), future), petstore.ErrWebhookTimestampTooNew},
	}
	for _, c := range cases {
		if err := wh.Verify(c.payload, c.headers); !errors.Is(err, c.want) {
			t.Errorf("%s: got %v, want %v", c.name, err, c.want)
		}
	}
	bad := webhookHeaders("webhook", good, now)
	bad.Set("webhook-timestamp", "soon")
	if err := wh.Verify(payload, bad); !errors.Is(err, petstore.ErrWebhookInvalidTimestamp) {
		t.Errorf("invalid timestamp: got %v", err)
	}
}

func TestWebhookRejectsUnusableSecrets(t *testing.T) {
	for _, secret := range []string{"whsec_!!!", "whsec_", ""} {
		if _, err := petstore.NewWebhook(secret); err == nil {
			t.Errorf("secret %q must be rejected", secret)
		}
	}
}
