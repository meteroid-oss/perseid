package com.torture;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertThrows;
import static org.junit.jupiter.api.Assertions.assertTrue;

import com.sun.net.httpserver.HttpServer;
import com.torture.api.ThingsListThingsOptions;
import com.torture.exceptions.ApiException;
import com.torture.exceptions.ApiTimeoutException;
import com.torture.exceptions.InternalServerException;
import com.torture.exceptions.NotFoundException;
import com.torture.exceptions.UnprocessableEntityException;
import com.torture.models.ValidationError;
import com.torture.models.Kind;
import com.torture.models.ThingPatch;
import java.net.InetSocketAddress;
import java.nio.charset.StandardCharsets;
import java.time.Duration;
import java.time.OffsetDateTime;
import java.util.ArrayDeque;
import java.util.ArrayList;
import java.util.Deque;
import java.util.List;
import okhttp3.OkHttpClient;
import org.junit.jupiter.api.AfterEach;
import org.junit.jupiter.api.BeforeEach;
import org.junit.jupiter.api.Test;

class HttpTest {
    private static final String THING =
            "{\"id\":\"i\",\"name\":\"n\",\"created_at\":\"2024-01-02T03:04:05Z\",\"kind\":\"alpha\",\"count\":1,"
                    + "\"nullable_required\":null,\"tags\":[],\"metadata\":{},\"attrs\":{}}";

    private HttpServer server;
    private final List<String> requests = new ArrayList<>();
    private final List<String> idempotencyKeys = new ArrayList<>();
    private final List<String> traces = new ArrayList<>();
    private String errorBody = "{\"title\":\"no\"}";
    private final Deque<Integer> statuses = new ArrayDeque<>();
    private String retryAfter;

    @BeforeEach
    void start() throws Exception {
        server = HttpServer.create(new InetSocketAddress("127.0.0.1", 0), 0);
        server.createContext("/", exchange -> {
            requests.add(exchange.getRequestMethod() + " " + exchange.getRequestURI().getRawPath()
                    + (exchange.getRequestURI().getRawQuery() == null ? "" : "?" + exchange.getRequestURI().getRawQuery()));
            idempotencyKeys.add(exchange.getRequestHeaders().getFirst("idempotency-key"));
            traces.add(exchange.getRequestHeaders().getFirst("x-trace"));
            int status = statuses.isEmpty() ? 200 : statuses.poll();
            if (retryAfter != null && status != 200) {
                exchange.getResponseHeaders().add("Retry-After", retryAfter);
            }
            byte[] body = (status == 200 ? THING : errorBody).getBytes(StandardCharsets.UTF_8);
            exchange.sendResponseHeaders(status, body.length);
            exchange.getResponseBody().write(body);
            exchange.close();
        });
        server.start();
    }

    @AfterEach
    void stop() {
        server.stop(0);
    }

    private Torture client() {
        TortureOptions options = new TortureOptions();
        options.setServerUrl("http://127.0.0.1:" + server.getAddress().getPort() + "/v1");
        options.setRetrySchedule(List.of(1L, 1L));
        return new Torture("token", options);
    }

    @Test
    void pathsKeepTheServerPrefixAndEscapeParameters() throws Exception {
        client().getThings().getThing("a/b?c=d");
        client().getThings().getThing("100%");
        assertEquals(List.of("GET /v1/things/a%2Fb%3Fc=d", "GET /v1/things/100%25"), requests);
        assertThrows(IllegalArgumentException.class, () -> client().getThings().getThing(".."));
    }

    @Test
    void queryParametersAreEncoded() throws Exception {
        client().getThings().listThings(new ThingsListThingsOptions()
                .ids(List.of("x", "y"))
                .kind(Kind.BETA_2)
                .since(OffsetDateTime.parse("2024-01-02T03:04:00Z")));
        assertEquals("GET /v1/things?ids=x&ids=y&kind=beta-2&since=2024-01-02T03%3A04%3A00Z", requests.get(0));
    }

    @Test
    void rateLimitedRequestsAreRetriedAfterTheirDelay() throws Exception {
        statuses.add(429);
        retryAfter = "0";
        assertEquals("n", client().getThings().getThing("t").getName());
        assertEquals(2, requests.size());
    }

