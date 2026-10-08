package features

import (
	"context"
	"encoding/base64"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"math"
	"net/http"
	"os"
	"reflect"
	"strconv"
	"strings"
	"sync"
	"sync/atomic"
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
	before := "e9"
	events = client.Widgets().ListEventsAutoPaging(ctx, "w1", "created", &WidgetsListEventsOptions{EndingBefore: &before})
	expect(t, ids(t, events, func(e Event) string { return e.ID }), []string{"e7", "e8", "e6"})
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
	expect(t, records.Total, int64(3))
	expect(t, records.EntryPage.Total, int64(3))
	expect(t, records.Items[0].ID, "r1")
	records, err = records.NextPage(ctx)
	must(t, err)
	expect(t, records.Items[0].ID, "r3")
	expect(t, records.Total, int64(3))
	expect(t, records.HasNextPage(), false)

	var gadgetPages [][]string
	var totalPages []int32
	gadgetPage, err := client.Gadgets().List(ctx, nil)
	for ; gadgetPage != nil; gadgetPage, err = gadgetPage.NextPage(ctx) {
		var pageIDs []string
		for _, gadget := range gadgetPage.Items {
			pageIDs = append(pageIDs, gadget.ID)
		}
		gadgetPages = append(gadgetPages, pageIDs)
		totalPages = append(totalPages, gadgetPage.Meta.TotalPages)
		expect(t, gadgetPage.Meta.Page, int32(len(gadgetPages)-1))
	}
	must(t, err)
	expect(t, gadgetPages, [][]string{{"g1", "g2"}, {"g3"}})
	expect(t, totalPages, []int32{2, 2})

	encoded, err := json.Marshal(records)
	must(t, err)
	expect(t, asJSON(t, encoded), map[string]any{"data": []any{map[string]any{"id": "r3"}}, "total": float64(3)})

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
	expect(t, page.NextCursor, Ptr("c2"))
	expect(t, len(page.WidgetList.Data), 2)
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
	expect(t, page.NextCursor == nil, true)
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
		IDs:      Ptr(NewWireSearchIDsFromArrayOfStrings([]string{"x", "y"})),
		Tags:     []string{"t1", "t2"},
		Range:    &SearchRange{Gte: Ptr[int64](1), Lt: Ptr[int64](9)},
		Created:  Ptr(NewWireSearchCreatedFromRangeQuerySpecs(RangeQuerySpecs{Gte: Ptr[int64](3), Lt: Ptr[int64](7)})),
	}
	expect(t, status(wire.Search(ctx, search)),
		"created[gte]=3&created[lt]=7&expand[]=a&expand[]=b&filter[amount][gte]=5&filter[status]=open&ids=x&ids=y"+
			"&metadata[k]=v&range[gte]=1&range[lt]=9&tags=t1,t2")
	expect(t, status(wire.Search(ctx, &WireSearchOptions{Created: Ptr(NewWireSearchCreatedFromInteger(5))})), "created=5")
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
	beta := &WireBetaSearchOptions{Limit: Ptr[int32](2), Features: []string{"x", "y"}}
	expect(t, status(wire.BetaSearch(ctx, beta)), "beta=true&limit=2|features=x,y")
	expect(t, status(wire.UpdateImage(ctx, "42", strings.NewReader("png"))), "42:image/png:png")
}

// The scenarios of tests/features/SCENARIOS.md, one test per group, against the strict mock server.

func must(t *testing.T, err error) {
	t.Helper()
	if err != nil {
		t.Fatal(err)
	}
}

// apiError is the *APIError in err's chain, which must exist.
func apiError(t *testing.T, err error) *APIError {
	t.Helper()
	var apiErr *APIError
	if !errors.As(err, &apiErr) {
		t.Fatalf("want an *APIError, got %T: %v", err, err)
	}
	return apiErr
}

// asJSON decodes raw JSON text, whatever its spacing.
func asJSON(t *testing.T, raw []byte) any {
	t.Helper()
	var value any
	must(t, json.Unmarshal(raw, &value))
	return value
}

var scenarioCount atomic.Int64

// scenarioID is an X-Scenario-Id unique to one test case.
func scenarioID(name string) string {
	return fmt.Sprintf("go-%s-%d", name, scenarioCount.Add(1))
}

// scenarioClient is a client of the mock server with the token "tok".
func scenarioClient(options Options) *Client {
	options.ServerURL = os.Getenv("FEATURES_URL")
	return New("tok", &options)
}

