package unionssdk

import (
	"encoding/json"
	"reflect"
	"testing"
	"time"
)

// The decoding and encoding of the unions and open enums of the SDK generated from
// tests/fixtures/edge-unions.yaml, which has the shapes the OpenAI spec uses.

func sameJSON(t *testing.T, got, want string) {
	t.Helper()
	var g, w any
	if err := json.Unmarshal([]byte(got), &g); err != nil {
		t.Fatalf("%s: %v", got, err)
	}
	if err := json.Unmarshal([]byte(want), &w); err != nil {
		t.Fatalf("%s: %v", want, err)
	}
	if !reflect.DeepEqual(g, w) {
		t.Errorf("got %s, want %s", got, want)
	}
}

// roundTrip decodes in as a T and checks that it encodes back to the same JSON.
func roundTrip[T any](t *testing.T, in string) T {
	t.Helper()
	var v T
	if err := json.Unmarshal([]byte(in), &v); err != nil {
		t.Fatalf("%s: %v", in, err)
	}
	out, err := json.Marshal(v)
	if err != nil {
		t.Fatalf("%s: %v", in, err)
	}
	sameJSON(t, string(out), in)
	return v
}

func TestUnionsSharingAJSONTypeAreToldApartByTheirItems(t *testing.T) {
	for _, tc := range []struct{ in, kind string }{
		{`"hi"`, "string"},
		{`["a","b"]`, "array_of_strings"},
		{`[1,2]`, "array_of_integers"},
		{`[[1],[2,3]]`, "array_of_integer_arrays"},
		{`[]`, "array_of_strings"},
	} {
		u := roundTrip[CreateCompletionRequestPrompt](t, tc.in)
		if u.Kind() != tc.kind || u.Raw() != nil {
			t.Errorf("%s: kind %q, raw %s", tc.in, u.Kind(), u.Raw())
		}
	}

	u := roundTrip[CreateCompletionRequestPrompt](t, `[[1],[2,3]]`)
	if !reflect.DeepEqual(u.OfArrayOfIntegerArrays, [][]int64{{1}, {2, 3}}) {
		t.Errorf("%+v", u)
	}
	if tokens, ok := u.AsArrayOfIntegerArrays(); !ok || len(tokens) != 2 {
		t.Errorf("AsArrayOfIntegerArrays: %v %v", tokens, ok)
	}
	if _, ok := u.AsString(); ok {
		t.Error("a token array is not a string")
	}

	// A first item that fits but a later one that does not leaves the value undecoded.
	for _, in := range []string{`["a",1]`, `true`, `{"a":1}`} {
		u := roundTrip[CreateCompletionRequestPrompt](t, in)
		if u.Kind() != "" || string(u.Raw()) != in {
			t.Errorf("%s: kind %q, raw %s", in, u.Kind(), u.Raw())
		}
	}
	var none CreateCompletionRequestPrompt
	if err := json.Unmarshal([]byte(`null`), &none); err != nil || none.Kind() != "" || none.Raw() != nil {
		t.Errorf("null: %+v %v", none, err)
	}

	// Encoding picks the variant the caller set.
	out, err := json.Marshal(NewCreateCompletionRequestPromptFromArrayOfIntegers([]int64{1, 2}))
	if err != nil {
		t.Fatal(err)
	}
	sameJSON(t, string(out), `[1,2]`)
	out, err = json.Marshal(NewCreateCompletionRequestPromptFromString("x"))
	if err != nil {
		t.Fatal(err)
	}
	sameJSON(t, string(out), `"x"`)

	request := roundTrip[CreateCompletionRequest](t, `{"model":"m","prompt":["a"],"stop":["x","y"]}`)
	if request.Prompt.OfArrayOfStrings == nil || request.Stop == nil || request.Stop.OfArrayOfStrings == nil {
		t.Errorf("%+v", request)
	}
}

func TestDateTimeFallsBackToTheFreeFormString(t *testing.T) {
	created := roundTrip[CompletionCreatedAt](t, `"2024-01-02T03:04:05Z"`)
	if created.OfDateTime == nil || !created.OfDateTime.Equal(time.Date(2024, 1, 2, 3, 4, 5, 0, time.UTC)) || created.OfString != nil {
		t.Errorf("%+v", created)
	}
	free := roundTrip[CompletionCreatedAt](t, `"yesterday"`)
	if free.OfString == nil || *free.OfString != "yesterday" || free.OfDateTime != nil {
		t.Errorf("%+v", free)
	}
}

