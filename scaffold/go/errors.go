package @@PACKAGE_NAME@@

import (
	"fmt"
	"net/http"
)

// APIError is returned for every non-2xx response. errors.Is matches it against
// status sentinels such as [ErrNotFound], and [ErrorBody] decodes its body.
type APIError struct {
	// Method and Path are those of the operation, such as "GET" and "/pets/{id}".
	Method string
	Path   string

	StatusCode int
	// Header holds the response headers, such as the request id to quote to support.
	Header http.Header
	// Body is the body decoded as the error schema the operation declares for
	// the status, such as *ErrorResponse, else as plain JSON; nil when not JSON.
	Body    any
	RawBody []byte
}

func newAPIError(statusCode int, body []byte) *APIError {
	return &APIError{StatusCode: statusCode, RawBody: body}
}

// setHeader is called by the SDK with the headers of the response.
func (e *APIError) setHeader(header http.Header) {
	e.Header = header
}

// setRequest is called by the SDK with the operation that failed.
func (e *APIError) setRequest(method, path string) {
	e.Method, e.Path = method, path
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

// Error reads like "@@PACKAGE_NAME@@: POST /items: 429 Too Many Requests: " and the
// [APIError.Message] of the body, else the body itself, cut.
func (e *APIError) Error() string {
	return "@@PACKAGE_NAME@@: " + errorText(e.Method, e.Path, e.StatusCode, e.Message(), e.RawBody)
}

// TransportError is returned when no response was received: the connection
// failed or the call was canceled. A timeout is a [*TimeoutError] instead.
type TransportError struct {
	Method string
	Path   string
	Err    error
}

func (e *TransportError) Error() string {
	return fmt.Sprintf("@@PACKAGE_NAME@@: %s %s: %v", e.Method, e.Path, e.Err)
}

func (e *TransportError) Unwrap() error { return e.Err }

// DecodeError is returned when a 2xx response body, or an event of a stream,
// could not be decoded.
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