// countRequests counts the HTTP attempts that go through the client it is a middleware of.
func countRequests(count *atomic.Int32) Middleware {
	return func(next http.RoundTripper) http.RoundTripper {
		return RoundTripperFunc(func(req *http.Request) (*http.Response, error) {
			count.Add(1)
			return next.RoundTrip(req)
		})
	}
}

type serverAttempts struct {
	Attempts int      `json:"attempts"`
	Keys     []string `json:"keys"`
}

// attemptsOf asks the mock server what it saw for a scenario id.
func attemptsOf(t *testing.T, id string) serverAttempts {
	t.Helper()
	resp, err := http.Get(os.Getenv("FEATURES_URL") + "/__server/attempts/" + id)
	must(t, err)
	defer resp.Body.Close()
	var seen serverAttempts
	must(t, json.NewDecoder(resp.Body).Decode(&seen))
	return seen
}

func TestScenarioItems(t *testing.T) {
	ctx := context.Background()
	client := scenarioClient(Options{})

	item, err := client.Items().Retrieve(ctx, "i1")
	must(t, err)
	expect(t, item.ID, "i1")
	expect(t, item.Name, "first")
	expect(t, item.Note, Ptr("hi"))

	// The unset note is omitted from the body (the server requires exactly {"name":"renamed"}).
	item, err = client.Items().Update(ctx, "i1", ItemPatch{Name: Ptr("renamed")})
	must(t, err)
	expect(t, item.Name, "renamed")
	expect(t, item.Note == nil, true)

	// 204 without a body or content type.
	must(t, client.Items().Delete(ctx, "i1"))
}

func TestScenarioContent(t *testing.T) {
	ctx := context.Background()
	content := scenarioClient(Options{}).Content()

	t.Run("nullable body", func(t *testing.T) {
		item, err := content.RetrieveScenariosNullableBody(ctx)
		must(t, err)
		expect(t, item == nil, true)
	})
	t.Run("text", func(t *testing.T) {
		text, err := content.RetrieveScenariosText(ctx)
		must(t, err)
		expect(t, text, "hello text\n")
		csv, err := content.RetrieveScenariosCsv(ctx)
		must(t, err)
		expect(t, csv, "id,name\n1,alpha\n2,\"be,ta\"\n")
	})
	t.Run("binary", func(t *testing.T) {
		blob, err := content.DownloadBlob(ctx)
		must(t, err)
		all := make([]byte, 256)
		for i := range all {
			all[i] = byte(i)
		}
		expect(t, blob, all)
		image, err := content.DownloadImage(ctx)
		must(t, err)
		png := []byte{0x89, 'P', 'N', 'G', '\r', '\n', 0x1a, '\n'}
		for range 4 {
			png = append(png, 0x00, 0x01, 0xfe, 0xff)
		}
		expect(t, len(image), 24)
		expect(t, image, png)
	})
	t.Run("malformed JSON is a decode error", func(t *testing.T) {
		health, err := content.RetrieveScenariosMalformed(ctx)
		var decodeErr *DecodeError
		if !errors.As(err, &decodeErr) || health != nil {
			t.Fatalf("got %+v, %v", health, err)
		}
		expect(t, decodeErr.StatusCode, 200)
		expect(t, string(decodeErr.RawBody), `{"status": `)
		var sdkErr SDKError
		expect(t, errors.As(err, &sdkErr), true)
		expect(t, errors.As(err, new(*APIError)), false)
	})
	t.Run("empty body is a decode error", func(t *testing.T) {
		health, err := content.RetrieveScenariosEmptyBody(ctx)
		var decodeErr *DecodeError
		if !errors.As(err, &decodeErr) || health != nil {
			t.Fatalf("got %+v, %v", health, err)
		}
	})
	t.Run("unknown fields are kept and ignored", func(t *testing.T) {
		health, err := content.ListScenariosExtraFields(ctx)
		must(t, err)
		expect(t, health.Status, "ok")
		expect(t, len(health.ExtraFields), 3)
		expect(t, asJSON(t, health.ExtraFields["extra"]), float64(1))
		expect(t, asJSON(t, health.ExtraFields["nested"]), map[string]any{"a": []any{float64(1), float64(2), map[string]any{"b": nil}}})
		expect(t, asJSON(t, health.ExtraFields["list"]), []any{float64(1), "x"})
	})
	t.Run("nulls", func(t *testing.T) {
		bag, err := content.Nulls().Retrieve(ctx)
		must(t, err)
		expect(t, bag.Name == nil, true)
		expect(t, bag.Tags, RequiredSlice[*string]{Ptr("a"), nil, Ptr("b")})
		expect(t, bag.Counts, RequiredMap[*int64]{"x": Ptr[int64](1), "y": nil})
		expect(t, bag.Note == nil, true)
	})
	t.Run("nulls are sent", func(t *testing.T) {
		// The name is an explicit null, the note is unset and omitted.
		sent := NullBag{
			Tags:   RequiredSlice[*string]{Ptr("a"), nil},
			Counts: RequiredMap[*int64]{"x": nil, "y": Ptr[int64](2)},
		}
		echoed, err := content.Nulls().Create(ctx, sent)
		must(t, err)
		expect(t, echoed.Name == nil, true)
		expect(t, echoed.Tags, sent.Tags)
		expect(t, echoed.Counts, sent.Counts)
		expect(t, echoed.Note == nil, true)
	})
	t.Run("additional properties", func(t *testing.T) {
		bag, err := content.RetrieveScenariosBag(ctx)
		must(t, err)
		expect(t, bag.ID, "b1")
		expect(t, bag.ExtraFields, map[string]int64{"a": 1, "b": 2})
	})
	t.Run("string map", func(t *testing.T) {
		labels, err := content.ListScenariosLabels(ctx)
		must(t, err)
		expect(t, labels, map[string]string{"k": "v", "z": "y"})
	})
	t.Run("enums", func(t *testing.T) {
		known, err := content.RetrieveScenariosEnum(ctx, EnumValueModeKnown)
		must(t, err)
		expect(t, known.Kind, PaintKindRed)
		expect(t, known.Kinds, []PaintKindsItem{PaintKindsItemGreen, PaintKindsItemBlue})
		expect(t, known.Kind.IsKnown(), true)

		unknown, err := content.RetrieveScenariosEnum(ctx, EnumValueModeUnknown)
		must(t, err)
		expect(t, unknown.Kind, PaintKind("magenta"))
		expect(t, unknown.Kind.IsKnown(), false)
		expect(t, unknown.Kinds, []PaintKindsItem{PaintKindsItemRed, PaintKindsItem("magenta")})
	})
}

