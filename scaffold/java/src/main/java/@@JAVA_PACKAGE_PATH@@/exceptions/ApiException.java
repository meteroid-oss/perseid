package @@JAVA_PACKAGE@@.exceptions;

import @@JAVA_PACKAGE@@.Utils;
import com.fasterxml.jackson.databind.ObjectMapper;
import okhttp3.Headers;

public class ApiException extends Exception implements Utils.WithResponseHeaders {
    private static final long serialVersionUID = 1L;

    private final int code;
    private final String responseBody;
    private transient Headers headers = Headers.of();

    public ApiException(String message, int code, String responseBody) {
        super(message);
        this.code = code;
        this.responseBody = responseBody;
    }

    public ApiException(String message, int code, String responseBody, ObjectMapper mapper) {
        this(message, code, responseBody);
    }

    public int getCode() { return code; }

    public String getResponseBody() { return responseBody; }

    /** The response headers, such as the request id to quote to support. */
    public Headers getHeaders() { return headers; }

    @Override
    public void setResponseHeaders(Headers headers) { this.headers = headers; }
}
