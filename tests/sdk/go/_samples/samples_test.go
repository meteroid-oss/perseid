package samples

import (
	"bytes"
	"encoding/json"
	"fmt"
	"math/big"
	"os"
	"reflect"
	"sort"
	"strconv"
	"strings"
	"testing"
	"time"
)

// Decodes every schema-derived sample of every model with the generated types and encodes it
// again (see tests/sdk/run.sh, which writes samples.json with `perseid samples` and, with
// samples_registry.py, the registry of the types of the models).
//
// The only normalizations are the documented ones of the Go SDK:
//   - date-times compare as instants, since time.Time does not keep the spelling of the offset;
//   - an optional property that is null, an empty array or an empty object may be absent from the
//     output (`omitempty`: only PATCH bodies tell null from absent, through *Nullable);
//   - a number that is not an integer compares as a float64; integers must come back digit for
//     digit, whatever their size.
//
// Strings, decimals, unknown properties and everything else must come back exactly.

// samplesCodec decodes JSON text with the type of a model, then encodes the value.
type samplesCodec func(text string) ([]byte, error)

func samplesCodecOf[T any](text string) ([]byte, error) {
	var model T
	if err := json.Unmarshal([]byte(text), &model); err != nil {
		return nil, fmt.Errorf("decode failed: %w", err)
	}
	out, err := json.Marshal(&model)
	if err != nil {
		return nil, fmt.Errorf("encode failed: %w", err)
	}
	return out, nil
}

// samplesDecode reads JSON text keeping every number as its digits.
func samplesDecode(text []byte) (any, error) {
	decoder := json.NewDecoder(bytes.NewReader(text))
	decoder.UseNumber()
	var value any
	if err := decoder.Decode(&value); err != nil {
		return nil, err
	}
	return value, nil
}

func samplesKeys(object map[string]any) []string {
	keys := make([]string, 0, len(object))
	for key := range object {
		keys = append(keys, key)
	}
	sort.Strings(keys)
	return keys
}

// samplesOmittable reports whether an optional property with this value may be left out when encoding.
func samplesOmittable(value any) bool {
	switch value := value.(type) {
	case nil:
		return true
	case []any:
		return len(value) == 0
	case map[string]any:
		return len(value) == 0
	}
	return false
}

func samplesSameNumber(want, got json.Number) bool {
	if want == got {
		return true
	}
	wantRat, wantOK := new(big.Rat).SetString(string(want))
	gotRat, gotOK := new(big.Rat).SetString(string(got))
	if wantOK && gotOK && wantRat.Cmp(gotRat) == 0 {
		return true
	}
	if !strings.ContainsAny(string(want), ".eE") {
		return false
	}
	wantFloat, wantErr := strconv.ParseFloat(string(want), 64)
	gotFloat, gotErr := strconv.ParseFloat(string(got), 64)
	return wantErr == nil && gotErr == nil && wantFloat == gotFloat
}

// samplesDifference is the first difference between what was sampled and what came back, if any.
func samplesDifference(want, got any, path string, dropped *int) string {
	switch want := want.(type) {
	case map[string]any:
		other, ok := got.(map[string]any)
		if !ok {
			return fmt.Sprintf("%s: an object became %v", path, got)
		}
		for _, key := range samplesKeys(want) {
			found, present := other[key]
			switch {
			case present:
				if difference := samplesDifference(want[key], found, path+"/"+key, dropped); difference != "" {
					return difference
				}
			case samplesOmittable(want[key]):
				*dropped++
			default:
				return fmt.Sprintf("%s/%s: lost, was %v", path, key, want[key])
			}
		}
		for _, key := range samplesKeys(other) {
			if _, known := want[key]; !known {
				return fmt.Sprintf("%s/%s: added, is %v", path, key, other[key])
			}
		}
		return ""
	case []any:
		other, ok := got.([]any)
		if !ok {
			return fmt.Sprintf("%s: an array became %v", path, got)
		}
		if len(want) != len(other) {
			return fmt.Sprintf("%s: %d items became %d", path, len(want), len(other))
		}
		for i := range want {
			if difference := samplesDifference(want[i], other[i], fmt.Sprintf("%s/%d", path, i), dropped); difference != "" {
				return difference
			}
		}
		return ""
	case string:
		other, ok := got.(string)
		if !ok {
			return fmt.Sprintf("%s: %q became %v", path, want, got)
		}
		if want == other {
			return ""
		}
		wantTime, wantErr := time.Parse(time.RFC3339Nano, want)
		gotTime, gotErr := time.Parse(time.RFC3339Nano, other)
		if wantErr == nil && gotErr == nil && wantTime.Equal(gotTime) {
			return ""
		}
		return fmt.Sprintf("%s: %q became %q", path, want, other)
	case json.Number:
		other, ok := got.(json.Number)
		if !ok || !samplesSameNumber(want, other) {
			return fmt.Sprintf("%s: %s became %v", path, want, got)
		}
		return ""
	}
	if want != got {
		return fmt.Sprintf("%s: %v became %v", path, want, got)
	}
	return ""
}

func TestSamplesRoundTrip(t *testing.T) {
	raw, err := os.ReadFile("samples.json")
	if err != nil {
		t.Fatal(err)
	}
	var models map[string]struct {
		Kind    string `json:"kind"`
		Samples []struct {
			Name string          `json:"name"`
			JSON json.RawMessage `json:"json"`
		} `json:"samples"`
	}
	if err := json.Unmarshal(raw, &models); err != nil {
		t.Fatal(err)
	}
	if len(models) != len(samplesRegistry) {
		t.Fatalf("%d models, %d registered types", len(models), len(samplesRegistry))
	}

	schemas := make([]string, 0, len(models))
	for schema := range models {
		schemas = append(schemas, schema)
	}
	sort.Strings(schemas)
	checked, dropped := 0, 0
	for _, schema := range schemas {
		model := models[schema]
		codec, ok := samplesRegistry[schema]
		if !ok {
			t.Errorf("%s has no registered type", schema)
			continue
		}
		t.Run(schema, func(t *testing.T) {
			if len(model.Samples) == 0 {
				t.Fatal("the model has no sample")
			}
			for _, sample := range model.Samples {
				checked++
				fail := func(format string, args ...any) {
					t.Errorf("sample %q: %s\n    input:  %s", sample.Name, fmt.Sprintf(format, args...), sample.JSON)
				}
				encoded, err := codec(string(sample.JSON))
				if err != nil {
					fail("%v", err)
					continue
				}
				want, err := samplesDecode(sample.JSON)
				if err != nil {
					fail("the sample is not JSON: %v", err)
					continue
				}
				got, err := samplesDecode(encoded)
				if err != nil {
					fail("the output is not JSON: %v\n    output: %s", err, encoded)
					continue
				}
				if difference := samplesDifference(want, got, "", &dropped); difference != "" {
					fail("%s\n    output: %s", difference, encoded)
					continue
				}
				// What the SDK wrote is a fixed point: reading it back gives it again.
				again, err := codec(string(encoded))
				if err != nil {
					fail("own output: %v", err)
					continue
				}
				if second, _ := samplesDecode(again); !reflect.DeepEqual(got, second) {
					fail("not stable, %s was encoded again as %s", encoded, again)
				}
			}
		})
	}
	t.Logf("%d samples of %d models, %d empty or null optional properties left out", checked, len(models), dropped)
}
