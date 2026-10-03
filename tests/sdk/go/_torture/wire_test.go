package torture

import (
	"context"
	"testing"
)

func TestAPIKeyCookieKeepsCookieOctets(t *testing.T) {
	for _, tc := range []struct{ key, sent string }{
		{"abc/def+ghi==", "abc/def+ghi=="},
		{"a b,c;d\"e\\f", "a%20b%2Cc%3Bd%22e%5Cf"},
		{"café", "caf%C3%A9"},
	} {
		req := newRequest("GET", "/x", nil)
		req.SetAPIKeyCookie("sid", tc.key)
		if len(req.cookies) != 1 || req.cookies[0] != "sid="+tc.sent {
			t.Errorf("key %q: cookies = %q, want sid=%s", tc.key, req.cookies, tc.sent)
		}
	}
}

func TestStyledPathParamKeepsDotSegmentsOut(t *testing.T) {
	for _, tc := range []struct{ value, style, path string }{
		{"", "label", "/pets/%2E"},
		{".", "label", "/pets/%2E%2E"},
		{"..", "simple", "/pets/%2E%2E"},
		{"a", "label", "/pets/.a"},
	} {
		req := newRequest("GET", "/pets/{id}", nil)
		req.SetStyledPathParam("id", tc.value, tc.style, false)
		if req.err != nil || req.path != tc.path {
			t.Errorf("%q %s: path = %q (%v), want %q", tc.value, tc.style, req.path, req.err, tc.path)
		}
	}
}

func TestEmptySuccessBodyOfAnOptionalCollectionIsNil(t *testing.T) {
	for _, status := range []int{200, 205} {
		sc := newScript(reply(status, ""))
		var list []string
		if err := sc.client(Options{}).execute(context.Background(), newRequest("GET", "/x", nil), &list); err != nil {
			t.Errorf("status %d: list: %v", status, err)
		}
		var fields map[string]string
		if err := sc.client(Options{}).execute(context.Background(), newRequest("GET", "/x", nil), &fields); err != nil {
			t.Errorf("status %d: map: %v", status, err)
		}
	}
}