func TestScenarioEncoding(t *testing.T) {
	ctx := context.Background()
	encoding := scenarioClient(Options{}).Encoding()

	t.Run("int64 beyond 2^53", func(t *testing.T) {
		sent := BigBox{Value: 9007199254740993, Min: math.MinInt64}
		got, err := encoding.ScenariosBigint(ctx, sent)
		must(t, err)
		expect(t, got.Value, int64(9007199254740993))
		expect(t, got.Min, int64(math.MinInt64))
	})
	t.Run("bytes", func(t *testing.T) {
		want := []byte("hello\xfb\xff\xfe")
		// The SDK keeps a `format: byte` property as its standard base64 text.
		echoed, err := encoding.Bytes().Create(ctx, Blob{Data: base64.StdEncoding.EncodeToString(want)})
		must(t, err)
		expect(t, echoed.Data, "aGVsbG/7//4=")
		got, err := encoding.Bytes().Retrieve(ctx)
		must(t, err)
		for _, blob := range []*Blob{echoed, got} {
			decoded, err := base64.StdEncoding.DecodeString(blob.Data)
			must(t, err)
			expect(t, decoded, want)
		}
	})
	t.Run("dates and times", func(t *testing.T) {
		instant := time.Date(2024, 1, 2, 3, 4, 5, 250_000_000, time.UTC)
		offset := instant.In(time.FixedZone("plus2", 2*3600))
		day := Date{Year: 2024, Month: time.January, Day: 2}
		for _, at := range []time.Time{instant, offset} {
			box, err := encoding.RetrieveScenariosDatetime(ctx, at, day)
			must(t, err)
			expect(t, box.At.Equal(instant), true)
			expect(t, box.Day, day)
			box, err = encoding.ScenariosDatetime(ctx, DateBox{At: at, Day: day})
			must(t, err)
			expect(t, box.At.Equal(instant), true)
			expect(t, box.Day, day)
		}
	})
	t.Run("path segments", func(t *testing.T) {
		for _, value := range []string{"plain", "sp ace", "sl/ash", "q?mark", "per%cent", "ha#sh", "lit%25eral", "a+b", "héllo wörld ✓"} {
			health, err := encoding.RetrieveScenarioPath(ctx, value)
			if err != nil {
				t.Fatalf("%q: %v", value, err)
			}
			expect(t, health.Status, value)
		}
	})
	t.Run("query text", func(t *testing.T) {
		for _, value := range []string{"plain", "sp ace", "a&b=c+d", "100%", "slash/qm?", "héllo wörld ✓"} {
			health, err := encoding.RetrieveScenariosQuery(ctx, value)
			if err != nil {
				t.Fatalf("%q: %v", value, err)
			}
			expect(t, health.Status, value)
		}
	})
	t.Run("exploded query array", func(t *testing.T) {
		health, err := encoding.RetrieveScenariosMulti(ctx, []string{"b", "a", "c"}, true)
		must(t, err)
		expect(t, health.Status, "b,a,c")
	})
	t.Run("headers", func(t *testing.T) {
		var seen []http.Header
		capture := func(next http.RoundTripper) http.RoundTripper {
			return RoundTripperFunc(func(req *http.Request) (*http.Response, error) {
				seen = append(seen, req.Header.Clone())
				return next.RoundTrip(req)
			})
		}
		headers := scenarioClient(Options{Middleware: []Middleware{capture}}).Encoding()
		health, err := headers.ListScenariosHeaders(ctx, "acme", nil)
		must(t, err)
		expect(t, health.Status, "acme|")
		health, err = headers.ListScenariosHeaders(ctx, "acme", &EncodingListScenariosHeadersOptions{XTraceID: Ptr("t1")})
		must(t, err)
		expect(t, health.Status, "acme|t1")
		expect(t, len(seen), 2)
		expect(t, seen[0].Get("X-Tenant"), "acme")
		_, traced := seen[0]["X-Trace-Id"]
		expect(t, traced, false)
		expect(t, seen[1].Get("X-Trace-Id"), "t1")
		// An operation that declares no security carries no credentials, token or not.
		expect(t, seen[0].Get("Authorization"), "")
	})
}

