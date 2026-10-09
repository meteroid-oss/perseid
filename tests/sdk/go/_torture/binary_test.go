package torture

import (
	"context"
	"errors"
	"io"
	"net/http"
	"os"
	"path/filepath"
	"strings"
	"testing"
	"time"
)

// chunks answers with each part flushed in turn, after wait allows it for each but the first.
func chunks(wait func(*http.Request) bool, parts ...string) func(int32, http.ResponseWriter, *http.Request) {
	return func(_ int32, w http.ResponseWriter, r *http.Request) {
		w.Header().Set("Content-Type", "application/pdf")
		w.Header().Set("Content-Disposition", `attachment; filename="t1.pdf"`)
		for i, part := range parts {
			if i > 0 && wait != nil && !wait(r) {
				return
			}
			_, _ = io.WriteString(w, part)
			w.(http.Flusher).Flush()
		}
	}
}

func gated(gate <-chan struct{}) func(*http.Request) bool {
	return func(r *http.Request) bool {
		select {
		case <-gate:
			return true
		case <-r.Context().Done():
			return false
		}
	}
}

func paced(delay time.Duration) func(*http.Request) bool {
	return func(r *http.Request) bool {
		select {
		case <-time.After(delay):
			return true
		case <-r.Context().Done():
			return false
		}
	}
}

func TestBinaryResponsesStreamBeforeTheirBodyEnds(t *testing.T) {
	gate := make(chan struct{})
	client, _ := server(t, chunks(gated(gate), "%PDF-", "rest"))
	body, err := client.Things().Download(context.Background(), "t1")
	if err != nil {
		t.Fatal(err)
	}
	defer body.Close()
	if body.Header.Get("Content-Type") != "application/pdf" || !strings.Contains(body.Header.Get("Content-Disposition"), "t1.pdf") {
		t.Errorf("headers %v", body.Header)
	}
	first := make([]byte, 5)
	if _, err := io.ReadFull(body, first); err != nil || string(first) != "%PDF-" {
		t.Fatalf("first part %q, %v", first, err)
	}
	close(gate)
	rest, err := body.Bytes()
	if err != nil || string(rest) != "rest" {
		t.Fatalf("rest %q, %v", rest, err)
	}
}

func TestBinaryResponsesReadWholeOrToAFile(t *testing.T) {
	client, _ := server(t, chunks(nil, "%PDF-", "1.7"))
	body, err := client.Things().Download(context.Background(), "t1")
	if err != nil {
		t.Fatal(err)
	}
	data, err := body.Bytes()
	if err != nil || string(data) != "%PDF-1.7" {
		t.Fatalf("Bytes: %q, %v", data, err)
	}
	if n, err := body.Read(make([]byte, 1)); n != 0 || err == nil {
		t.Errorf("read after Bytes closed the body: %d, %v", n, err)
	}

	path := filepath.Join(t.TempDir(), "t1.pdf")
	body, err = client.Things().Download(context.Background(), "t1")
	if err != nil {
		t.Fatal(err)
	}
	if err := body.WriteToFile(path); err != nil {
		t.Fatal(err)
	}
	if written, _ := os.ReadFile(path); string(written) != "%PDF-1.7" {
		t.Errorf("file holds %q", written)
	}
}

func TestAFailedDownloadLeavesTheFileAsItWas(t *testing.T) {
	client, _ := server(t, func(attempt int32, w http.ResponseWriter, _ *http.Request) {
		length := "8"
		if attempt == 1 {
			length = "100"
		}
		w.Header().Set("Content-Length", length)
		_, _ = io.WriteString(w, "%PDF-1.7")
	})
	dir := t.TempDir()
	path := filepath.Join(dir, "t1.pdf")
	if err := os.WriteFile(path, []byte("old"), 0o600); err != nil {
		t.Fatal(err)
	}
	for _, want := range []string{"old", "%PDF-1.7"} {
		body, err := client.Things().Download(context.Background(), "t1")
		if err != nil {
			t.Fatal(err)
		}
		if err := body.WriteToFile(path); (err == nil) != (want != "old") {
			t.Errorf("WriteToFile: %v", err)
		}
		if written, _ := os.ReadFile(path); string(written) != want {
			t.Errorf("file holds %q, want %q", written, want)
		}
	}
	if entries, _ := os.ReadDir(dir); len(entries) != 1 {
		t.Errorf("files left: %v", entries)
	}
}

func TestBinaryErrorStatusesAreReturnedBeforeTheBody(t *testing.T) {
	client, rec := server(t, func(_ int32, w http.ResponseWriter, _ *http.Request) {
		w.Header().Set("Content-Type", "application/json")
		w.WriteHeader(http.StatusNotFound)
		_, _ = io.WriteString(w, `{"message":"no such thing"}`)
	})
	body, err := client.Things().Download(context.Background(), "t1")
	var apiErr *APIError
	if body != nil || !errors.As(err, &apiErr) || apiErr.StatusCode != http.StatusNotFound || !strings.Contains(string(apiErr.RawBody), "no such thing") {
		t.Fatalf("got %v, %v", body, err)
	}
	if rec.requests.Load() != 1 {
		t.Errorf("%d attempts", rec.requests.Load())
	}
}

func TestBinaryResponsesAreRetriedUntilTheirHeadersOnly(t *testing.T) {
	client, rec := server(t, func(n int32, w http.ResponseWriter, _ *http.Request) {
		if n == 1 {
			w.WriteHeader(http.StatusServiceUnavailable)
			return
		}
		// The body is cut short of its declared length.
		w.Header().Set("Content-Length", "10")
		_, _ = io.WriteString(w, "%PDF-")
	})
	body, err := client.Things().Download(context.Background(), "t1")
	if err != nil {
		t.Fatal(err)
	}
	data, err := body.Bytes()
	var transportErr *TransportError
	if string(data) != "%PDF-" || !errors.As(err, &transportErr) {
		t.Errorf("got %q, %v", data, err)
	}
	if got := rec.requests.Load(); got != 2 {
		t.Errorf("%d attempts, want 2", got)
	}
}

func TestTheTimeoutOfABinaryResponseBoundsEachReadNotTheDownload(t *testing.T) {
	parts := strings.Split("xxxxxx", "")
	client, _ := server(t, chunks(paced(50*time.Millisecond), parts...))
	body, err := client.Things().Download(context.Background(), "t1", WithTimeout(200*time.Millisecond))
	if err != nil {
		t.Fatal(err)
	}
	if data, err := body.Bytes(); err != nil || len(data) != len(parts) {
		t.Fatalf("a download longer than the timeout, but no read: %q, %v", data, err)
	}

	client, _ = server(t, chunks(paced(time.Second), parts...))
	body, err = client.Things().Download(context.Background(), "t1", WithTimeout(100*time.Millisecond))
	if err != nil {
		t.Fatal(err)
	}
	_, err = body.Bytes()
	var timeoutErr *TimeoutError
	if !errors.As(err, &timeoutErr) {
		t.Errorf("a stalled read: %v", err)
	}
}

func TestCancellingTheContextStopsTheRead(t *testing.T) {
	client, _ := server(t, chunks(gated(make(chan struct{})), "%PDF-", "never"))
	ctx, cancel := context.WithCancel(context.Background())
	body, err := client.Things().Download(ctx, "t1", WithTimeout(-1))
	if err != nil {
		t.Fatal(err)
	}
	defer body.Close()
	time.AfterFunc(50*time.Millisecond, cancel)
	data, err := io.ReadAll(body)
	if string(data) != "%PDF-" || !errors.Is(err, context.Canceled) {
		t.Errorf("got %q, %v", data, err)
	}
}
