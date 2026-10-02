package features

import (
	"context"
	"encoding/json"
	"errors"
	"io"
	"net/http"
	"os"
	"reflect"
	"strings"
	"testing"
	"time"
)

func ids[T any](t *testing.T, pager *AutoPager[T], id func(T) string) []string {
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
	expect(t, status(client.Account().CheckHealth(ctx)), "||")
	expect(t, status(client.Account().RetrieveMachine(ctx)), "Bearer tok||")
	widget := func(w Widget) string { return w.ID }
	expect(t, ids(t, client.Widgets().ListAutoPaging(ctx, nil), widget), []string{"w1", "w2", "w3"})
	events := client.Widgets().ListEventsAutoPaging(ctx, "w1", "created", nil)
	expect(t, ids(t, events, func(e Event) string { return e.ID }), []string{"e1", "e2", "e3"})
	gadgets := client.Gadgets().ListAutoPaging(ctx, nil)
	var gadgetIds []string
	for gadgets.Next() {
		gadgetIds = append(gadgetIds, gadgets.Current().ID)
	}
	expect(t, gadgets.Err(), nil)
	expect(t, gadgetIds, []string{"g1", "g2", "g3"})
	expect(t, ids(t, client.Records().ListAutoPaging(ctx, nil), func(r Entry) string { return r.ID }), []string{"r1", "r2", "r3"})
	var records *RecordsListPage
	records, err := client.Records().List(ctx, nil)
	if err != nil {
		t.Fatal(err)
	}
	expect(t, records.Body.Total, int64(3))

	_, err = New("", &Options{ServerURL: url}).Widgets().List(ctx, nil)
	expect(t, errors.Is(err, ErrUnauthorized) && !errors.Is(err, ErrNotFound), true)
	var sdkErr SDKError
	var apiErr *APIError
	expect(t, errors.As(err, &sdkErr) && errors.As(err, &apiErr), true)
	expect(t, apiErr.Body, map[string]any{"error": "unauthorized"})
	expect(t, apiErr.RequestID(), "req_mock")

	basic := New("", &Options{ServerURL: url, BasicAuth: &BasicAuth{Username: "u", Password: "p"}})
	expect(t, status(basic.Account().CreateSession(ctx)), "Basic dTpw||")
	provided := New("", &Options{ServerURL: url, TokenProvider: func(context.Context) (string, error) { return "fresh", nil }})
	expect(t, status(provided.Account().RetrieveMachine(ctx)), "Bearer fresh||")
	keyed := New("", &Options{ServerURL: url, APIKeys: map[string]string{"api_key": "k"}})
	expect(t, ids(t, keyed.Widgets().ListAutoPaging(ctx, nil), widget), []string{"w1", "w2", "w3"})
	failing := New("", &Options{ServerURL: url}).Widgets().ListAutoPaging(ctx, nil)
	expect(t, failing.Next(), false)
	if failing.Err() == nil {
		t.Fatal("expected an authentication error")
	}
}

func TestSmokeParity(t *testing.T) {
	ctx := context.Background()
	url := os.Getenv("FEATURES_URL")
	client := New("tok", &Options{ServerURL: url})

	var resp *http.Response
	page, err := client.Widgets().List(ctx, nil, WithResponseInto(&resp))
	if err != nil {
		t.Fatal(err)
	}
	expect(t, resp.StatusCode, 200)
	expect(t, resp.Header.Get("X-Request-Id"), "req_mock")
	expect(t, len(page.Items), 2)
	expect(t, page.Items[0].ExtraFields["color"], json.RawMessage(`"red"`))
	encoded, err := json.Marshal(page.Items[0])
	if err != nil {
		t.Fatal(err)
	}
	expect(t, string(encoded), `{"id":"w1","name":"w1","color":"red"}`)
	expect(t, page.HasNextPage(), true)
	if page, err = page.NextPage(ctx); err != nil {
		t.Fatal(err)
	}
	expect(t, page.Items[0].ID, "w3")
	expect(t, page.HasNextPage(), false)
	if page, err = page.NextPage(ctx); page != nil || err != nil {
		t.Fatalf("after the last page: %v %v", page, err)
	}

	completion, err := client.Streaming().CreateCompletion(ctx, CompletionRequest{Prompt: "ab"})
	if err != nil {
		t.Fatal(err)
	}
	expect(t, completion.Text, "AB")
	request := CompletionRequest{Prompt: "ab"}
	stream, err := client.Streaming().CreateCompletionStream(ctx, request)
	if err != nil {
		t.Fatal(err)
	}
	defer stream.Close()
	var deltas []string
	for chunk, err := range stream.All() {
		if err != nil {
			t.Fatal(err)
		}
		expect(t, stream.Event().Event, "message")
		expect(t, chunk.ExtraFields["index"] != nil, true)
		deltas = append(deltas, chunk.Delta)
	}
	expect(t, deltas, []string{"a", "b"})
	expect(t, request.Stream, (*bool)(nil))

	t.Setenv("FEATURES_BASE_URL", url)
	t.Setenv("FEATURES_API_KEY", "env")
	health, err := New("", nil).Account().RetrieveMachine(ctx)
	if err != nil {
		t.Fatal(err)
	}
	expect(t, health.Status, "Bearer env||")
	health, err = New("tok", &Options{ServerURL: url}).Account().RetrieveMachine(ctx)
	if err != nil {
		t.Fatal(err)
	}
	expect(t, health.Status, "Bearer tok||")
}

