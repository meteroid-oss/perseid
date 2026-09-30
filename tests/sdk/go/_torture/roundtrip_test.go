package torture

import (
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
		{ThingPatch{Description: Null[string]()}, `{"description":null}`},
		{ThingPatch{Description: Set("d"), Count: Set(int64(2)), Name: Ptr("n")}, `{"count":2,"description":"d","name":"n"}`},
	} {
		got, err := json.Marshal(tc.patch)
		if err != nil {
			t.Fatal(err)
		}
		sameJSON(t, string(got), tc.want)
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
	_, err := client.Things().ListThings(context.Background(), ThingsListThingsOptions{
		IDs:       []string{"a", "b"},
		Since:     &since,
		XRequired: "r",
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
	thing, err := client.Things().CreateThing(context.Background(), ThingCreate{Name: "n", Kind: KindAlpha}, nil,
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
	_, err = slow.Things().GetThing(context.Background(), "t1", WithTimeout(20*time.Millisecond), WithMaxRetries(0))
	if !errors.Is(err, context.DeadlineExceeded) {
		t.Errorf("err = %v, want a deadline", err)
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
		_, err := client.Things().GetThing(context.Background(), "t1")
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
	for status, attempts := range map[int]int32{http.StatusBadGateway: 1, http.StatusTooManyRequests: 3} {
		client, rec := server(t, func(_ int32, w http.ResponseWriter, _ *http.Request) {
			w.Header().Set("X-Request-Id", "req_1")
			w.WriteHeader(status)
		})
		_, err := client.Things().UpdateThing(context.Background(), "t1", ThingPatch{})
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
	_, _ = client.Things().UpdateThing(context.Background(), "t1", ThingPatch{}, WithIdempotencyKey("k"))
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
	if _, err := client.Things().GetThing(context.Background(), "t1"); err != nil {
		t.Fatal(err)
	}
	if elapsed := time.Since(start); elapsed < 50*time.Millisecond || elapsed > 10*time.Second {
		t.Errorf("waited %v", elapsed)
	}
	if rec.requests.Load() != 2 {
		t.Errorf("%d attempts", rec.requests.Load())
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
	if len(call.retrySchedule) != DefaultNumRetries || call.retrySchedule[0] != 500*time.Millisecond {
		t.Fatalf("schedule = %v", call.retrySchedule)
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
