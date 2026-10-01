package petstore_test

import (
	"context"
	"io"
	"net/http"
	"strings"
	"sync"
	"testing"

	"github.com/petstore/petstore-go"
)

const petJSON = `{"id":"1","name":"Rex","created_at":"2024-01-01T00:00:00Z"}`

func respond(req *http.Request, body string) *http.Response {
	return &http.Response{
		StatusCode: http.StatusOK,
		Header:     http.Header{"Content-Type": {"application/json"}},
		Body:       io.NopCloser(strings.NewReader(body)),
		Request:    req,
	}
}

func origin(seen *[]*http.Request) petstore.Middleware {
	return func(http.RoundTripper) http.RoundTripper {
		return petstore.RoundTripperFunc(func(req *http.Request) (*http.Response, error) {
			*seen = append(*seen, req)
			return respond(req, petJSON), nil
		})
	}
}

func cache() petstore.Middleware {
	var mu sync.Mutex
	store := map[string]string{}
	return func(next http.RoundTripper) http.RoundTripper {
		return petstore.RoundTripperFunc(func(req *http.Request) (*http.Response, error) {
			if req.Method != http.MethodGet {
				return next.RoundTrip(req)
			}
			mu.Lock()
			body, ok := store[req.URL.String()]
			mu.Unlock()
			if ok {
				return respond(req, body), nil
			}
			resp, err := next.RoundTrip(req)
			if err != nil || resp.StatusCode != http.StatusOK {
				return resp, err
			}
			raw, err := io.ReadAll(resp.Body)
			if err != nil {
				return nil, err
			}
			mu.Lock()
			store[req.URL.String()] = string(raw)
			mu.Unlock()
			resp.Body = io.NopCloser(strings.NewReader(string(raw)))
			return resp, nil
		})
	}
}

func TestCacheMiddlewareAnswersRepeatedGetsWithoutReachingTheOrigin(t *testing.T) {
	var seen []*http.Request
	client := petstore.New("token", &petstore.Options{Middleware: []petstore.Middleware{cache(), origin(&seen)}})
	for i := 0; i < 3; i++ {
		pet, err := client.Pets().Retrieve(context.Background(), "1")
		if err != nil || pet.Name != "Rex" {
			t.Fatalf("pet %v, err %v", pet, err)
		}
	}
	if len(seen) != 1 {
		t.Fatalf("origin reached %d times, want 1", len(seen))
	}
	if _, err := client.Pets().Retrieve(context.Background(), "2"); err != nil || len(seen) != 2 {
		t.Fatalf("err %v, origin reached %d times, want 2", err, len(seen))
	}
}

func TestMiddlewareCanChangeTheRequest(t *testing.T) {
	var seen []*http.Request
	tag := func(next http.RoundTripper) http.RoundTripper {
		return petstore.RoundTripperFunc(func(req *http.Request) (*http.Response, error) {
			req.Header.Set("x-tag", "yes")
			return next.RoundTrip(req)
		})
	}
	client := petstore.New("token", &petstore.Options{Middleware: []petstore.Middleware{tag, origin(&seen)}})
	if _, err := client.Pets().Retrieve(context.Background(), "1"); err != nil {
		t.Fatal(err)
	}
	if got := seen[0].Header.Get("x-tag"); got != "yes" {
		t.Fatalf("x-tag = %q", got)
	}
	if got := seen[0].Header.Get("Authorization"); got != "Bearer token" {
		t.Fatalf("Authorization = %q", got)
	}
}
