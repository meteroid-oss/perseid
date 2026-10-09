using System;
using System.Net;
using System.Net.Http.Headers;
using System.Text.Json.Serialization.Metadata;

namespace @@PACKAGE_NAME@@;

/// <summary>
/// Thrown for every response with a non-2xx status code, as the subclass of its status, such as
/// <see cref="NotFoundException"/> or <see cref="RateLimitException"/>.
/// </summary>
public class ApiException : @@CLIENT_NAME@@Exception
{
    private readonly Lazy<object?> _error;

    /// <summary>Creates the exception of an error response.</summary>
    /// <param name="statusCode">The status code.</param>
    /// <param name="body">The raw response body.</param>
    /// <param name="headers">The response headers.</param>
    public ApiException(HttpStatusCode statusCode, string body, HttpResponseHeaders headers)
        : base(ApiExceptionExtensions.Describe(statusCode, body))
    {
        StatusCode = statusCode;
        Body = body;
        Headers = headers;
        _error = new(() => ApiExceptionExtensions.DecodeError(Body, ErrorType));
    }

    /// <summary>The status code.</summary>
    public HttpStatusCode StatusCode { get; }

    /// <summary>The raw response body.</summary>
    public string Body { get; }

    /// <summary>The response headers.</summary>
    public HttpResponseHeaders Headers { get; }

    /// <summary>
    /// The body parsed as the error schema the operation declares for this status, else as a
    /// <see cref="System.Text.Json.JsonElement"/>; <c>null</c> when it is not JSON. Match it with
    /// <c>is</c>, or read it as a given model with <c>GetError&lt;T&gt;()</c>.
    /// </summary>
    public object? Error => _error.Value;

    /// <summary>The <c>x-request-id</c> (or <c>request-id</c>) header, to quote to support.</summary>
    public string? RequestId => ApiExceptionExtensions.RequestId(Headers);

    /// <summary>The error schema the operation declares for this status, set by the SDK.</summary>
    internal JsonTypeInfo? ErrorType { get; set; }

    /// <summary>Called by the SDK for every error response: return a subclass to map specific errors.
    /// A plain <see cref="ApiException"/> becomes the status class, such as <see cref="NotFoundException"/>.</summary>
    internal static ApiException FromResponse(
        HttpStatusCode statusCode,
        string body,
        HttpResponseHeaders headers
    ) => new(statusCode, body, headers);
}
