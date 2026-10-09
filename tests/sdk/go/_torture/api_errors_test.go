package torture

import (
	"bufio"
	"context"
	"errors"
	"net/http"
	"strings"
	"testing"
)

func TestAPIErrorsReadAsTheMessageOfTheBody(t *testing.T) {
	for body, want := range map[string]string{
		`{"error":{"message":"slow down","type":"rate_limit"}}`: "torture: GET /things/{thing_id}: 429 Too Many Requests: slow down",
		`{"message":"slow down"}`:                               "torture: GET /things/{thing_id}: 429 Too Many Requests: slow down",
		`{"detail":"slow down"}`:                                "torture: GET /things/{thing_id}: 429 Too Many Requests: slow down",
		`{"error":"slow down"}`:                                 "torture: GET /things/{thing_id}: 429 Too Many Requests: slow down",
		`{"code":7}`:                                            `torture: GET /things/{thing_id}: 429 Too Many Requests: {"code":7}`,
		"":                                                      "torture: GET /things/{thing_id}: 429 Too Many Requests",
	} {
		sc := newScript(reply(429, body))
		_, err := sc.client(retries(0)).Things().Retrieve(context.Background(), "t")
		var apiErr *APIError
		if !errors.As(err, &apiErr) || err.Error() != want {
			t.Errorf("%s: %v", body, err)
			continue
		}
		if apiErr.Method != http.MethodGet || string(apiErr.RawBody) != body {
			t.Errorf("%s: %+v", body, apiErr)
		}
	}
	long := newScript(reply(500, strings.Repeat("x", 600)))
	_, err := long.client(retries(0)).Things().Retrieve(context.Background(), "t")
	if !strings.HasSuffix(err.Error(), ": "+strings.Repeat("x", 512)+"...") {
		t.Errorf("a long body is not cut: %v", err)
	}
}

func TestStreamErrorEventsAreAPIErrors(t *testing.T) {
	type delta struct {
		A int `json:"a"`
	}
	stream := func(body string) *Stream[delta] {
		header := http.Header{"X-Request-Id": {"req_1"}}
		events := &EventStream{reader: bufio.NewReader(strings.NewReader(body)), method: "POST", path: "/s", status: 200, header: header}
		return &Stream[delta]{events: events}
	}
	for _, body := range []string{
		"event: error\ndata: {\"message\":\"overloaded\"}\n\n",
		"data: {\"error\":{\"message\":\"overloaded\"}}\n\n",
	} {
		s := stream("data: {\"a\":1}\n\n" + body + "data: {\"a\":2}\n\n")
		if !s.Next() || s.Current().A != 1 {
			t.Fatalf("%q: first event %v", body, s.Err())
		}
		var apiErr *APIError
		if s.Next() || !errors.As(s.Err(), &apiErr) {
			t.Fatalf("%q: err = %v", body, s.Err())
		}
		if apiErr.StatusCode != 200 || apiErr.RequestID() != "req_1" || apiErr.Message() != "overloaded" || apiErr.Body == nil {
			t.Errorf("%q: %+v", body, apiErr)
		}
		if s.Next() {
			t.Errorf("%q: the stream goes on after its error", body)
		}
	}

	s := stream("event: ping\ndata: {\"a\":\"x\"}\n\nevent: keepalive\ndata: alive\n\ndata: {\"a\":3}\n\n")
	if !s.Next() || s.Current().A != 3 {
		t.Errorf("keepalives are not skipped: %v", s.Err())
	}
	var decodeErr *DecodeError
	if s := stream("data: nope\n\n"); s.Next() || !errors.As(s.Err(), &decodeErr) {
		t.Errorf("a broken event: %v", s.Err())
	}
	withError := &Stream[map[string]any]{events: stream("data: {\"error\":null,\"a\":1}\n\n").events}
	if !withError.Next() {
		t.Errorf("a null error is data: %v", withError.Err())
	}
}