func TestScalarsAndTypeArrays(t *testing.T) {
	c := roundTrip[Completion](t, `{"id":"c1","model":"alpha-1","created_at":"2024-01-02T03:04:05Z","choices":[],"score":1,"ratio":2.5,"stop":"x"}`)
	if c.Score == nil || *c.Score != 1 || c.Ratio == nil || *c.Ratio != 2.5 {
		t.Errorf("integer|number is a float: %+v", c)
	}
	if c.Stop == nil || c.Stop.OfString == nil || *c.Stop.OfString != "x" {
		t.Errorf("[string, integer, null]: %+v", c.Stop)
	}
	c = roundTrip[Completion](t, `{"id":"c1","model":"alpha-1","created_at":"x","choices":[],"stop":3}`)
	if c.Stop == nil || c.Stop.OfInteger == nil || *c.Stop.OfInteger != 3 {
		t.Errorf("%+v", c.Stop)
	}
}

func TestObjectUnionsWithoutADiscriminatorAreDecodedByBestMatch(t *testing.T) {
	allowed := roundTrip[ToolChoice](t, `{"mode":"auto","tools":[{"type":"x"}]}`)
	if allowed.OfAllowedTools == nil || allowed.OfHostedTool != nil || allowed.OfFunctionTool != nil {
		t.Errorf("%+v", allowed)
	}
	hosted := roundTrip[ToolChoice](t, `{"type":"file_search"}`)
	if hosted.OfHostedTool == nil || hosted.OfAllowedTools != nil {
		t.Errorf("%+v", hosted)
	}
	function := roundTrip[ToolChoice](t, `{"name":"f","arguments":"{}"}`)
	if function.OfFunctionTool == nil || function.OfFunctionTool.Name != "f" {
		t.Errorf("%+v", function)
	}
	mode := roundTrip[ToolChoice](t, `"auto"`)
	if mode.Raw() != nil || mode.Kind() == "" {
		t.Errorf("the string variant: %+v", mode)
	}
	unknown := roundTrip[ToolChoice](t, `{"other":1}`)
	if unknown.Kind() != "" || string(unknown.Raw()) != `{"other":1}` {
		t.Errorf("no variant fits: %+v", unknown)
	}
	if tool, ok := unknown.AsHostedTool(); ok && tool.Type != "" {
		t.Errorf("an object of no variant is not a hosted tool: %+v", tool)
	}

	response := roundTrip[Response](t, `{"id":"r","model":"m","tool_choice":{"name":"f"},"output":[{"type":"web_search"},{"name":"g"}]}`)
	if response.ToolChoice == nil || response.ToolChoice.OfFunctionTool == nil || len(response.Output) != 2 {
		t.Errorf("%+v", response)
	}
	if response.Output[0].OfHostedTool == nil || response.Output[1].OfFunctionTool == nil {
		t.Errorf("%+v", response.Output)
	}
}

func TestUnionBodiesAreTyped(t *testing.T) {
	plain := roundTrip[CreateTranscriptionResponse](t, `{"text":"hello"}`)
	if plain.OfTranscription == nil || plain.OfTranscriptionVerbose != nil {
		t.Errorf("%+v", plain)
	}
	verbose := roundTrip[CreateTranscriptionResponse](t, `{"text":"hello","duration":1.5,"language":"en"}`)
	if verbose.OfTranscriptionVerbose == nil || verbose.OfTranscription != nil {
		t.Errorf("a verbose body is not the plain one: %+v", verbose)
	}
	named := roundTrip[TranscriptionResult](t, `{"text":"hello","duration":1.5,"language":"en"}`)
	if named.OfTranscriptionVerbose == nil {
		t.Errorf("%+v", named)
	}

	text := roundTrip[CreateGradeRequest](t, `{"text":"a","reference":"b"}`)
	score := roundTrip[CreateGradeRequest](t, `{"score":0.5}`)
	if text.OfGradeByText == nil || score.OfGradeByScore == nil || text.OfGradeByScore != nil {
		t.Errorf("%+v %+v", text, score)
	}
	grade := roundTrip[Grade](t, `{"id":"g","result":{"score":2}}`)
	if grade.Result == nil || grade.Result.OfGradeByScore == nil {
		t.Errorf("%+v", grade)
	}
}

