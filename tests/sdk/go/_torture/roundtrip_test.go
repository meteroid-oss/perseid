package torture

import (
	"bufio"
	"context"
	"encoding/json"
	"errors"
	"io"
	"net/http"
	"net/http/httptest"
	"reflect"
	"strings"
	"sync/atomic"
	"testing"
	"time"
)

func roundTrip[T any](t *testing.T, in string) string {
	t.Helper()
	var v T
	if err := json.Unmarshal([]byte(in), &v); err != nil {
		t.Fatalf("unmarshal %s: %v", in, err)
	}
	out, err := json.Marshal(v)
	if err != nil {
		t.Fatalf("marshal: %v", err)
	}
	return string(out)
}

func sameJSON(t *testing.T, got, want string) {
	t.Helper()
	var g, w any
	if err := json.Unmarshal([]byte(got), &g); err != nil {
		t.Fatalf("output is not JSON: %s", got)
	}
	if err := json.Unmarshal([]byte(want), &w); err != nil {
		t.Fatalf("want is not JSON: %s", want)
	}
	if !reflect.DeepEqual(g, w) {
		t.Errorf("got  %s\nwant %s", got, want)
	}
}

func TestRoundTrip(t *testing.T) {
	const thing = `{"id":"i","name":"n","created_at":"2024-01-02T03:04:05.123456789Z","kind":"alpha","count":9007199254740993,"nullable_required":null,"tags":[],"metadata":{},"attrs":{"x":[1,2]}`
	cases := []struct {
		name, in, want string
		rt             func(*testing.T, string) string
	}{
		{"int64 and nanoseconds", thing + `}`, "", roundTrip[Thing]},
		{"unknown enum", strings.Replace(thing, `"alpha"`, `"brand-new"`, 1) + `,"priority":99}`, "", roundTrip[Thing]},
		{"offset time", strings.Replace(thing, `.123456789Z`, `+02:00`, 1) + `}`, "", roundTrip[Thing]},
		// Response models do not tell an absent optional field from a null one.
		{"optional null", thing + `,"nullable_optional":null}`, thing + `}`, roundTrip[Thing]},
		{"union", `{"type":"circle","radius":1.5}`, "", roundTrip[Shape]},
		{"unknown union variant", `{"type":"triangle","a":1}`, "", roundTrip[Shape]},
		{"mapped union", `{"pet_type":"Cat","meow":true}`, "", roundTrip[Pet]},
		{"inline variant", `{"event":"deleted","reason":null}`, "", roundTrip[InlineEvent]},
		{"allOf", `{"id":"b1","created_at":"2024-01-02T03:04:05Z","extra":"e","sibling_prop":"s"}`, "", roundTrip[Composed]},
		{"recursion", `{"value":"root","children":[{"value":"c","children":[]}],"next":{"value":"n","children":[]}}`, "", roundTrip[TreeNode]},
		{"nested unions", `{"shape":{"type":"circle","radius":1},"shapes":[{"type":"square","side":1}],"shape_map":{"k":{"type":"square","side":3}},"inline_union":["a","b"],"empty":{},"free_form":{"any":1},"counts":{"a":9007199254740993}}`, "", roundTrip[UnionHolder]},
	}
	for _, tc := range cases {
		t.Run(tc.name, func(t *testing.T) {
			want := tc.want
			if want == "" {
				want = tc.in
			}
			sameJSON(t, tc.rt(t, tc.in), want)
		})
	}
}

func TestUnknownVariantIsDetectable(t *testing.T) {
	var shape Shape
	if err := json.Unmarshal([]byte(`{"type":"triangle","a":1}`), &shape); err != nil {
		t.Fatal(err)
	}
	if shape.IsKnown() || shape.Circle != nil || string(shape.Raw()) != `{"type":"triangle","a":1}` {
		t.Errorf("unknown variant: IsKnown=%v Raw=%s", shape.IsKnown(), shape.Raw())
	}
	if Kind("brand-new").IsKnown() || !KindAlpha.IsKnown() {
		t.Error("enum IsKnown")
	}
}

func TestPatchBodiesTellAbsentFromNull(t *testing.T) {
	for _, tc := range []struct {
		patch ThingPatch
		want  string
	}{
		{ThingPatch{}, `{}`},
		{ThingPatch{Description: ExplicitNull[string]()}, `{"description":null}`},
		{ThingPatch{Description: NewNullable("d"), Count: NewNullable(int64(2)), Name: Ptr("n")}, `{"count":2,"description":"d","name":"n"}`},
	} {
		got, err := json.Marshal(tc.patch)
		if err != nil {
			t.Fatal(err)
		}
		sameJSON(t, string(got), tc.want)
	}
}

