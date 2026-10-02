package torture

import (
	"context"
	"errors"
	"io"
	"net/http"
	"strconv"
	"strings"
	"sync"
	"sync/atomic"
	"testing"
	"time"
)

// Request building, retries and error handling against the SDK generated from
// tests/fixtures/torture.yaml, with scripted attempts instead of a network.

// step is what one attempt receives.
type step struct {
	status int
	header map[string]string
	body   string
	// fail makes the attempt fail before any response, as a broken connection does.
	fail error
	// hang makes the attempt wait for its context, as an unresponsive server does.
	hang bool
}

func reply(status int, body string) step { return step{status: status, body: body} }

func replyWith(status int, body string, header map[string]string) step {
	return step{status: status, body: body, header: header}
}

// script plays one step per attempt (the last one repeats) and records every request it gets.
type script struct {
	mu    sync.Mutex
	steps []step
	seen  []*http.Request
}

func newScript(steps ...step) *script { return &script{steps: steps} }

func (s *script) RoundTrip(req *http.Request) (*http.Response, error) {
	s.mu.Lock()
	s.seen = append(s.seen, req.Clone(req.Context()))
	current := s.steps[min(len(s.seen), len(s.steps))-1]
	s.mu.Unlock()
	switch {
	case current.hang:
		<-req.Context().Done()
		return nil, req.Context().Err()
	case current.fail != nil:
		return nil, current.fail
	}
	header := http.Header{}
	for name, value := range current.header {
		header.Set(name, value)
	}
	return &http.Response{
		StatusCode: current.status,
		Header:     header,
		Body:       io.NopCloser(strings.NewReader(current.body)),
		Request:    req,
	}, nil
}

func (s *script) attempts() int {
	s.mu.Lock()
	defer s.mu.Unlock()
	return len(s.seen)
}

func (s *script) request(attempt int) *http.Request {
	s.mu.Lock()
	defer s.mu.Unlock()
	return s.seen[attempt]
}

// client is a client whose transport is the script, behind the middleware of options.
func (s *script) client(options Options) *Client {
	options.Middleware = append(options.Middleware, func(http.RoundTripper) http.RoundTripper { return s })
	if options.ServerURL == "" {
		options.ServerURL = "https://torture.example.com"
	}
	return New("token", &options)
}

// retries is an Options that retries n times without waiting.
func retries(n int) Options {
	return Options{RetrySchedule: make([]time.Duration, n)}
}

func TestPathParametersArePercentEncoded(t *testing.T) {
	for _, tc := range []struct{ value, encoded string }{
		{"plain", "plain"},
		{"sp ace", "sp%20ace"},
		{"sl/ash", "sl%2Fash"},
		{"q?mark=1", "q%3Fmark=1"},
		{"per%cent", "per%25cent"},
		{"lit%25eral", "lit%2525eral"},
		{"ha#sh", "ha%23sh"},
		{"a+b", "a+b"},
		{"h\u00e9llo w\u00f6rld \u2713", "h%C3%A9llo%20w%C3%B6rld%20%E2%9C%93"},
		{"\U0001F389", "%F0%9F%8E%89"},
		{"a/../b", "a%2F..%2Fb"},
		{"..%2F..", "..%252F.."},
	} {
		sc := newScript(reply(200, thingJSON))
		if _, err := sc.client(retries(0)).Things().Retrieve(context.Background(), tc.value); err != nil {
			t.Fatalf("%q: %v", tc.value, err)
		}
		got := sc.request(0).URL
		if got.EscapedPath() != "/things/"+tc.encoded || got.RawQuery != "" || got.Fragment != "" {
			t.Errorf("%q was sent as %s", tc.value, got)
		}
	}
}

func TestDotSegmentsNeverReachTheServer(t *testing.T) {
	for _, dots := range []string{"..", "."} {
		sc := newScript(reply(200, thingJSON))
		_, err := sc.client(retries(0)).Things().Retrieve(context.Background(), dots)
		var reqErr *RequestError
		if !errors.As(err, &reqErr) {
			t.Errorf("%q: err = %v, want a RequestError", dots, err)
		}
		if sc.attempts() != 0 {
			t.Errorf("%q: a rejected path was sent", dots)
		}
	}
}

func TestTheBaseURLPathPrefixIsKept(t *testing.T) {
	for _, base := range []string{"https://torture.example.com/api/v2", "https://torture.example.com/api/v2/"} {
		sc := newScript(reply(200, thingJSON))
		options := retries(0)
		options.ServerURL = base
		if _, err := sc.client(options).Things().Retrieve(context.Background(), "t1"); err != nil {
			t.Fatal(err)
		}
		if got := sc.request(0).URL.String(); got != "https://torture.example.com/api/v2/things/t1" {
			t.Errorf("%s: sent %s", base, got)
		}
	}
}