func TestScenarioCookies(t *testing.T) {
	ctx := context.Background()
	t.Setenv(APIKeyEnv, "")

	health, err := scenarioClient(Options{}).Cookies().RetrieveScenariosCookie(ctx, "abc123")
	must(t, err)
	expect(t, health.Status, "abc123")

	var seen http.Header
	capture := func(next http.RoundTripper) http.RoundTripper {
		return RoundTripperFunc(func(req *http.Request) (*http.Response, error) {
			seen = req.Header.Clone()
			return next.RoundTrip(req)
		})
	}
	// The API key cookie replaces the token of the client.
	keyed := New("", &Options{
		ServerURL:  os.Getenv("FEATURES_URL"),
		APIKeys:    map[string]string{"api_key_cookie": "ck1"},
		Middleware: []Middleware{capture},
	})
	health, err = keyed.Cookies().RetrieveScenariosCookieAuth(ctx)
	must(t, err)
	expect(t, health.Status, "ck1")
	expect(t, seen.Get("Cookie"), "auth_token=ck1")
	expect(t, seen.Get("Authorization"), "")

	_, err = New("", &Options{ServerURL: os.Getenv("FEATURES_URL")}).Cookies().RetrieveScenariosCookieAuth(ctx)
	expect(t, errors.Is(err, ErrUnauthorized), true)
}