func TestNullableStates(t *testing.T) {
	var absent *Nullable[string]
	if absent.IsNull() || !ExplicitNull[string]().IsNull() || NewNullable("").IsNull() {
		t.Error("IsNull is true only for an explicit null")
	}
	if v, ok := NewNullable("d").Get(); !ok || v != "d" {
		t.Error("NewNullable(\"d\").Get()")
	}
}

type recorded struct {
	requests atomic.Int32
	last     atomic.Pointer[http.Request]
}

func server(t *testing.T, handler func(n int32, w http.ResponseWriter, r *http.Request)) (*Client, *recorded) {
	t.Helper()
	rec := &recorded{}
	srv := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		n := rec.requests.Add(1)
		rec.last.Store(r.Clone(context.Background()))
		handler(n, w, r)
	}))
	t.Cleanup(srv.Close)
	return New("token", &Options{ServerURL: srv.URL, RetrySchedule: []time.Duration{time.Millisecond, time.Millisecond}}), rec
}

const thingJSON = `{"id":"t1","name":"n","created_at":"2024-01-02T03:04:05Z","kind":"alpha","count":1,"nullable_required":null,"tags":[],"metadata":{},"attrs":{}}`

func TestQueryParameters(t *testing.T) {
	client, rec := server(t, func(_ int32, w http.ResponseWriter, _ *http.Request) {
		io.WriteString(w, `{"data":[]}`)
	})
	since := time.Date(2024, 1, 2, 3, 4, 5, 123456789, time.FixedZone("x", 3600))
	_, err := client.Things().List(context.Background(), "r", &ThingsListOptions{
		IDs:   []string{"a", "b"},
		Since: &since,
	})
	if err != nil {
		t.Fatal(err)
	}
	query := rec.last.Load().URL.Query()
	if got := query.Get("since"); got != "2024-01-02T02:04:05.123456789Z" {
		t.Errorf("since = %q", got)
	}
	if got := query["ids"]; !reflect.DeepEqual(got, []string{"a", "b"}) {
		t.Errorf("ids = %v", got)
	}
}

func TestRequestOptions(t *testing.T) {
	client, rec := server(t, func(_ int32, w http.ResponseWriter, _ *http.Request) {
		io.WriteString(w, thingJSON)
	})
	thing, err := client.Things().Create(context.Background(), ThingCreate{Name: "n", Kind: KindAlpha}, nil,
		WithHeader("X-Extra", "1"), WithHeader("User-Agent", "mine"), WithIdempotencyKey("key-1"))
	if err != nil {
		t.Fatal(err)
	}
	if thing.ID != "t1" {
		t.Errorf("id = %q", thing.ID)
	}
	header := rec.last.Load().Header
	for name, want := range map[string]string{"X-Extra": "1", "User-Agent": "mine", "Idempotency-Key": "key-1", "Authorization": "Bearer token"} {
		if got := header.Get(name); got != want {
			t.Errorf("%s = %q, want %q", name, got, want)
		}
	}

	slow, _ := server(t, func(_ int32, w http.ResponseWriter, r *http.Request) {
		<-r.Context().Done()
	})
	_, err = slow.Things().Retrieve(context.Background(), "t1", WithTimeout(20*time.Millisecond), WithMaxRetries(0))
	var timeout *TimeoutError
	if !errors.Is(err, context.DeadlineExceeded) || !errors.As(err, &timeout) {
		t.Errorf("err = %v, want a deadline", err)
	}

	var transport *TransportError
	_, err = New("token", &Options{ServerURL: "http://127.0.0.1:1", MaxRetries: -1}).Things().Retrieve(context.Background(), "t1")
	if !errors.As(err, &transport) || errors.As(err, &timeout) {
		t.Errorf("err = %v, want a connection error", err)
	}
	var sdkErr SDKError
	if !errors.As(err, &sdkErr) {
		t.Errorf("err = %v, want an SDKError", err)
	}
}

func TestRetries(t *testing.T) {
	for _, tc := range []struct {
		status   int
		attempts int32
	}{
		{http.StatusTooManyRequests, 3},
		{http.StatusRequestTimeout, 3},
		{http.StatusBadGateway, 3},
		{http.StatusConflict, 1},
		{http.StatusBadRequest, 1},
	} {
		client, rec := server(t, func(_ int32, w http.ResponseWriter, _ *http.Request) {
			w.WriteHeader(tc.status)
		})
		_, err := client.Things().Retrieve(context.Background(), "t1")
		var apiErr *APIError
		if !errors.As(err, &apiErr) || apiErr.StatusCode != tc.status {
			t.Errorf("%d: err = %v", tc.status, err)
		}
		if got := rec.requests.Load(); got != tc.attempts {
			t.Errorf("%d: %d attempts, want %d", tc.status, got, tc.attempts)
		}
	}
}

