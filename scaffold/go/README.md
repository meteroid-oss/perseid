# @@CLIENT_NAME@@ Go SDK

@@DESCRIPTION@@

```sh
go get @@GO_MODULE@@
```

```go
import @@PACKAGE_NAME@@ "@@GO_MODULE@@"

client := @@PACKAGE_NAME@@.New("your-api-key", nil)
```

Every API area hangs off the client as an accessor method, and every method takes a
`context.Context` first and trailing options such as `@@PACKAGE_NAME@@.WithTimeout(time.Minute)`
or `@@PACKAGE_NAME@@.WithIdempotencyKey(key)`. Optional fields are pointers (`@@PACKAGE_NAME@@.Ptr(v)`).

A non-2xx response is an `*@@PACKAGE_NAME@@.APIError`:

```go
var apiErr *@@PACKAGE_NAME@@.APIError
switch {
case errors.Is(err, @@PACKAGE_NAME@@.ErrNotFound):
	// ...
case errors.As(err, &apiErr):
	log.Printf("status %d, request %s", apiErr.StatusCode, apiErr.RequestID())
}
```

Requires Go 1.22 or later.

- Source: @@REPOSITORY@@
- License: @@LICENSE@@