func TestTheRequiredHeaderIsSentAndTheOptionalOneOnlyWhenGiven(t *testing.T) {
	sc := newScript(reply(200, `{"data":[]}`))
	client := sc.client(retries(0))
	if _, err := client.Things().List(context.Background(), "req-1", nil); err != nil {
		t.Fatal(err)
	}
	first := sc.request(0).Header
	if got := first.Get("X-Required"); got != "req-1" {
		t.Errorf("X-Required = %q", got)
	}
	if _, sent := first["X-Trace-Id"]; sent {
		t.Error("X-Trace-Id was sent without a value")
	}
	if _, err := client.Things().List(context.Background(), "req-2", &ThingsListOptions{XTraceID: Ptr("trace-1")}); err != nil {
		t.Fatal(err)
	}
	second := sc.request(1).Header
	if second.Get("X-Required") != "req-2" || second.Get("X-Trace-Id") != "trace-1" {
		t.Errorf("headers = %v", second)
	}
}

func TestErrorBodiesNeedNotBeJSON(t *testing.T) {
	for _, tc := range []struct {
		status      int
		contentType string
		body        string
		sentinel    error
	}{
		{500, "text/plain", "upstream exploded", ErrServer},
		{502, "text/html", "<html><h1>Bad Gateway</h1></html>", ErrServer},
		{404, "application/json", "", ErrNotFound},
		{400, "application/json", `{"truncated": `, ErrBadRequest},
		{418, "text/plain", "teapot", nil},
	} {
		sc := newScript(replyWith(tc.status, tc.body, map[string]string{"Content-Type": tc.contentType, "X-Request-Id": "req_7"}))
		_, err := sc.client(retries(0)).Things().Retrieve(context.Background(), "t")
		var apiErr *APIError
		if !errors.As(err, &apiErr) || apiErr.StatusCode != tc.status {
			t.Fatalf("%d: err = %v", tc.status, err)
		}
		if tc.sentinel != nil && !errors.Is(err, tc.sentinel) {
			t.Errorf("%d: err does not match %v", tc.status, tc.sentinel)
		}
		if tc.sentinel == nil && (errors.Is(err, ErrServer) || errors.Is(err, ErrNotFound) || errors.Is(err, ErrBadRequest)) {
			t.Errorf("%d: err matches a sentinel of another status", tc.status)
		}
		if string(apiErr.RawBody) != tc.body {
			t.Errorf("%d: raw body %q", tc.status, apiErr.RawBody)
		}
		if apiErr.Body != nil {
			t.Errorf("%d: %q is not JSON, but Body = %#v", tc.status, tc.body, apiErr.Body)
		}
		if body, ok := ErrorBody[Problem](err); ok {
			t.Errorf("%d: %q decoded as %+v", tc.status, tc.body, body)
		}
		if apiErr.RequestID() != "req_7" {
			t.Errorf("%d: request id %q", tc.status, apiErr.RequestID())
		}
		if message := err.Error(); !strings.Contains(message, strconv.Itoa(tc.status)) || !strings.Contains(message, tc.body) {
			t.Errorf("%d: message %q", tc.status, message)
		}
		if sc.attempts() != 1 {
			t.Errorf("%d: %d attempts", tc.status, sc.attempts())
		}
	}

	alt := newScript(replyWith(404, "", map[string]string{"Request-Id": "alt_1"}))
	_, err := alt.client(retries(0)).Things().Retrieve(context.Background(), "t")
	var apiErr *APIError
	if !errors.As(err, &apiErr) || apiErr.RequestID() != "alt_1" {
		t.Errorf("Request-Id: %v", err)
	}
	none := newScript(reply(404, ""))
	_, err = none.client(retries(0)).Things().Retrieve(context.Background(), "t")
	if !errors.As(err, &apiErr) || apiErr.RequestID() != "" {
		t.Errorf("no request id: %v", err)
	}
}

func TestUnauthenticatedCallsRaiseTheTypedAuthenticationError(t *testing.T) {
	sc := newScript(replyWith(401, `{"title":"no credentials"}`, map[string]string{"X-Request-Id": "req_9"}))
	_, err := sc.client(retries(2)).Things().Retrieve(context.Background(), "t")
	if !errors.Is(err, ErrUnauthorized) || errors.Is(err, ErrForbidden) {
		t.Fatalf("err = %v", err)
	}
	problem, ok := ErrorBody[Problem](err)
	if !ok || problem.Title != "no credentials" {
		t.Errorf("body = %+v", problem)
	}
	if sc.attempts() != 1 {
		t.Errorf("a 401 is not retried: %d attempts", sc.attempts())
	}
}