    @Test
    void postsAreRetriedWithTheSameIdempotencyKey() throws Exception {
        statuses.add(503);
        client().getThings().createThing(new com.torture.models.ThingCreate().name("n").kind(Kind.ALPHA));
        assertEquals(2, idempotencyKeys.size());
        assertTrue(idempotencyKeys.get(0).startsWith("auto_"));
        assertEquals(idempotencyKeys.get(0), idempotencyKeys.get(1));
    }

    @Test
    void patchesWithoutAnIdempotencyKeyAreNotRetried() {
        statuses.add(503);
        ApiException error = assertThrows(
                ApiException.class, () -> client().getThings().updateThing("t", new ThingPatch()));
        assertEquals(503, error.getCode());
        assertEquals(1, requests.size());
    }

    @Test
    void aLongRetryAfterReturnsTheError() {
        statuses.add(429);
        retryAfter = "3600";
        ApiException error = assertThrows(ApiException.class, () -> client().getThings().getThing("t"));
        assertEquals(429, error.getCode());
        assertEquals("3600", error.getHeaders().get("retry-after"));
        assertEquals(1, requests.size());
    }

    @Test
    void anInjectedClientIsUsed() throws Exception {
        List<String> seen = new ArrayList<>();
        TortureOptions options = new TortureOptions();
        options.setServerUrl("http://127.0.0.1:" + server.getAddress().getPort());
        options.setHttpClient(new OkHttpClient.Builder()
                .addInterceptor(chain -> {
                    seen.add(chain.request().url().encodedPath());
                    return chain.proceed(chain.request());
                })
                .build());
        new Torture("token", options).getThings().getThing("t");
        assertEquals(List.of("/things/t"), seen);
    }

    @Test
    void timeoutsApplyToEachAttempt() {
        server.removeContext("/");
        server.createContext("/", exchange -> {
            try {
                Thread.sleep(2000);
            } catch (InterruptedException ignored) {
            }
            exchange.close();
        });
        TortureOptions options = new TortureOptions();
        options.setServerUrl("http://127.0.0.1:" + server.getAddress().getPort());
        options.setTimeout(Duration.ofMillis(200));
        options.setRetrySchedule(List.of());
        ApiTimeoutException error =
                assertThrows(ApiTimeoutException.class, () -> new Torture("token", options).getThings().getThing("t"));
        assertEquals(0, error.getCode());
    }

    @Test
    void requestOptionsApplyToOneCall() {
        statuses.add(503);
        RequestOptions once =
                RequestOptions.builder().header("x-trace", "t1").maxRetries(0).idempotencyKey("k1").build();
        InternalServerException error = assertThrows(
                InternalServerException.class,
                () -> client().getThings().createThing(new com.torture.models.ThingCreate().name("n"), once));
        assertEquals(503, error.getCode());
        assertEquals(List.of("t1"), traces);
        assertEquals(List.of("k1"), idempotencyKeys);
        client().getThings().listThings(RequestOptions.builder().header("x-trace", "t2").build());
        assertEquals(List.of("t1", "t2"), traces);
    }

    @Test
    void aPerCallTimeoutOverridesTheClients() {
        server.removeContext("/");
        server.createContext("/", exchange -> {
            try {
                Thread.sleep(2000);
            } catch (InterruptedException ignored) {
            }
            exchange.close();
        });
        RequestOptions fast = RequestOptions.builder().timeout(Duration.ofMillis(200)).maxRetries(0).build();
        assertThrows(ApiTimeoutException.class, () -> client().getThings().getThing("t", fast));
    }

    @Test
    void errorsHaveAStatusClassAndATypedBody() {
        statuses.add(404);
        NotFoundException missing = assertThrows(NotFoundException.class, () -> client().getThings().getThing("t"));
        assertEquals(404, missing.getCode());

        statuses.add(422);
        errorBody = "{\"message\":\"bad name\",\"fields\":{\"name\":[\"too short\"]}}";
        UnprocessableEntityException invalid = assertThrows(
                UnprocessableEntityException.class,
                () -> client().getThings().createThing(new com.torture.models.ThingCreate().name("n")));
        ValidationError body = invalid.getError(ValidationError.class).orElseThrow();
        assertEquals("bad name", body.getMessage());
        assertEquals(List.of("too short"), body.getFields().get("name"));
        assertTrue(invalid.getMessage().contains("422") && invalid.getMessage().contains("bad name"));
    }
}