func TestSmokeStreaming(t *testing.T) {
	ctx := context.Background()
	client := New("tok", &Options{ServerURL: os.Getenv("FEATURES_URL")})
	topic := "news"
	stream, err := client.Streaming().RetrieveEventsStream(ctx, &StreamingRetrieveEventsStreamOptions{Topic: &topic})
	if err != nil {
		t.Fatal(err)
	}
	defer stream.Close()
	var events []SSEEvent
	for stream.Next() {
		events = append(events, stream.Event())
	}
	expect(t, stream.Err(), nil)
	expect(t, events, []SSEEvent{
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
		Tags:  []string{"a", "b"},
	}
	uploaded, err := client.Streaming().UploadFile(ctx, body)
	if err != nil {
		t.Fatal(err)
	}
	expect(t, uploaded.Status, `count=::2;file=a.txt:text/plain:hello;meta=:application/json:{"status":"ok"};name=::doc;tags=::a;tags=::b`)
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

func TestSmokeWire(t *testing.T) {
	ctx := context.Background()
	wire := New("tok", &Options{ServerURL: os.Getenv("FEATURES_URL")}).Wire()
	status := func(health *Health, err error) string {
		t.Helper()
		if err != nil {
			t.Fatal(err)
		}
		return health.Status
	}
	search := &WireSearchOptions{
		Filter:   &Filter{Status: Ptr("open"), Amount: &FilterAmount{Gte: Ptr[int64](5)}},
		Expand:   []string{"a", "b"},
		Metadata: map[string]string{"k": "v"},
		IDs:      Ptr(NewWireSearchIDsFromList([]string{"x", "y"})),
		Tags:     []string{"t1", "t2"},
		Range:    &SearchRange{Gte: Ptr[int64](1), Lt: Ptr[int64](9)},
	}
	expect(t, status(wire.Search(ctx, search)),
		"expand[]=a&expand[]=b&filter[amount][gte]=5&filter[status]=open&ids=x&ids=y"+
			"&metadata[k]=v&range[gte]=1&range[lt]=9&tags=t1,t2")
	charge := Charge{
		Amount:   100,
		Capture:  Ptr(true),
		Metadata: map[string]string{"order": "7"},
		Items:    []ChargeItemsItem{{Price: "p1", Quantity: Ptr[int64](2)}, {Price: "p2"}},
		Expand:   []string{"customer"},
		Statuses: []string{"a", "b"},
		Codes:    []string{"c1", "c2"},
		Shipping: &ChargeShipping{Address: &ChargeShippingAddress{Line1: Ptr("1 Main"), City: Ptr("Paris")}},
	}
	expect(t, status(wire.CreateCharge(ctx, charge)),
		"application/x-www-form-urlencoded|amount=100&capture=true&codes=c1,c2&expand[]=customer"+
			"&items[0][price]=p1&items[0][quantity]=2&items[1][price]=p2&metadata[order]=7"+
			"&shipping[address][city]=Paris&shipping[address][line1]=1 Main&statuses=a&statuses=b")
	beta := &WireBetaSearchOptions{Limit: Ptr[int32](2), Features: Ptr("x,y")}
	expect(t, status(wire.BetaSearch(ctx, beta)), "beta=true&limit=2|features=x,y")
	expect(t, status(wire.UpdateImage(ctx, "42", strings.NewReader("png"))), "42:image/png:png")
}