func TestTimeoutsAreRetried(t *testing.T) {
	sc := newScript(step{hang: true}, reply(200, thingJSON))
	options := retries(1)
	options.Timeout = 50 * time.Millisecond
	thing, err := sc.client(options).Things().Retrieve(context.Background(), "t")
	if err != nil || thing.ID != "t1" || sc.attempts() != 2 {
		t.Fatalf("%+v %v after %d attempts", thing, err, sc.attempts())
	}

	hung := newScript(step{hang: true})
	options.Timeout = 20 * time.Millisecond
	_, err = hung.client(options).Things().Retrieve(context.Background(), "t")
	var timeout *TimeoutError
	if !errors.As(err, &timeout) || !errors.Is(err, context.DeadlineExceeded) {
		t.Errorf("err = %v, want a timeout", err)
	}
	if hung.attempts() != 2 {
		t.Errorf("%d attempts, want the first and one retry", hung.attempts())
	}
}

func TestConnectionFailuresAreRetried(t *testing.T) {
	broken := errors.New("connection reset by peer")
	sc := newScript(step{fail: broken}, reply(200, thingJSON))
	thing, err := sc.client(retries(1)).Things().Retrieve(context.Background(), "t")
	if err != nil || thing.ID != "t1" || sc.attempts() != 2 {
		t.Fatalf("%+v %v after %d attempts", thing, err, sc.attempts())
	}

	down := newScript(step{fail: broken})
	_, err = down.client(retries(1)).Things().Retrieve(context.Background(), "t")
	var transport *TransportError
	var timeout *TimeoutError
	if !errors.As(err, &transport) || errors.As(err, &timeout) || !errors.Is(err, broken) {
		t.Errorf("err = %v, want a connection error", err)
	}
	if !strings.Contains(err.Error(), "connection reset by peer") || down.attempts() != 2 {
		t.Errorf("%v after %d attempts", err, down.attempts())
	}
}

func TestMiddlewareRunsOncePerAttempt(t *testing.T) {
	var count atomic.Int32
	counter := func(next http.RoundTripper) http.RoundTripper {
		return RoundTripperFunc(func(req *http.Request) (*http.Response, error) {
			count.Add(1)
			return next.RoundTrip(req)
		})
	}
	sc := newScript(
		replyWith(503, "busy", map[string]string{"Retry-After-Ms": "0"}),
		replyWith(429, "slow down", map[string]string{"Retry-After-Ms": "0"}),
		reply(200, thingJSON),
	)
	options := retries(2)
	options.Middleware = []Middleware{counter}
	if _, err := sc.client(options).Things().Retrieve(context.Background(), "t"); err != nil {
		t.Fatal(err)
	}
	if sc.attempts() != 3 || count.Load() != 3 {
		t.Errorf("%d attempts, the middleware ran %d times: it sits inside the retry loop", sc.attempts(), count.Load())
	}
	if got := sc.request(2).Header.Get("Torture-Retry-Count"); got != "2" {
		t.Errorf("retry count header = %q", got)
	}
	if got := sc.request(0).Header.Get("Torture-Retry-Count"); got != "" {
		t.Errorf("the first attempt carries a retry count: %q", got)
	}
}

func TestAutomaticIdempotencyKeysSurviveRetries(t *testing.T) {
	sc := newScript(replyWith(503, "busy", nil), reply(200, thingJSON), reply(200, thingJSON))
	client := sc.client(retries(2))
	create := ThingCreate{Name: "n", Kind: KindAlpha}
	if _, err := client.Things().Create(context.Background(), create, nil); err != nil {
		t.Fatal(err)
	}
	if sc.attempts() != 2 {
		t.Fatalf("a POST with a key is retried: %d attempts", sc.attempts())
	}
	key := sc.request(0).Header.Get("Idempotency-Key")
	if !strings.HasPrefix(key, "auto_") || sc.request(1).Header.Get("Idempotency-Key") != key {
		t.Errorf("keys %q and %q", key, sc.request(1).Header.Get("Idempotency-Key"))
	}

	// Another call draws another key.
	if _, err := client.Things().Create(context.Background(), create, nil); err != nil {
		t.Fatal(err)
	}
	if other := sc.request(2).Header.Get("Idempotency-Key"); other == key || !strings.HasPrefix(other, "auto_") {
		t.Errorf("second call key %q, first %q", other, key)
	}

	// A key of the caller is kept on every attempt.
	mine := newScript(replyWith(503, "busy", nil), reply(200, thingJSON))
	if _, err := mine.client(retries(2)).Things().Create(context.Background(), create, &ThingsCreateOptions{IdempotencyKey: Ptr("mine")}); err != nil {
		t.Fatal(err)
	}
	for attempt := range 2 {
		if got := mine.request(attempt).Header.Get("Idempotency-Key"); got != "mine" {
			t.Errorf("attempt %d key %q", attempt, got)
		}
	}
}