func TestScenarioOAuth(t *testing.T) {
	ctx := context.Background()
	t.Setenv(APIKeyEnv, "")
	t.Setenv(ClientIDEnv, "")
	t.Setenv(ClientSecretEnv, "")
	const secret = "p@ss word"
	client := func(id, secret string, options Options) *Client {
		options.ServerURL, options.ClientID, options.ClientSecret = os.Getenv("FEATURES_URL"), id, secret
		return New("", &options)
	}
	status := func(client *Client) string {
		t.Helper()
		health, err := client.Account().RetrieveMachine(ctx)
		must(t, err)
		return health.Status
	}

	cached := client("go-oauth", secret, Options{})
	expect(t, status(cached), "Bearer at-go-oauth-1||")
	expect(t, status(cached), "Bearer at-go-oauth-1||")
	expect(t, attemptsOf(t, "go-oauth").Attempts, 1)

	expect(t, status(client("go-oauth-body", secret, Options{OAuthClientAuth: "body"})), "Bearer at-go-oauth-body-1||")

	expect(t, status(client("go-oauth-revoked", secret, Options{MaxRetries: -1})), "Bearer at-go-oauth-revoked-2||")
	expect(t, attemptsOf(t, "go-oauth-revoked").Attempts, 2)

	concurrent := client("go-oauth-concurrent", secret, Options{})
	var wg sync.WaitGroup
	for range 4 {
		wg.Add(1)
		go func() {
			defer wg.Done()
			if got := status(concurrent); got != "Bearer at-go-oauth-concurrent-1||" {
				t.Errorf("got %q", got)
			}
		}()
	}
	wg.Wait()
	expect(t, attemptsOf(t, "go-oauth-concurrent").Attempts, 1)

	_, err := client("go-oauth-bad", "wrong", Options{MaxRetries: -1}).Account().RetrieveMachine(ctx)
	expect(t, errors.Is(err, ErrUnauthorized), true)
	expect(t, apiError(t, err).Body, map[string]any{"error": "invalid_client"})

	t.Setenv(ClientIDEnv, "go-oauth-env")
	t.Setenv(ClientSecretEnv, secret)
	fromEnv := New("", &Options{ServerURL: os.Getenv("FEATURES_URL")})
	expect(t, status(fromEnv), "Bearer at-go-oauth-env-1||")

	// A token wins over the client credentials.
	withToken := New("tok", &Options{ServerURL: os.Getenv("FEATURES_URL"), ClientID: "go-oauth", ClientSecret: secret})
	expect(t, status(withToken), "Bearer tok||")
}

func TestScenarioRetries(t *testing.T) {
	ctx := context.Background()
	retries := scenarioClient(Options{MaxRetries: 2}).Retries()

	t.Run("Retry-After in seconds", func(t *testing.T) {
		id := scenarioID("flaky")
		start := time.Now()
		health, err := retries.RetrieveScenariosFlaky(ctx, id)
		must(t, err)
		expect(t, health.Status, "attempt=2")
		expect(t, attemptsOf(t, id).Attempts, 2)
		if elapsed := time.Since(start); elapsed < 900*time.Millisecond || elapsed > 10*time.Second {
			t.Errorf("waited %v, want about the 1s of Retry-After", elapsed)
		}
	})
	t.Run("no retries", func(t *testing.T) {
		id := scenarioID("flaky-none")
		health, err := scenarioClient(Options{MaxRetries: -1}).Retries().RetrieveScenariosFlaky(ctx, id)
		expect(t, health == nil, true)
		expect(t, apiError(t, err).StatusCode, 503)
		expect(t, errors.Is(err, ErrServer), true)
		expect(t, attemptsOf(t, id).Attempts, 1)
	})
	t.Run("Retry-After as an HTTP date", func(t *testing.T) {
		id := scenarioID("rate-limited")
		start := time.Now()
		health, err := retries.RetrieveScenariosRateLimited(ctx, id)
		must(t, err)
		expect(t, health.Status, "attempt=2")
		expect(t, attemptsOf(t, id).Attempts, 2)
		if elapsed := time.Since(start); elapsed < 900*time.Millisecond || elapsed > 10*time.Second {
			t.Errorf("waited %v, want 1 to 2s from the HTTP date", elapsed)
		}
	})
	t.Run("retries are exhausted", func(t *testing.T) {
		id := scenarioID("unavailable")
		fast := scenarioClient(Options{RetrySchedule: []time.Duration{5 * time.Millisecond, 5 * time.Millisecond}}).Retries()
		_, err := fast.RetrieveScenariosUnavailable(ctx, id)
		expect(t, apiError(t, err).StatusCode, 503)
		expect(t, attemptsOf(t, id).Attempts, 3)
	})
	t.Run("the idempotency key survives retries", func(t *testing.T) {
		id := scenarioID("idempotent")
		fast := scenarioClient(Options{RetrySchedule: []time.Duration{5 * time.Millisecond, 5 * time.Millisecond}}).Retries()
		health, err := fast.ScenariosIdempotent(ctx, id, "idem-1", Payment{Amount: 5})
		must(t, err)
		expect(t, health.Status, "attempts=2;key=idem-1")
		seen := attemptsOf(t, id)
		expect(t, seen.Attempts, 2)
		expect(t, seen.Keys, []string{"idem-1", "idem-1"})
	})
}

