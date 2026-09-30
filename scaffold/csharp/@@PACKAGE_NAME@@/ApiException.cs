using System;
using System.Net;
using System.Net.Http.Headers;

namespace @@PACKAGE_NAME@@;

/// <summary>Thrown for every response with a non-2xx status code.</summary>
public class ApiException : Exception
{
    public ApiException(HttpStatusCode statusCode, string body, HttpResponseHeaders headers)
        : base($"@@CLIENT_NAME@@ API error (status {(int)statusCode}): {body}")
    {
        StatusCode = statusCode;
        Body = body;
        Headers = headers;
    }

    public HttpStatusCode StatusCode { get; }

    /// <summary>The raw response body.</summary>
    public string Body { get; }

    public HttpResponseHeaders Headers { get; }

    /// <summary>Called by the SDK for every error response: return a subclass to map specific errors.
    /// A plain <see cref="ApiException"/> becomes the status class, such as <see cref="NotFoundException"/>.</summary>
    internal static ApiException FromResponse(
        HttpStatusCode statusCode,
        string body,
        HttpResponseHeaders headers
    ) => new(statusCode, body, headers);
}