func TestInlineVariantsOfTaggedUnionsAreHoisted(t *testing.T) {
	part := roundTrip[ContentPart](t, `{"type":"text","text":"hi"}`)
	if part.Type != ContentPartText || part.Text == nil || part.Text.Text != "hi" {
		t.Errorf("%+v", part)
	}
	roundTrip[ContentPart](t, `{"type":"file","file":{"file_id":"f","filename":"a.txt"}}`)
	roundTrip[ContentPart](t, `{"type":"image_url","image_url":{"url":"u","detail":"low"}}`)

	out, err := json.Marshal(NewContentPartText(ContentPartTextVariant{Text: "x"}))
	if err != nil {
		t.Fatal(err)
	}
	sameJSON(t, string(out), `{"type":"text","text":"x"}`)
	out, err = json.Marshal(ContentPartTextVariant{Text: "x"})
	if err != nil {
		t.Fatal(err)
	}
	sameJSON(t, string(out), `{"type":"text","text":"x"}`)

	var unknown ContentPart
	if err := json.Unmarshal([]byte(`{"type":"audio","x":1}`), &unknown); err != nil {
		t.Fatal(err)
	}
	if unknown.IsKnown() || string(unknown.Raw()) != `{"type":"audio","x":1}` {
		t.Errorf("%+v", unknown)
	}

	request := roundTrip[CreateMessageRequest](t, `{"role":"user","content":[{"type":"text","text":"hi"}]}`)
	if request.Content.OfArrayOfContentParts == nil || request.Content.OfArrayOfContentParts[0].Text == nil {
		t.Errorf("%+v", request)
	}
	roundTrip[CreateMessageRequest](t, `{"role":"user","content":"hi"}`)
	roundTrip[Message](t, `{"id":"m","role":"user","content":[{"type":"text","text":"hi"}],"events":[{"type":"ping"}]}`)
}

func TestRefinedAliasVariantsResolveToTheirSchema(t *testing.T) {
	grader := roundTrip[EvalGrader](t, `{"type":"string_check","name":"n","input":"i","reference":"r","operation":"eq"}`)
	if string(grader.Type) != "string_check" || !grader.IsKnown() {
		t.Errorf("%+v", grader)
	}
	roundTrip[Eval](t, `{"id":"e","name":"n","graders":[{"type":"text_similarity","name":"n","input":"i","reference":"r","evaluation_metric":"bleu","pass_threshold":0.5}]}`)
}

func TestOpenEnumsKeepUnknownValues(t *testing.T) {
	if !ModelIDs("alpha-1").IsKnown() || ModelIDs("zeta").IsKnown() {
		t.Error("ModelIDs.IsKnown")
	}
	if len(AllVoiceValues) != 4 || len(AllIncludeValues) != 3 {
		t.Errorf("merged enums: %v %v", AllVoiceValues, AllIncludeValues)
	}
	if !VoiceWithOpen("coral").IsKnown() || VoiceWithOpen("whisper").IsKnown() {
		t.Error("VoiceWithOpen.IsKnown")
	}

	c := roundTrip[Completion](t, `{"id":"c1","model":"zeta","fallback_model":"chat-large","voice":"sage","voice_open":"whisper","include":["usage"],"created_at":"2024-01-02T03:04:05Z","choices":[{"index":0,"text":"t","finish_reason":"weird"}]}`)
	if string(c.Model) != "zeta" || c.Model.IsKnown() || c.FallbackModel == nil || *c.FallbackModel != "chat-large" {
		t.Errorf("%+v", c)
	}
}

func TestOneUntypableFieldDoesNotUntypeItsModel(t *testing.T) {
	u := roundTrip[Untypable](t, `{"name":"n","mixed":1,"count":2}`)
	if u.Name != "n" || string(u.Mixed) != "1" || u.Count == nil || *u.Count != 2 {
		t.Errorf("%+v", u)
	}
}

func TestRequiredOnlyAlternativesKeepTheStruct(t *testing.T) {
	image := roundTrip[ImageRef](t, `{"file_id":"f"}`)
	if image.FileID == nil || *image.FileID != "f" || image.ImageURL != nil {
		t.Errorf("%+v", image)
	}
	roundTrip[CreateFileBatchRequest](t, `{"file_ids":["a","b"]}`)
}