func TestCancellingACallStopsItsRetries(t *testing.T) {
	// During the wait before a retry.
	sc := newScript(replyWith(503, "busy", map[string]string{"Retry-After-Ms": "800"}), reply(200, thingJSON))
	ctx, cancel := context.WithCancel(context.Background())
	time.AfterFunc(100*time.Millisecond, cancel)
	start := time.Now()
	_, err := sc.client(retries(2)).Things().Retrieve(ctx, "t")
	if !errors.Is(err, context.Canceled) {
		t.Errorf("err = %v, want the cancellation", err)
	}
	if elapsed := time.Since(start); elapsed > 600*time.Millisecond {
		t.Errorf("waited %v for the retry", elapsed)
	}
	if sc.attempts() != 1 {
		t.Errorf("%d attempts: nothing is sent once the call is canceled", sc.attempts())
	}

	// During an attempt, which is not a timeout; the client stays usable.
	hung := newScript(step{hang: true}, reply(200, thingJSON))
	client := hung.client(retries(2))
	ctx, cancel = context.WithCancel(context.Background())
	time.AfterFunc(50*time.Millisecond, cancel)
	_, err = client.Things().Retrieve(ctx, "t")
	var timeout *TimeoutError
	if !errors.Is(err, context.Canceled) || errors.As(err, &timeout) || hung.attempts() != 1 {
		t.Errorf("err = %v after %d attempts", err, hung.attempts())
	}
	if thing, err := client.Things().Retrieve(context.Background(), "t"); err != nil || thing.ID != "t1" {
		t.Errorf("%+v %v", thing, err)
	}

	// Before the call: the attempt fails at once and is not retried.
	pre := newScript(step{hang: true})
	ctx, cancel = context.WithCancel(context.Background())
	cancel()
	if _, err = pre.client(retries(2)).Things().Retrieve(ctx, "t"); !errors.Is(err, context.Canceled) || pre.attempts() > 1 {
		t.Errorf("err = %v after %d attempts", err, pre.attempts())
	}
}

func TestUnknownResponsePropertiesAreTolerated(t *testing.T) {
	body := strings.Replace(thingJSON, `{`, `{"future":{"a":[1,{"b":null}]},"flag":true,`, 1)
	sc := newScript(reply(200, body))
	thing, err := sc.client(retries(0)).Things().Retrieve(context.Background(), "t")
	if err != nil {
		t.Fatal(err)
	}
	if thing.ID != "t1" || string(thing.ExtraFields["future"]) != `{"a":[1,{"b":null}]}` || string(thing.ExtraFields["flag"]) != "true" {
		t.Errorf("%+v", thing)
	}
}

func TestASuccessBodyThatDoesNotDecodeIsADecodeError(t *testing.T) {
	for _, body := range []string{`{"id": 1}`, "", "not json", `[]`, `"text"`, `{"id": "t1"`} {
		sc := newScript(reply(200, body))
		thing, err := sc.client(retries(2)).Things().Retrieve(context.Background(), "t")
		var decodeErr *DecodeError
		if !errors.As(err, &decodeErr) || thing != nil {
			t.Errorf("%q: %+v, %v", body, thing, err)
			continue
		}
		var apiErr *APIError
		if errors.As(err, &apiErr) || decodeErr.StatusCode != 200 || string(decodeErr.RawBody) != body {
			t.Errorf("%q: %+v", body, decodeErr)
		}
		if sc.attempts() != 1 {
			t.Errorf("%q: a decode error is not retried, %d attempts", body, sc.attempts())
		}
	}

	// A 201 with garbage fails the same way, a 204 has nothing to decode.
	created := newScript(reply(201, "garbage"))
	if _, err := created.client(retries(0)).Things().Retrieve(context.Background(), "t"); !errors.As(err, new(*DecodeError)) {
		t.Errorf("201: %v", err)
	}
	deleted := newScript(reply(204, ""))
	if err := deleted.client(retries(0)).Things().Delete(context.Background(), "t"); err != nil {
		t.Errorf("204: %v", err)
	}
}
