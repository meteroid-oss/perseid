package com.torture;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertFalse;
import static org.junit.jupiter.api.Assertions.assertInstanceOf;
import static org.junit.jupiter.api.Assertions.assertThrows;
import static org.junit.jupiter.api.Assertions.assertTrue;

import com.fasterxml.jackson.databind.JsonNode;
import com.sun.net.httpserver.HttpServer;
import com.torture.api.ThingsListOptions;
import com.torture.exceptions.ApiConnectionException;
import com.torture.exceptions.ApiException;
import com.torture.exceptions.ApiTimeoutException;
import com.torture.exceptions.InternalServerException;
import com.torture.exceptions.InvalidDataException;
import com.torture.exceptions.NotFoundException;
import com.torture.exceptions.TortureException;
import com.torture.exceptions.UnprocessableEntityException;
import com.torture.streaming.EventStream;
import com.torture.models.ChatReply;
import com.torture.models.ChatRequest;
import com.torture.models.Kind;
import com.torture.models.Thing;
import com.torture.models.ThingCreate;
import com.torture.models.ThingPatch;
import com.torture.models.ValidationError;
import java.net.InetSocketAddress;
import java.net.ServerSocket;
import java.nio.charset.StandardCharsets;
import java.time.Duration;
import java.time.OffsetDateTime;
import java.util.ArrayDeque;
import java.util.ArrayList;
import java.util.Deque;
import java.util.List;
import java.util.Optional;
import java.util.concurrent.CompletionException;
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
    private final List<String> required = new ArrayList<>();
    private final List<String> bodies = new ArrayList<>();
    private int okStatus = 200;
    private String okType = "application/json";
    private String errorBody = "{\"title\":\"no\"}";
    private String okBody = THING;
    private final Deque<Integer> statuses = new ArrayDeque<>();
    private String retryAfter;
    private String retryAfterMs;

    @BeforeEach
    void start() throws Exception {
        server = HttpServer.create(new InetSocketAddress("127.0.0.1", 0), 0);
        server.createContext("/", exchange -> {
            requests.add(exchange.getRequestMethod() + " " + exchange.getRequestURI().getRawPath()
                    + (exchange.getRequestURI().getRawQuery() == null ? "" : "?" + exchange.getRequestURI().getRawQuery()));
            idempotencyKeys.add(exchange.getRequestHeaders().getFirst("idempotency-key"));
            traces.add(exchange.getRequestHeaders().getFirst("x-trace"));
            required.add(exchange.getRequestHeaders().getFirst("x-required"));
            bodies.add(new String(exchange.getRequestBody().readAllBytes(), StandardCharsets.UTF_8));
            int status = statuses.isEmpty() ? 200 : statuses.poll();
            if (retryAfter != null && status != 200) {
                exchange.getResponseHeaders().add("Retry-After", retryAfter);
            }
            if (retryAfterMs != null && status != 200) {
                exchange.getResponseHeaders().add("retry-after-ms", retryAfterMs);
            }
            exchange.getResponseHeaders().add("x-request-id", "req_" + requests.size());
            byte[] body = (status == 200 ? okBody : errorBody).getBytes(StandardCharsets.UTF_8);
            exchange.getResponseHeaders().add("content-type", status == 200 ? okType : "application/json");
            if (status == 200) {
                status = okStatus;
            }
            exchange.sendResponseHeaders(status, body.length == 0 ? -1 : body.length);
            exchange.getResponseBody().write(body);
            exchange.close();
        });
        server.start();
    }

    @AfterEach
    void stop() {
        server.stop(0);
    }

    private String url() {
        return "http://127.0.0.1:" + server.getAddress().getPort();
    }

    private Torture client() {
        return new Torture("token", TortureOptions.builder().baseUrl(url() + "/v1").retrySchedule(List.of(1L, 1L)).build());
    }

    private static ThingCreate create() {
        return ThingCreate.builder().name("n").kind(Kind.ALPHA).build();
    }

    @Test
    void withoutServersTheBaseUrlIsRequired() {
        IllegalStateException missing =
                assertThrows(IllegalStateException.class, () -> new Torture("token", TortureOptions.builder().build()));
        assertTrue(missing.getMessage().contains("baseUrl(...)"), missing.getMessage());
        assertTrue(missing.getMessage().contains("TORTURE_BASE_URL"), missing.getMessage());
        assertThrows(NoSuchFieldException.class, () -> Torture.class.getField("DEFAULT_BASE_URL"));
    }

    @Test
    void pathsKeepTheServerPrefixAndEscapeParameters() {
        client().things().retrieve("a/b?c=d");
        client().things().retrieve("100%");
        assertEquals(List.of("GET /v1/things/a%2Fb%3Fc=d", "GET /v1/things/100%25"), requests);
        assertThrows(IllegalArgumentException.class, () -> client().things().retrieve(".."));
        assertThrows(NullPointerException.class, () -> client().things().retrieve(null));
    }

    @Test
    void queryParametersAreEncodedAndRequiredOnesArePositional() {
        okBody = "{\"data\":[]}";
        client().things().list("needed", ThingsListOptions.builder()
                .ids(List.of("x", "y"))
                .kind(Kind.BETA_2)
                .since(OffsetDateTime.parse("2024-01-02T03:04:00Z"))
                .build());
        assertEquals("GET /v1/things?ids=x&ids=y&kind=beta-2&since=2024-01-02T03%3A04%3A00Z", requests.get(0));
        assertEquals(List.of("needed"), required);
    }

    @Test
    void rateLimitedRequestsAreRetriedAfterTheirDelay() {
        statuses.add(429);
        retryAfter = "0";
        assertEquals("n", client().things().retrieve("t").name());
        assertEquals(2, requests.size());
    }

    @Test
    void retryAfterMsWinsOverRetryAfter() {
        statuses.add(503);
        retryAfter = "3600";
        retryAfterMs = "5";
        assertEquals("n", client().things().retrieve("t").name());
        assertEquals(2, requests.size());
    }

    @Test
    void postsAreRetriedWithTheSameIdempotencyKey() {
        statuses.add(503);
        client().things().create(create());
        assertEquals(2, idempotencyKeys.size());
        assertTrue(idempotencyKeys.get(0).startsWith("auto_"));
        assertEquals(idempotencyKeys.get(0), idempotencyKeys.get(1));
    }

    @Test
    void patchesWithoutAnIdempotencyKeyAreNotRetried() {
        statuses.add(503);
        ApiException error = assertThrows(
                ApiException.class, () -> client().things().update("t", ThingPatch.builder().build()));
        assertEquals(503, error.statusCode());
        assertEquals(1, requests.size());
    }

    @Test
    void aLongRetryAfterFallsBackToTheBackoff() {
        statuses.addAll(List.of(429, 429, 429));
        retryAfter = "3600";
        ApiException error = assertThrows(ApiException.class, () -> client().things().retrieve("t"));
        assertEquals(429, error.statusCode());
        assertEquals("3600", error.headers().get("retry-after"));
        assertEquals("req_3", error.requestId().orElseThrow());
        assertEquals(3, requests.size());
    }

    @Test
    void everyServerErrorIsRetried() {
        statuses.add(501);
        assertEquals("n", client().things().retrieve("t").name());
        assertEquals(2, requests.size());
    }

    @Test
    void streamTwinsSetStreamWithoutTouchingTheBody() {
        okType = "text/event-stream";
        okBody = "data: {\"text\":\"hi\"}\n\ndata: [DONE]\n\n";
        List<String> texts = new ArrayList<>();
        try (EventStream<ChatReply> stream = client().chats().createStream(null)) {
            stream.forEach(reply -> texts.add(reply.text()));
        }
        ChatRequest body = ChatRequest.builder().model("m").build();
        client().chats().createStream(body).close();
        assertEquals(List.of("hi"), texts);
        assertEquals("{\"stream\":true}", bodies.get(0));
        assertEquals("{\"model\":\"m\",\"stream\":true}", bodies.get(1));
        assertTrue(body.stream().isEmpty());
    }

    @Test
    void aBodilessSuccessIsEmptyWhereTheSpecAllowsIt() {
        okStatus = 202;
        okBody = "";
        assertEquals(Optional.empty(), client().jobs().retrieve("j"));
        okStatus = 200;
        okBody = "{\"id\":\"j\"}";
        assertEquals("j", client().jobs().retrieve("j").orElseThrow().id());
    }

    @Test
    void anInjectedClientIsUsed() {
        List<String> seen = new ArrayList<>();
        OkHttpClient http = new OkHttpClient.Builder()
                .addInterceptor(chain -> {
                    seen.add(chain.request().url().encodedPath());
                    return chain.proceed(chain.request());
                })
                .build();
        try (Torture torture = new Torture("token", TortureOptions.builder().baseUrl(url()).httpClient(http).build())) {
            torture.things().retrieve("t");
        }
        assertEquals(List.of("/things/t"), seen);
        assertFalse(http.dispatcher().executorService().isShutdown(), "a given client stays open");
    }

    @Test
    void timeoutsApplyToEachAttempt() {
        slowServer();
        TortureOptions options = TortureOptions.builder()
                .baseUrl(url())
                .timeout(Duration.ofMillis(200))
                .maxRetries(0)
                .build();
        ApiTimeoutException error =
                assertThrows(ApiTimeoutException.class, () -> new Torture("token", options).things().retrieve("t"));
        assertInstanceOf(ApiConnectionException.class, error);
    }

    @Test
    void connectionErrorsAreNotApiErrors() throws Exception {
        int port;
        try (ServerSocket socket = new ServerSocket(0)) {
            port = socket.getLocalPort();
        }
        TortureOptions options = TortureOptions.builder().baseUrl("http://127.0.0.1:" + port).maxRetries(0).build();
        TortureException error =
                assertThrows(TortureException.class, () -> new Torture("token", options).things().retrieve("t"));
        assertInstanceOf(ApiConnectionException.class, error);
        assertFalse(error instanceof ApiException);
    }

    @Test
    void requestOptionsApplyToOneCall() {
        statuses.add(503);
        RequestOptions once =
                RequestOptions.builder().header("x-trace", "t1").maxRetries(0).idempotencyKey("k1").build();
        InternalServerException error = assertThrows(
                InternalServerException.class, () -> client().things().create(create(), once));
        assertEquals(503, error.statusCode());
        assertEquals(List.of("t1"), traces);
        assertEquals(List.of("k1"), idempotencyKeys);
        okBody = "{\"data\":[]}";
        client().things().list("r", RequestOptions.builder().header("x-trace", "t2").build());
        assertEquals(List.of("t1", "t2"), traces);
    }

    @Test
    void aPerCallTimeoutOverridesTheClients() {
        slowServer();
        RequestOptions fast = RequestOptions.builder().timeout(Duration.ofMillis(200)).maxRetries(0).build();
        assertThrows(ApiTimeoutException.class, () -> client().things().retrieve("t", fast));
    }

    @Test
    void errorsHaveAStatusClassAndATypedBody() {
        statuses.add(404);
        NotFoundException missing = assertThrows(NotFoundException.class, () -> client().things().retrieve("t"));
        assertEquals(404, missing.statusCode());
        assertTrue(missing.error().isPresent());

        statuses.add(422);
        errorBody = "{\"message\":\"bad name\",\"fields\":{\"name\":[\"too short\"]}}";
        UnprocessableEntityException invalid =
                assertThrows(UnprocessableEntityException.class, () -> client().things().create(create()));
        ValidationError body = invalid.error(ValidationError.class).orElseThrow();
        assertEquals("bad name", body.message());
        assertEquals(List.of("too short"), body.fields().orElseThrow().get("name"));
        assertTrue(invalid.getMessage().contains("422") && invalid.getMessage().contains("bad name"));

        statuses.add(500);
        errorBody = "{\"anything\":1}";
        ApiException other = assertThrows(ApiException.class, () -> client().things().retrieve("t", RequestOptions.builder().maxRetries(0).build()));
        assertEquals(1, assertInstanceOf(JsonNode.class, other.error().orElseThrow()).get("anything").asInt());
    }

    @Test
    void rawResponsesCarryTheStatusAndHeaders() {
        ApiResponse<Thing> response = client().withRawResponse().things().retrieve("t");
        assertEquals(200, response.statusCode());
        assertEquals("req_1", response.requestId().orElseThrow());
        assertEquals("n", response.body().name());
        ApiResponse<Void> deleted = client().withRawResponse().things().delete("t");
        assertEquals(200, deleted.statusCode());
    }

    @Test
    void asyncCallsRetryAndFailWithTheSdkExceptions() throws Exception {
        statuses.add(503);
        assertEquals("n", client().async().things().retrieve("t").get().name());
        assertEquals(2, requests.size());

        statuses.add(404);
        CompletionException error =
                assertThrows(CompletionException.class, () -> client().async().things().retrieve("t").join());
        assertInstanceOf(NotFoundException.class, error.getCause());

        ApiResponse<Thing> raw = client().async().withRawResponse().things().retrieve("t").get();
        assertEquals("req_" + requests.size(), raw.requestId().orElseThrow());
    }

    @Test
    void responsesMissingRequiredPropertiesFailOnAccess() {
        okBody = "{\"name\":\"n\"}";
        Thing thing = client().things().retrieve("t");
        assertEquals("n", thing.name());
        InvalidDataException error = assertThrows(InvalidDataException.class, thing::id);
        assertTrue(error.getMessage().contains("`id`"));
        okBody = "not json";
        assertThrows(InvalidDataException.class, () -> client().things().retrieve("t"));
    }

    private void slowServer() {
        server.removeContext("/");
        server.createContext("/", exchange -> {
            try {
                Thread.sleep(2000);
            } catch (InterruptedException ignored) {
            }
            exchange.close();
        });
    }
}
