package @@PACKAGE_NAME@@

import "fmt"

// APIError is returned for every non-2xx response.
type APIError struct {
	StatusCode int
	RawBody    []byte
}

func newAPIError(statusCode int, body []byte) *APIError {
	return &APIError{StatusCode: statusCode, RawBody: body}
}

func (e *APIError) Error() string {
	return fmt.Sprintf("@@PACKAGE_NAME@@: API error (status %d): %s", e.StatusCode, e.RawBody)
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