func TestNonIdempotentRequestsAreOnlyRetriedOn429(t *testing.T) {
	for status, attempts := range map[int]int32{http.StatusBadGateway: 1, http.StatusRequestTimeout: 1, http.StatusTooManyRequests: 3} {
		client, rec := server(t, func(_ int32, w http.ResponseWriter, _ *http.Request) {
			w.Header().Set("X-Request-Id", "req_1")
			w.WriteHeader(status)
		})
		_, err := client.Things().Update(context.Background(), "t1", ThingPatch{})
		var apiErr *APIError
		if !errors.As(err, &apiErr) || apiErr.Header.Get("X-Request-Id") != "req_1" {
			t.Fatalf("%d: err = %v", status, err)
		}
		if got := rec.requests.Load(); got != attempts {
			t.Errorf("PATCH %d: %d attempts, want %d", status, got, attempts)
		}
	}
	client, rec := server(t, func(_ int32, w http.ResponseWriter, _ *http.Request) {
		w.WriteHeader(http.StatusBadGateway)
	})
	_, _ = client.Things().Update(context.Background(), "t1", ThingPatch{}, WithIdempotencyKey("k"))
	if got := rec.requests.Load(); got != 3 {
		t.Errorf("PATCH with a key: %d attempts, want 3", got)
	}
}

func TestRetryAfterOverridesTheSchedule(t *testing.T) {
	client, rec := server(t, func(n int32, w http.ResponseWriter, _ *http.Request) {
		if n == 1 {
			w.Header().Set("Retry-After", "0.05")
			w.WriteHeader(http.StatusTooManyRequests)
			return
		}
		io.WriteString(w, thingJSON)
	})
	client.cfg.retrySchedule = []time.Duration{time.Hour}
	start := time.Now()
	if _, err := client.Things().Retrieve(context.Background(), "t1"); err != nil {
		t.Fatal(err)
	}
	if elapsed := time.Since(start); elapsed < 50*time.Millisecond || elapsed > 10*time.Second {
		t.Errorf("waited %v", elapsed)
	}
	if rec.requests.Load() != 2 {
		t.Errorf("%d attempts", rec.requests.Load())
	}
}

func TestRetryAfterMsAndPerRequestRetries(t *testing.T) {
	client, rec := server(t, func(n int32, w http.ResponseWriter, _ *http.Request) {
		if n == 1 {
			w.Header().Set("Retry-After-Ms", "30")
			w.Header().Set("Retry-After", "3600")
			w.WriteHeader(http.StatusServiceUnavailable)
			return
		}
		io.WriteString(w, thingJSON)
	})
	client.cfg.retrySchedule = []time.Duration{time.Hour}
	start := time.Now()
	var resp *http.Response
	if _, err := client.Things().Retrieve(context.Background(), "t1", WithResponseInto(&resp)); err != nil {
		t.Fatal(err)
	}
	if elapsed := time.Since(start); elapsed < 30*time.Millisecond || elapsed > 10*time.Second {
		t.Errorf("waited %v", elapsed)
	}
	if resp.StatusCode != http.StatusOK || rec.requests.Load() != 2 {
		t.Errorf("status %d after %d attempts", resp.StatusCode, rec.requests.Load())
	}

	failing, rec := server(t, func(_ int32, w http.ResponseWriter, _ *http.Request) {
		w.WriteHeader(http.StatusBadGateway)
	})
	_, err := failing.Things().Retrieve(context.Background(), "t1", WithMaxRetries(0), WithResponseInto(&resp))
	if err == nil || rec.requests.Load() != 1 || resp.StatusCode != http.StatusBadGateway {
		t.Errorf("err %v after %d attempts", err, rec.requests.Load())
	}
}

func TestUnknownPropertiesRoundTrip(t *testing.T) {
	var thing Thing
	if err := json.Unmarshal([]byte(strings.Replace(thingJSON, `{`, `{"future":{"a":[1]},`, 1)), &thing); err != nil {
		t.Fatal(err)
	}
	if string(thing.ExtraFields["future"]) != `{"a":[1]}` {
		t.Fatalf("extra fields: %v", thing.ExtraFields)
	}
	out, err := json.Marshal(thing)
	if err != nil {
		t.Fatal(err)
	}
	sameJSON(t, string(out), strings.Replace(thingJSON, `{`, `{"future":{"a":[1]},`, 1))

	var cased Thing
	if err := json.Unmarshal([]byte(strings.Replace(thingJSON, `"id"`, `"ID"`, 1)), &cased); err != nil {
		t.Fatal(err)
	}
	if cased.ID != "t1" || cased.ExtraFields != nil {
		t.Errorf("a differently cased known property: ID %q, extra %v", cased.ID, cased.ExtraFields)
	}

	create := ThingCreate{Name: "n", Kind: KindAlpha, ExtraFields: map[string]json.RawMessage{"beta": []byte(`true`), "name": []byte(`"ignored"`)}}
	out, err = json.Marshal(create)
	if err != nil {
		t.Fatal(err)
	}
	var fields map[string]any
	if err := json.Unmarshal(out, &fields); err != nil || fields["beta"] != true || fields["name"] != "n" {
		t.Fatalf("%s %v", out, err)
	}
}

