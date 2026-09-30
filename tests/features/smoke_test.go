package features

import (
	"context"
	"io"
	"os"
	"reflect"
	"strings"
	"testing"
	"time"
)

func ids[T any](t *testing.T, pager *Pager[T], id func(T) string) []string {
	t.Helper()
	var out []string
	pager.All()(func(item T, err error) bool {
		if err != nil {
			t.Fatal(err)
		}
		out = append(out, id(item))
		return true
	})
	return out
}

func expect(t *testing.T, got, want any) {
	t.Helper()
	if !reflect.DeepEqual(got, want) {
		t.Fatalf("got %v, want %v", got, want)
	}
}

func TestSmoke(t *testing.T) {
	status := func(health *Health, err error) string {
		t.Helper()
		if err != nil {
			t.Fatal(err)
		}
		return health.Status
	}
	ctx := context.Background()
	url := os.Getenv("FEATURES_URL")
	client := New("tok", &Options{ServerURL: url})
	expect(t, status(client.Account().Health(ctx)), "||")
	expect(t, status(client.Account().MachineStatus(ctx)), "Bearer tok||")
	widget := func(w Widget) string { return w.Id }
	expect(t, ids(t, client.Widgets().ListWidgetsIter(ctx, nil), widget), []string{"w1", "w2", "w3"})
	events := client.Widgets().ListWidgetEventsIter(ctx, "w1", WidgetsListWidgetEventsOptions{Kind: "created"})
	expect(t, ids(t, events, func(e Event) string { return e.Id }), []string{"e1", "e2", "e3"})
	gadgets := client.Gadgets().ListGadgetsIter(ctx, nil)
	var gadgetIds []string
	for gadgets.Next() {
		gadgetIds = append(gadgetIds, gadgets.Current().Id)
	}
	expect(t, gadgets.Err(), nil)
	expect(t, gadgetIds, []string{"g1", "g2", "g3"})
	expect(t, ids(t, client.Records().ListRecordsIter(ctx, nil), func(r Entry) string { return r.Id }), []string{"r1", "r2", "r3"})

	basic := New("", &Options{ServerURL: url, BasicAuth: &BasicAuth{Username: "u", Password: "p"}})
	expect(t, status(basic.Account().CreateSession(ctx)), "Basic dTpw||")
	provided := New("", &Options{ServerURL: url, TokenProvider: func(context.Context) (string, error) { return "fresh", nil }})
	expect(t, status(provided.Account().MachineStatus(ctx)), "Bearer fresh||")
	keyed := New("", &Options{ServerURL: url, APIKeys: map[string]string{"api_key": "k"}})
	expect(t, ids(t, keyed.Widgets().ListWidgetsIter(ctx, nil), widget), []string{"w1", "w2", "w3"})
	failing := New("", &Options{ServerURL: url}).Widgets().ListWidgetsIter(ctx, nil)
	expect(t, failing.Next(), false)
	if failing.Err() == nil {
		t.Fatal("expected an authentication error")
	}
}

func TestSmokeStreaming(t *testing.T) {
	ctx := context.Background()
	client := New("tok", &Options{ServerURL: os.Getenv("FEATURES_URL")})
	topic := "news"
	stream, err := client.Streaming().StreamEvents(ctx, &StreamingStreamEventsOptions{Topic: &topic})
	if err != nil {
		t.Fatal(err)
	}
	defer stream.Close()
	var events []SseEvent
	for stream.Next() {
		events = append(events, stream.Event())
	}
	expect(t, stream.Err(), nil)
	expect(t, events, []SseEvent{
		{Event: "greeting", Data: "news", ID: "1"},
		{Event: "message", Data: "line1\nline2", ID: "1"},
		{Event: "message", Data: `{"n": 3}`, ID: "3", Retry: 1500 * time.Millisecond},
	})

	count := int32(2)
	body := StreamingUploadFileBody{
		File:  Upload{Reader: strings.NewReader("hello"), Filename: "a.txt", ContentType: "text/plain"},
		Name:  "doc",
		Count: &count,
		Meta:  &Health{Status: "ok"},
	}
	uploaded, err := client.Streaming().UploadFile(ctx, body)
	if err != nil {
		t.Fatal(err)
	}
	expect(t, uploaded.Status, `count=::2;file=a.txt:text/plain:hello;meta=:application/json:{"status":"ok"};name=::doc`)
	content, err := client.Streaming().UploadContent(ctx, "f1", strings.NewReader("raw"))
	if err != nil {
		t.Fatal(err)
	}
	expect(t, content.Status, "application/octet-stream:raw")
	piped, err := client.Streaming().UploadContent(ctx, "f1", io.MultiReader(strings.NewReader("pi"), strings.NewReader("ped")))
	if err != nil {
		t.Fatal(err)
	}
	expect(t, piped.Status, "application/octet-stream:piped")
}
