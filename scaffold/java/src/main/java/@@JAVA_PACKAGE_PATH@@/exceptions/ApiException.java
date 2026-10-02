package @@JAVA_PACKAGE@@.exceptions;

import okhttp3.Headers;

import java.util.Optional;

/**
 * An error response of the API. Common statuses have their own subclass, such as {@link
 * NotFoundException} or {@link RateLimitException}. Failures to get a response are {@link
 * ApiConnectionException}s instead.
 */
public class ApiException extends @@CLIENT_NAME@@Exception {
    private static final long serialVersionUID = 1L;

    /** The HTTP status. */
    private final int statusCode;

    /** The response body. */
    private final String body;

    private final transient Headers headers;
    private final transient Object error;

    /**
     * An exception for an error response.
     *
     * @param message the message, with the status and the start of the body
     * @param statusCode the HTTP status
     * @param headers the response headers
     * @param body the response body
     * @param error the body parsed as the schema the operation declares for this status, else as
     *     JSON, or null
     */
    public ApiException(
            String message, int statusCode, Headers headers, String body, Object error) {
        super(message);
        this.statusCode = statusCode;
        this.headers = headers == null ? Headers.of() : headers;
        this.body = body == null ? "" : body;
        this.error = error;
    }

    /**
     * The HTTP status.
     *
     * @return the status code
     */
    public int statusCode() {
        return statusCode;
    }

    /**
     * The response body, as received.
     *
     * @return the body, empty when there was none
     */
    public String body() {
        return body;
    }

    /**
     * The response headers.
     *
     * @return the headers
     */
    public Headers headers() {
        return headers;
    }

    /**
     * The request id to quote to support, if the response has one.
     *
     * @return the {@code x-request-id} or {@code request-id} header
     */
    public Optional<String> requestId() {
        String id = headers.get("x-request-id");
        return Optional.ofNullable(id != null ? id : headers.get("request-id"));
    }

    /**
     * The body parsed as the schema the operation declares for this status, else as a {@code
     * JsonNode}.
     *
     * @return the parsed body, empty when it is not JSON
     */
    public Optional<Object> error() {
        return Optional.ofNullable(error);
    }

    /**
     * The body as a {@code type}, if it parsed as one.
     *
     * @param <T> the error model
     * @param type the class of the error model
     * @return the parsed body, empty when it is not a {@code type}
     */
    public <T> Optional<T> error(Class<T> type) {
        return type.isInstance(error) ? Optional.of(type.cast(error)) : Optional.empty();
    }
}
