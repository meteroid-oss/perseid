package torture

import (
	"context"
	"encoding/json"
	"errors"
	"io"
	"net/http"
	"testing"
)

func TestPrimitiveOrObjectUnions(t *testing.T) {
	for _, tc := range []struct {
		in    string
		check func(StringOrInt) bool
	}{
		{`"a"`, func(u StringOrInt) bool { return u.String != nil && *u.String == "a" && u.Integer == nil }},
		{`7`, func(u StringOrInt) bool { return u.Integer != nil && *u.Integer == 7 && u.String == nil }},
		{`true`, func(u StringOrInt) bool { return string(u.Raw()) == "true" }},
		{`null`, func(u StringOrInt) bool { return u.String == nil && u.Integer == nil && u.Raw() == nil }},
	} {
		var u StringOrInt
		if err := json.Unmarshal([]byte(tc.in), &u); err != nil || !tc.check(u) {
			t.Fatalf("%s: %+v %v", tc.in, u, err)
		}
		if out, err := json.Marshal(u); err != nil || string(out) != tc.in {
			t.Errorf("%s re-encodes as %s %v", tc.in, out, err)
		}
	}

	holder := UnionHolder{Shape: NewShapeCircle(Circle{Radius: 1}), StrOrInt: Ptr(NewUnionHolderStrOrIntFromInteger(3)), InlineUnion: Ptr(NewUnionHolderInlineUnionFromList([]string{"x"}))}
	out, err := json.Marshal(holder)
	if err != nil {
		t.Fatal(err)
	}
	sameJSON(t, string(out), `{"shape":{"type":"circle","radius":1},"shapes":[],"str_or_int":3,"inline_union":["x"]}`)
	var decoded UnionHolder
	if err := json.Unmarshal([]byte(`{"str_or_int":"s","inline_union":"one"}`), &decoded); err != nil {
		t.Fatal(err)
	}
	if *decoded.StrOrInt.String != "s" || *decoded.InlineUnion.String != "one" {
		t.Fatalf("%+v", decoded)
	}
}

func TestVariantsDefaultTheirDiscriminator(t *testing.T) {
	out, err := json.Marshal(Circle{Radius: 2})
	if err != nil {
		t.Fatal(err)
	}
	sameJSON(t, string(out), `{"type":"circle","radius":2}`)
	out, err = json.Marshal(Cat{})
	if err != nil {
		t.Fatal(err)
	}
	var fields map[string]any
	if err := json.Unmarshal(out, &fields); err != nil || fields["pet_type"] != "Cat" {
		t.Fatalf("%s %v", out, err)
	}
}

func TestErrorsMatchStatusSentinelsAndDecodeBodies(t *testing.T) {
	client, _ := server(t, func(_ int32, w http.ResponseWriter, r *http.Request) {
		w.Header().Set("X-Request-Id", "req_1")
		if r.Method == http.MethodGet {
			w.WriteHeader(http.StatusNotFound)
			io.WriteString(w, `{"title":"no widgets"}`)
			return
		}
		w.WriteHeader(http.StatusUnprocessableEntity)
		io.WriteString(w, `not json`)
	})

	_, err := client.Widgets().ListWidgets(context.Background(), nil)
	if !errors.Is(err, ErrNotFound) || errors.Is(err, ErrConflict) || errors.Is(err, ErrServer) {
		t.Fatalf("sentinels: %v", err)
	}
	problem, ok := ErrorBody[Problem](err)
	if !ok || problem.Title != "no widgets" {
		t.Fatalf("body: %+v %v", problem, ok)
	}
	var apiErr *APIError
	if !errors.As(err, &apiErr) || apiErr.RequestID() != "req_1" {
		t.Fatalf("request id: %v", err)
	}

	_, err = client.Widgets().CreateWidget(context.Background(), CreateWidgetRequest{})
	if !errors.Is(err, ErrUnprocessableEntity) {
		t.Fatalf("422: %v", err)
	}
	if body, ok := ErrorBody[Problem](err); ok {
		t.Fatalf("a non-JSON body decoded: %+v", body)
	}
	if _, ok := ErrorBody[Problem](errors.New("other")); ok {
		t.Fatal("a non-API error decoded")
	}
}