func TestRetryAfterParsing(t *testing.T) {
	now := time.Date(2024, 1, 2, 3, 4, 5, 0, time.UTC)
	for value, want := range map[string]time.Duration{
		"":                              0,
		"3":                             3 * time.Second,
		"-1":                            0,
		"soon":                          0,
		"Tue, 02 Jan 2024 03:04:15 GMT": 10 * time.Second,
		"Tue, 02 Jan 2024 03:04:00 GMT": 0,
	} {
		if got := parseRetryAfter(value, now); got != want {
			t.Errorf("parseRetryAfter(%q) = %v, want %v", value, got, want)
		}
	}
}

func TestDefaultBackoffIsJittered(t *testing.T) {
	call := newConfig("", nil).callConfig(nil)
	if len(call.retrySchedule) != DefaultMaxRetries || call.retrySchedule[0] != 500*time.Millisecond {
		t.Fatalf("schedule = %v", call.retrySchedule)
	}
	if n := len(newConfig("", &Options{MaxRetries: 4}).callConfig(nil).retrySchedule); n != 4 {
		t.Fatalf("MaxRetries: 4 gives %d retries", n)
	}
	for range 100 {
		if d := call.delay(0, 0); d < 375*time.Millisecond || d > 500*time.Millisecond {
			t.Fatalf("delay = %v", d)
		}
	}
	if d := call.delay(0, 2*time.Second); d != 2*time.Second {
		t.Errorf("Retry-After ignored: %v", d)
	}
	if d := call.delay(0, 2*time.Hour); d > 500*time.Millisecond {
		t.Errorf("Retry-After beyond a minute should fall back to the schedule: %v", d)
	}
}

func TestEventStreams(t *testing.T) {
	events := func(body string) *EventStream {
		return &EventStream{reader: bufio.NewReader(strings.NewReader(body)), status: http.StatusOK}
	}
	long := events("data: " + strings.Repeat("x", 2<<20) + "\n\n")
	var decodeErr *DecodeError
	if long.Next() || !errors.As(long.Err(), &decodeErr) {
		t.Fatalf("a line over 1 MiB: %v", long.Err())
	}

	stream := &Stream[map[string]int]{events: events("event: delta\ndata: {\"a\":1}\n\ndata: [DONE]\n\ndata: {\"b\":2}\n\n")}
	var got []map[string]int
	for value, err := range stream.All() {
		if err != nil {
			t.Fatal(err)
		}
		if stream.Event().Event != "delta" {
			t.Errorf("event %+v", stream.Event())
		}
		got = append(got, value)
	}
	if !reflect.DeepEqual(got, []map[string]int{{"a": 1}}) {
		t.Errorf("got %v", got)
	}
	broken := &Stream[map[string]int]{events: events("data: nope\n\n")}
	if broken.Next() || !errors.As(broken.Err(), &decodeErr) {
		t.Errorf("err %v", broken.Err())
	}
}

func TestBaseURL(t *testing.T) {
	client, rec := server(t, func(_ int32, w http.ResponseWriter, _ *http.Request) {
		_, _ = io.WriteString(w, thingJSON)
	})
	t.Setenv(BaseURLEnv, client.cfg.serverURL)
	if _, err := New("token", nil).Things().Retrieve(context.Background(), "t1"); err != nil || rec.requests.Load() != 1 {
		t.Fatalf("%s ignored: %v", BaseURLEnv, err)
	}

	t.Setenv(BaseURLEnv, "")
	unset := New("token", nil)
	unset.cfg.serverURL = ""
	_, err := unset.Things().Retrieve(context.Background(), "t1")
	var reqErr *RequestError
	if !errors.As(err, &reqErr) || !strings.Contains(err.Error(), "Options.ServerURL") || !strings.Contains(err.Error(), BaseURLEnv) {
		t.Fatalf("err = %v, want a RequestError naming both settings", err)
	}
	if rec.requests.Load() != 1 {
		t.Error("a request went out without a base URL")
	}
}
