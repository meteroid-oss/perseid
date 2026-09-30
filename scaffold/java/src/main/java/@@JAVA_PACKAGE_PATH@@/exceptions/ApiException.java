package @@JAVA_PACKAGE@@.exceptions;

import @@JAVA_INTERNAL_PACKAGE@@.Utils;
import com.fasterxml.jackson.databind.ObjectMapper;
import java.util.Optional;
import okhttp3.Headers;

/**
 * An error response of the API, or a failure to get one ({@link ApiConnectionException}, code 0).
 * Common statuses have their own subclass, such as {@link NotFoundException} or {@link
 * RateLimitException}.
 */
public class ApiException extends RuntimeException
        implements Utils.WithResponseHeaders, Utils.WithError {
    private static final long serialVersionUID = 1L;

    private final int code;
    private final String responseBody;
    private transient Headers headers = Headers.of();
    private transient Object error;

    public ApiException(String message, int code, String responseBody) {
        super(message);
        this.code = code;
        this.responseBody = responseBody;
    }

    public ApiException(String message, int code, String responseBody, ObjectMapper mapper) {
        this(message, code, responseBody);
    }

    /** The HTTP status, or 0 when there was no response. */
    public int getCode() {
        return code;
    }

    public String getResponseBody() {
        return responseBody;
    }

    /** The response headers. */
    public Headers getHeaders() {
        return headers;
    }

    /** The request id to quote to support, if the response has one. */
    public Optional<String> getRequestId() {
        String id = headers.get("x-request-id");
        return Optional.ofNullable(id != null ? id : headers.get("request-id"));
    }

    /** The body parsed as the schema the operation declares for this status, or null. */
    public Object getError() {
        return error;
    }

    /** The body as a {@code type}, if it parsed as one. */
    public <T> Optional<T> getError(Class<T> type) {
        return type.isInstance(error) ? Optional.of(type.cast(error)) : Optional.empty();
    }

    @Override
    public void setResponseHeaders(Headers headers) {
        this.headers = headers;
    }

    @Override
    public void setError(Object error) {
        this.error = error;
    }
}
