package petstore_test

import (
	"context"
	"errors"
	"net/http"
	"strings"
	"testing"

	"github.com/petstore/petstore-go"
)

func answering(body string) *petstore.Client {
	return petstore.New("token", &petstore.Options{Middleware: []petstore.Middleware{
		func(http.RoundTripper) http.RoundTripper {
			return petstore.RoundTripperFunc(func(req *http.Request) (*http.Response, error) {
				return respond(req, body), nil
			})
		},
	}})
}

func TestResponsesMissingARequiredPropertyAreDecodeErrors(t *testing.T) {
	for _, body := range []string{
		`{"id":"1","created_at":"2024-01-01T00:00:00Z"}`,
		`{"id":"1","name":null,"created_at":"2024-01-01T00:00:00Z"}`,
		`{"data":[{"id":"1","name":"Rex"}]}`,
	} {
		var err error
		if strings.HasPrefix(body, `{"data"`) {
			_, err = answering(body).Pets().List(context.Background(), nil)
		} else {
			_, err = answering(body).Pets().Retrieve(context.Background(), "1")
		}
		var decodeErr *petstore.DecodeError
		if !errors.As(err, &decodeErr) {
			t.Fatalf("%s: err %v, want a *DecodeError", body, err)
		}
		if !strings.Contains(err.Error(), "required property") {
			t.Fatalf("%s: err %q does not name the property", body, err)
		}
	}
}

func TestRequiredPropertiesMatchCaseInsensitivelyLikeEncodingJSON(t *testing.T) {
	pet, err := answering(`{"ID":"1","Name":"Rex","created_at":"2024-01-01T00:00:00Z"}`).Pets().Retrieve(context.Background(), "1")
	if err != nil || pet.Name != "Rex" {
		t.Fatalf("pet %v, err %v", pet, err)
	}
}

func TestWrongTypesAreDecodeErrors(t *testing.T) {
	_, err := answering(`{"id":"1","name":42,"created_at":"2024-01-01T00:00:00Z"}`).Pets().Retrieve(context.Background(), "1")
	var decodeErr *petstore.DecodeError
	if !errors.As(err, &decodeErr) {
		t.Fatalf("err %v, want a *DecodeError", err)
	}
}