func TestScenarioErrors(t *testing.T) {
	ctx := context.Background()

	t.Run("status codes", func(t *testing.T) {
		sentinels := map[int]error{
			400: ErrBadRequest, 401: ErrUnauthorized, 403: ErrForbidden,
			404: ErrNotFound, 409: ErrConflict, 422: ErrUnprocessableEntity,
		}
		for _, code := range []int{400, 401, 403, 404, 409, 422} {
			var requests atomic.Int32
			client := scenarioClient(Options{Middleware: []Middleware{countRequests(&requests)}})
			health, err := client.ErrorsAPI().RetrieveScenarioStatus(ctx, int32(code))
			expect(t, health == nil, true)
			apiErr := apiError(t, err)
			expect(t, apiErr.StatusCode, code)
			expect(t, errors.Is(err, sentinels[code]), true)
			expect(t, errors.Is(err, ErrServer), false)
			expect(t, apiErr.RequestID(), "req_mock")
			body, ok := apiErr.Body.(*Error)
			if !ok || body.Error != "status "+strconv.Itoa(code) || body.Code == nil || int(*body.Code) != code {
				t.Fatalf("%d: body %#v", code, apiErr.Body)
			}
			detail := apiErr.Detail()
			expect(t, detail.Error, body.Error)
			expect(t, requests.Load(), int32(1))
		}
	})
	t.Run("an error from a later page", func(t *testing.T) {
		var requests atomic.Int32
		client := scenarioClient(Options{Middleware: []Middleware{countRequests(&requests)}})
		pager := client.ErrorsAPI().ListScenariosPagesAutoPaging(ctx, nil)
		var got []string
		var failure error
		for item, err := range pager.All() {
			if err != nil {
				failure = err
				continue
			}
			got = append(got, item.ID)
		}
		expect(t, got, []string{"p1", "p2"})
		apiErr := apiError(t, failure)
		expect(t, apiErr.StatusCode, 409)
		expect(t, errors.Is(failure, ErrConflict), true)
		body, ok := apiErr.Body.(*Error)
		if !ok || body.Error != "page_gone" {
			t.Fatalf("body %#v", apiErr.Body)
		}
		// The error is not swallowed and the iteration does not loop.
		expect(t, pager.Next(), false)
		expect(t, pager.Err() == failure, true)
		expect(t, requests.Load(), int32(2))

		page, err := client.ErrorsAPI().ListScenariosPages(ctx, nil)
		must(t, err)
		expect(t, page.HasNextPage(), true)
		next, err := page.NextPage(ctx)
		expect(t, next == nil, true)
		expect(t, errors.Is(err, ErrConflict), true)
	})
	t.Run("an error from the first page", func(t *testing.T) {
		t.Setenv(APIKeyEnv, "")
		client := New("", &Options{ServerURL: os.Getenv("FEATURES_URL")})
		pager := client.Widgets().ListAutoPaging(ctx, nil)
		expect(t, pager.Next(), false)
		expect(t, errors.Is(pager.Err(), ErrUnauthorized), true)
		_, err := client.Widgets().List(ctx, nil)
		expect(t, errors.Is(err, ErrUnauthorized), true)
	})
}

func TestScenarioSSE(t *testing.T) {
	ctx := context.Background()
	streaming := scenarioClient(Options{}).Streaming()

	stream, err := streaming.RetrieveScenariosSse(ctx)
	must(t, err)
	defer stream.Close()
	var events []SSEEvent
	for stream.Next() {
		events = append(events, stream.Event())
	}
	must(t, stream.Err())
	// The comment line yields no event; the id persists, the retry applies to its own event.
	expect(t, events, []SSEEvent{
		{Event: "message", Data: "first"},
		{Event: "tick", Data: "line1\nline2", ID: "7"},
		{Event: "message", Data: `{"n": 3}`, ID: "7", Retry: 2500 * time.Millisecond},
		{Event: "message", Data: "tail", ID: "7"},
	})

	var requests atomic.Int32
	denied := scenarioClient(Options{Middleware: []Middleware{countRequests(&requests)}}).Streaming()
	refused, err := denied.RetrieveScenariosSseError(ctx)
	expect(t, refused == nil, true)
	apiErr := apiError(t, err)
	expect(t, apiErr.StatusCode, 403)
	expect(t, errors.Is(err, ErrForbidden), true)
	body, ok := apiErr.Body.(*Error)
	if !ok || body.Error != "forbidden" {
		t.Fatalf("body %#v", apiErr.Body)
	}
	expect(t, requests.Load(), int32(1))
}
