package @@PACKAGE_NAME@@

import (
	"fmt"
	"net/http"
)

// APIError is returned for every non-2xx response. errors.Is matches it against
// status sentinels such as [ErrNotFound], and [ErrorBody] decodes its body.
type APIError struct {
	StatusCode int
	// Header holds the response headers, such as the request id to quote to support.
	Header  http.Header
	RawBody []byte
}

func newAPIError(statusCode int, body []byte) *APIError {
	return &APIError{StatusCode: statusCode, RawBody: body}
}

// setHeader is called by the SDK with the headers of the response.
func (e *APIError) setHeader(header http.Header) {
	e.Header = header
}

// RequestID returns the id the API gave the request, to quote to support.
func (e *APIError) RequestID() string {
	for _, name := range []string{"X-Request-Id", "Request-Id"} {
		if id := e.Header.Get(name); id != "" {
			return id
		}
	}
	return ""
}

func (e *APIError) Error() string {
	body := e.RawBody
	if len(body) > 512 {
		body = append(body[:512:512], "..."...)
	}
	return fmt.Sprintf("@@PACKAGE_NAME@@: API error (status %d): %s", e.StatusCode, body)
}

// TransportError is returned when no response was received.
type TransportError struct {
	Method string
	Path   string
	Err    error
}

func (e *TransportError) Error() string {
	return fmt.Sprintf("@@PACKAGE_NAME@@: %s %s: %v", e.Method, e.Path, e.Err)
}

func (e *TransportError) Unwrap() error { return e.Err }

// DecodeError is returned when a 2xx response body could not be decoded.
type DecodeError struct {
	StatusCode int
	RawBody    []byte
	Err        error
}

func (e *DecodeError) Error() string {
	return fmt.Sprintf("@@PACKAGE_NAME@@: decoding response body (status %d): %v", e.StatusCode, e.Err)
}

func (e *DecodeError) Unwrap() error { return e.Err }

// UnionError reports a tagged union that could not be encoded.
type UnionError struct {
	Union         string
	Discriminator string
	Reason        string
}

func (e *UnionError) Error() string {
	if e.Discriminator == "" {
		return fmt.Sprintf("@@PACKAGE_NAME@@: cannot encode %s: %s", e.Union, e.Reason)
	}
	return fmt.Sprintf("@@PACKAGE_NAME@@: cannot encode %s variant %q: %s", e.Union, e.Discriminator, e.Reason)
}
