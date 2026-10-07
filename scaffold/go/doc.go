// Package @@PACKAGE_NAME@@ is the Go SDK for the @@CLIENT_NAME@@ API.
//
// The entry point is [New], which returns a [Client]. Every API area hangs off
// the client as an accessor method, and every method takes a context first and
// trailing [RequestOption] values, such as [WithTimeout] or [WithResponseInto]:
//
//	client := @@PACKAGE_NAME@@.New("", nil) // reads @@ENV_PREFIX@@_API_KEY
//
// @@ENV_PREFIX@@_BASE_URL, or Options.ServerURL, overrides the API's default
// server. When the API declares none, calls fail with a [*RequestError] until
// one of them is set.
//
// Optional request fields and query parameters are pointers, see [Ptr]: a nil
// pointer means "not sent". Required fields and parameters are plain values.
// Models keep the properties this SDK version does not know in ExtraFields.
//
// Every error is an [SDKError]. One returned for a non-2xx response is an
// [*APIError]: errors.Is matches it against status sentinels such as
// [ErrNotFound], and its Body holds the decoded error body.
//
// List operations return their first page, which embeds the decoded response
// and holds its Items, HasNextPage and NextPage, and have a ...AutoPaging twin
// returning an [AutoPager] over every item. Event streams are a [Stream] of
// decoded events, or an [EventStream] of raw ones.
package @@PACKAGE_NAME@@
