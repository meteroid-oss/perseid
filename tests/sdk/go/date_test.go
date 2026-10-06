package petstore_test

import (
	"encoding/json"
	"testing"
	"time"

	"github.com/petstore/petstore-go"
)

func TestDateRoundTripsAsPlainDate(t *testing.T) {
	var day petstore.Date
	if err := json.Unmarshal([]byte(`"2024-02-29"`), &day); err != nil {
		t.Fatal(err)
	}
	if day != (petstore.Date{Year: 2024, Month: time.February, Day: 29}) {
		t.Fatalf("decoded %+v", day)
	}
	out, err := json.Marshal(map[string]petstore.Date{"day": day})
	if err != nil || string(out) != `{"day":"2024-02-29"}` {
		t.Fatalf("encoded %s, %v", out, err)
	}
	late := time.Date(2024, 2, 29, 23, 30, 0, 0, time.FixedZone("UTC-5", -5*3600))
	if petstore.DateOf(late) != day || !day.In(time.UTC).Equal(time.Date(2024, 2, 29, 0, 0, 0, 0, time.UTC)) {
		t.Fatal("DateOf must keep the date of the time's own location")
	}
	if _, err := petstore.ParseDate("2024-02-30"); err == nil {
		t.Fatal("an impossible date must not parse")
	}
}
