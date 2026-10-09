package tags

import (
	"encoding/json"
	"reflect"
	"strings"
	"testing"
)

// Union variants sharing a tag, as OpenAI's InputItem, in the SDK generated from
// tests/sdk/rust/tags/openapi.yaml.

func turn(t *testing.T, in string) Turn {
	t.Helper()
	var value Turn
	if err := json.Unmarshal([]byte(in), &value); err != nil {
		t.Fatalf("%s: %v", in, err)
	}
	return value
}

func TestVariantsSharingATagAreSentWithIt(t *testing.T) {
	for _, value := range []Turn{
		NewTurnMessage(SimpleTurn{Content: "hi"}),
		NewTurnUserTurn(UserTurn{Parts: []string{"a"}}),
		NewTurnAssistantTurn(AssistantTurn{ID: "m1", Parts: []string{}}),
	} {
		out, err := json.Marshal(value)
		if err != nil {
			t.Fatal(err)
		}
		if !strings.Contains(string(out), `"type":"message"`) {
			t.Errorf("%s: not sent as a message", out)
		}
	}
}

func TestASharedTagDecodesAsTheVariantTheDataFitsBest(t *testing.T) {
	if got := turn(t, `{"type":"message","content":"hi"}`); got.Type != TurnMessage || got.Message.Content != "hi" {
		t.Errorf("simple: %+v", got)
	}
	if got := turn(t, `{"type":"message","role":"user","parts":["a"]}`); got.Type != TurnUserTurn || got.UserTurn.Parts[0] != "a" {
		t.Errorf("user: %+v", got)
	}
	if got := turn(t, `{"type":"message","id":"m1","parts":[]}`); got.Type != TurnAssistantTurn || got.AssistantTurn.ID != "m1" {
		t.Errorf("assistant: %+v", got)
	}
	var value Turn
	err := json.Unmarshal([]byte(`{"type":"message"}`), &value)
	if err == nil || !strings.Contains(err.Error(), "content") {
		t.Errorf("err = %v, want the first variant's", err)
	}
}

func TestSharedTagsRoundTrip(t *testing.T) {
	in := `{"turns":[{"type":"message","content":"hi"},{"type":"message","role":"user","parts":["a"]},` +
		`{"type":"message","id":"m1","parts":[]},{"type":"tool","output":"42"}]}`
	var conversation Conversation
	if err := json.Unmarshal([]byte(in), &conversation); err != nil {
		t.Fatal(err)
	}
	out, err := json.Marshal(conversation)
	if err != nil {
		t.Fatal(err)
	}
	var got, want any
	_ = json.Unmarshal(out, &got)
	_ = json.Unmarshal([]byte(in), &want)
	if !reflect.DeepEqual(got, want) {
		t.Errorf("got %s, want %s", out, in)
	}
}
