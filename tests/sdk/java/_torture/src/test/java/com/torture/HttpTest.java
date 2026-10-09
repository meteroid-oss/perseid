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
import com.torture.internal.EnvProxy;
import com.torture.exceptions.ApiException;
import com.torture.exceptions.ApiTimeoutException;
import com.torture.exceptions.AuthenticationException;
import com.torture.exceptions.BadRequestException;
import com.torture.exceptions.ConflictException;
import com.torture.exceptions.InternalServerException;
import com.torture.exceptions.InvalidDataException;
import com.torture.exceptions.NotFoundException;
import com.torture.exceptions.PermissionDeniedException;
import com.torture.exceptions.RateLimitException;
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
import java.net.URLDecoder;
import java.nio.charset.StandardCharsets;
import java.time.Duration;
import java.time.OffsetDateTime;
import java.util.ArrayList;
import java.util.Collections;
import java.util.Deque;
import java.util.LinkedHashMap;
import java.util.List;
import java.util.Map;
import java.util.Optional;
import java.util.concurrent.CompletableFuture;
import java.util.concurrent.CompletionException;
import java.util.concurrent.ConcurrentLinkedDeque;
import java.util.concurrent.ExecutorService;
import java.util.concurrent.Executors;
import java.util.concurrent.atomic.AtomicInteger;
import okhttp3.OkHttpClient;
import org.junit.jupiter.api.AfterEach;
import org.junit.jupiter.api.BeforeEach;
import org.junit.jupiter.api.Test;

class HttpTest {
    private static final String THING =
            "{\"id\":\"i\",\"name\":\"n\",\"created_at\":\"2024-01-02T03:04:05Z\",\"kind\":\"alpha\",\"count\":1,"
                    + "\"nullable_required\":null,\"tags\":[],\"metadata\":{},\"attrs\":{}}";

    private HttpServer server;
    private ExecutorService executor;
    private final List<String> requests = Collections.synchronizedList(new ArrayList<>());
    private final List<String> idempotencyKeys = Collections.synchronizedList(new ArrayList<>());
    private final List<String> traces = Collections.synchronizedList(new ArrayList<>());
    private final List<String> required = Collections.synchronizedList(new ArrayList<>());
    private final List<String> bodies = Collections.synchronizedList(new ArrayList<>());
    private final List<String> retryCounts = Collections.synchronizedList(new ArrayList<>());
    private final List<String> rawQueries = Collections.synchronizedList(new ArrayList<>());
    private volatile boolean stallFirstRequest;
    private int okStatus = 200;
    private String okType = "application/json";
    private String errorBody = "{\"title\":\"no\"}";
    private String okBody = THING;
    private final Deque<Integer> statuses = new ConcurrentLinkedDeque<>();
    private String retryAfter;
    private String retryAfterMs;

    @BeforeEach
    void start() throws Exception {
        server = HttpServer.create(new InetSocketAddress("127.0.0.1", 0), 0);
        executor = Executors.newCachedThreadPool();
        server.setExecutor(executor);
        server.createContext("/", exchange -> {
            requests.add(exchange.getRequestMethod() + " " + exchange.getRequestURI().getRawPath()
                    + (exchange.getRequestURI().getRawQuery() == null ? "" : "?" + exchange.getRequestURI().getRawQuery()));
            idempotencyKeys.add(exchange.getRequestHeaders().getFirst("idempotency-key"));
            traces.add(exchange.getRequestHeaders().getFirst("x-trace"));
            required.add(exchange.getRequestHeaders().getFirst("x-required"));
            retryCounts.add(exchange.getRequestHeaders().getFirst("x-torture-retry-count"));
            rawQueries.add(exchange.getRequestURI().getRawQuery());
            bodies.add(new String(exchange.getRequestBody().readAllBytes(), StandardCharsets.UTF_8));
            if (stallFirstRequest && requests.size() == 1) {
                try {
                    Thread.sleep(1500);
                } catch (InterruptedException ignored) {
                    Thread.currentThread().interrupt();
                }
                exchange.close();
                return;
            }
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
        executor.shutdownNow();
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

        statuses.add(503);
        assertThrows(
                CompletionException.class,
                () -> client().async().things().update("t", ThingPatch.builder().build()).join());
        assertEquals(2, requests.size());
    }

    @Test
    void patchesWithoutAnIdempotencyKeyAreRetriedOn429() throws Exception {
        statuses.add(429);
        retryAfter = "0";
        client().things().update("t", ThingPatch.builder().build());
        assertEquals(2, requests.size());

        statuses.add(429);
        client().async().things().update("t", ThingPatch.builder().build()).get();
        assertEquals(4, requests.size());
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
    void streamedErrorsAreApiErrorsAndKeepalivesAreSkipped() {
        okType = "text/event-stream";
        okBody = "event: ping\ndata: {}\n\ndata: {\"text\":\"hi\"}\n\n"
                + "data: {\"error\":{\"message\":\"overloaded\",\"type\":\"server_error\"}}\n\ndata: {\"text\":\"late\"}\n\n";
        List<String> texts = new ArrayList<>();
        ApiException error;
        try (EventStream<ChatReply> stream = client().chats().createStream(null)) {
            error = assertThrows(ApiException.class, () -> stream.forEach(reply -> texts.add(reply.text())));
            assertFalse(stream.iterator().hasNext());
        }
        assertEquals(List.of("hi"), texts);
        assertEquals("POST /v1/chats streamed an error: overloaded", error.getMessage());
        assertEquals(200, error.statusCode());
        assertEquals("req_1", error.requestId().orElseThrow());
        assertEquals("server_error", ((JsonNode) error.error().orElseThrow()).path("error").path("type").asText());

        okBody = "event: error\ndata: upstream gone\n\n";
        try (EventStream<ChatReply> stream = client().chats().createStream(null)) {
            ApiException named = assertThrows(ApiException.class, () -> stream.iterator().hasNext());
            assertEquals("upstream gone", named.body());
        }
    }

    @Test
    void theProxyOptionSendsRequestsThroughIt() {
        java.net.Proxy proxy = new java.net.Proxy(java.net.Proxy.Type.HTTP, server.getAddress());
        TortureOptions options = TortureOptions.builder().baseUrl("http://api.torture.invalid/v1").proxy(proxy).build();
        try (Torture client = new Torture("token", options)) {
            assertEquals("i", client.things().retrieve("t").id());
        }
        assertEquals(List.of("GET /v1/things/t"), requests);
    }

    @Test
    void environmentProxiesFollowTheSchemeAndNoProxy() {
        Map<String, String> env = Map.of(
                "HTTPS_PROXY", "user%40corp:p%3Ass@proxy.corp:3128",
                "http_proxy", "socks5://[::1]",
                "NO_PROXY", " Example.com, .internal ,*.svc, 10.0.0.0/8, ::1, 192.168.1.7");
        EnvProxy proxies = EnvProxy.fromEnv(env).orElseThrow();
        java.net.Proxy https = proxies.select(java.net.URI.create("https://api.torture.dev/v1")).get(0);
        assertEquals(java.net.Proxy.Type.HTTP, https.type());
        assertEquals("proxy.corp:3128", https.address().toString().replace("/<unresolved>", ""));
        java.net.Proxy http = proxies.select(java.net.URI.create("http://api.torture.dev/v1")).get(0);
        assertEquals(java.net.Proxy.Type.SOCKS, http.type());
        assertEquals(1080, ((InetSocketAddress) http.address()).getPort());
        for (String host : List.of("example.com", "api.example.com", "EXAMPLE.COM.", "a.b.internal", "x.svc", "10.1.2.3", "[::1]", "192.168.1.7")) {
            assertTrue(proxies.bypasses(host), host);
            assertEquals(java.net.Proxy.NO_PROXY, proxies.select(java.net.URI.create("https://" + host + "/")).get(0));
        }
        for (String host : List.of("notexample.com", "example.org", "internal.org", "11.0.0.1", "192.168.1.8", "[::2]")) {
            assertFalse(proxies.bypasses(host), host);
        }
        assertTrue(EnvProxy.fromEnv(Map.of("NO_PROXY", "*")).isEmpty());
        TortureException tls = assertThrows(TortureException.class, () -> EnvProxy.fromEnv(Map.of("HTTPS_PROXY", "HTTPS://p:443")));
        assertTrue(tls.getMessage().contains("https:// proxy"), tls.getMessage());
        assertTrue(EnvProxy.fromEnv(Map.of("HTTP_PROXY", "p:1", "HTTPS_PROXY", "p:1", "ALL_PROXY", "https://q")).isPresent());
        assertTrue(EnvProxy.fromEnv(Map.of("ALL_PROXY", "http://p:8080", "NO_PROXY", "*")).orElseThrow().bypasses("a.b"));
        okhttp3.Response challenge = new okhttp3.Response.Builder()
                .request(new okhttp3.Request.Builder().url("https://api.torture.dev").build())
                .protocol(okhttp3.Protocol.HTTP_1_1).code(407).message("Proxy Authentication Required").build();
        okhttp3.Request authorized = proxies.authenticate(null, challenge);
        assertEquals("Basic dXNlckBjb3JwOnA6c3M=", authorized.header("Proxy-Authorization"));
        assertEquals(null, proxies.authenticate(null, challenge.newBuilder().request(authorized).build()));
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
        assertEquals("POST /v1/things failed with status 422: bad name", invalid.getMessage());

        statuses.add(429);
        errorBody = "{\"error\":{\"message\":\"Slow down\",\"type\":\"rate_limit\"}}";
        RateLimitException limited = assertThrows(
                RateLimitException.class, () -> client().things().retrieve("t", RequestOptions.builder().maxRetries(0).build()));
        assertTrue(limited.getMessage().endsWith("failed with status 429: Slow down"), limited.getMessage());
        assertEquals(errorBody, limited.body());

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

    @Test
    void pathParametersAreEscapedByByte() {
        Map<String, String> escaped = new LinkedHashMap<>();
        escaped.put("plain", "plain");
        escaped.put("sp ace", "sp%20ace");
        escaped.put("sl/ash", "sl%2Fash");
        escaped.put("q?mark", "q%3Fmark");
        escaped.put("per%cent", "per%25cent");
        escaped.put("ha#sh", "ha%23sh");
        escaped.put("lit%25eral", "lit%2525eral");
        escaped.put("héllo wörld ✓", "h%C3%A9llo%20w%C3%B6rld%20%E2%9C%93");
        escaped.put("a/../b", "a%2F..%2Fb");
        List<String> want = new ArrayList<>();
        for (Map.Entry<String, String> entry : escaped.entrySet()) {
            client().things().retrieve(entry.getKey());
            want.add("GET /v1/things/" + entry.getValue());
        }
        assertEquals(want, requests);
        // Dot segments and empty values would change the path: nothing is sent for them.
        for (String invalid : List.of("..", ".", "")) {
            assertThrows(IllegalArgumentException.class, () -> client().things().retrieve(invalid), invalid);
        }
        assertEquals(want.size(), requests.size());
    }

    @Test
    void queryValuesAreEncodedSoThatTheyDecodeToThemselves() {
        okBody = "{\"data\":[]}";
        List<String> values = List.of("plain", "sp ace", "a&b=c+d", "100%", "slash/qm?", "héllo wörld ✓", "x#y");
        client().things().list("needed", ThingsListOptions.builder().ids(values).build());
        List<String> decoded = new ArrayList<>();
        for (String pair : rawQueries.get(0).split("&", -1)) {
            int eq = pair.indexOf('=');
            assertEquals("ids", URLDecoder.decode(pair.substring(0, eq), StandardCharsets.UTF_8), pair);
            decoded.add(URLDecoder.decode(pair.substring(eq + 1), StandardCharsets.UTF_8));
        }
        assertEquals(values, decoded);
    }

    @Test
    void everyErrorStatusHasItsOwnExceptionClass() {
        Map<Integer, Class<? extends ApiException>> classes = new LinkedHashMap<>();
        classes.put(400, BadRequestException.class);
        classes.put(401, AuthenticationException.class);
        classes.put(403, PermissionDeniedException.class);
        classes.put(404, NotFoundException.class);
        classes.put(409, ConflictException.class);
        classes.put(418, ApiException.class);
        classes.put(422, UnprocessableEntityException.class);
        classes.put(429, RateLimitException.class);
        classes.put(500, InternalServerException.class);
        classes.put(503, InternalServerException.class);
        RequestOptions once = RequestOptions.builder().maxRetries(0).build();
        for (Map.Entry<Integer, Class<? extends ApiException>> entry : classes.entrySet()) {
            statuses.add(entry.getKey());
            ApiException error = assertThrows(ApiException.class, () -> client().things().retrieve("t", once));
            assertEquals(entry.getValue(), error.getClass(), "status " + entry.getKey());
            assertEquals(entry.getKey(), error.statusCode());
            assertTrue(error.requestId().isPresent(), "the request id of status " + entry.getKey());
            assertTrue(error instanceof TortureException);
        }
        assertEquals(classes.size(), requests.size());
    }

    @Test
    void errorBodiesThatAreNotJsonOrAreEmptyAreKeptRaw() {
        RequestOptions once = RequestOptions.builder().maxRetries(0).build();
        statuses.add(400);
        errorBody = "<html>nope</html>";
        BadRequestException html = assertThrows(BadRequestException.class, () -> client().things().retrieve("t", once));
        assertEquals("<html>nope</html>", html.body());
        assertTrue(html.error().isEmpty());
        assertTrue(html.getMessage().contains("400") && html.getMessage().contains("nope"), html.getMessage());
        assertEquals("req_1", html.requestId().orElseThrow());

        statuses.add(404);
        errorBody = "";
        NotFoundException empty = assertThrows(NotFoundException.class, () -> client().things().retrieve("t", once));
        assertEquals("", empty.body());
        assertTrue(empty.error().isEmpty());
        assertEquals(404, empty.statusCode());
        assertEquals("req_2", empty.requestId().orElseThrow());

        statuses.add(500);
        errorBody = "{\"truncated\": ";
        InternalServerException truncated =
                assertThrows(InternalServerException.class, () -> client().things().retrieve("t", once));
        assertEquals("{\"truncated\": ", truncated.body());
        assertTrue(truncated.error().isEmpty());

        // A JSON body that is not an object is kept as JSON.
        statuses.add(409);
        errorBody = "[1,2]";
        ConflictException list = assertThrows(ConflictException.class, () -> client().things().retrieve("t", once));
        assertEquals(2, assertInstanceOf(JsonNode.class, list.error().orElseThrow()).size());
    }

    @Test
    void anEmptyBodyWhereAnObjectIsRequiredIsADecodeError() {
        okBody = "";
        assertThrows(InvalidDataException.class, () -> client().things().retrieve("t"));
        okBody = "{\"id\":";
        InvalidDataException truncated = assertThrows(InvalidDataException.class, () -> client().things().retrieve("t"));
        assertTrue(truncated.getMessage().length() > 0);
        assertEquals(2, requests.size(), "decode errors are not retried");
    }

    @Test
    void timedOutAttemptsAreRetried() {
        stallFirstRequest = true;
        TortureOptions options = TortureOptions.builder()
                .baseUrl(url())
                .timeout(Duration.ofMillis(300))
                .retrySchedule(List.of(1L))
                .build();
        try (Torture torture = new Torture("token", options)) {
            assertEquals("n", torture.things().retrieve("t").name());
        }
        assertEquals(2, requests.size());
        assertEquals(java.util.Arrays.asList(null, "1"), retryCounts);
    }

    @Test
    void timedOutAttemptsFailOnceTheRetriesAreSpent() {
        slowServer();
        TortureOptions options = TortureOptions.builder()
                .baseUrl(url())
                .timeout(Duration.ofMillis(200))
                .retrySchedule(List.of(1L))
                .build();
        try (Torture torture = new Torture("token", options)) {
            assertThrows(ApiTimeoutException.class, () -> torture.things().retrieve("t"));
        }
    }

    @Test
    void middlewareRunsInsideTheRetryLoop() {
        AtomicInteger calls = new AtomicInteger();
        List<Integer> statusesSeen = Collections.synchronizedList(new ArrayList<>());
        TortureOptions options = TortureOptions.builder()
                .baseUrl(url())
                .retrySchedule(List.of(1L, 1L))
                .addInterceptor(chain -> {
                    calls.incrementAndGet();
                    okhttp3.Response response = chain.proceed(chain.request());
                    statusesSeen.add(response.code());
                    return response;
                })
                .build();
        statuses.add(503);
        try (Torture torture = new Torture("token", options)) {
            assertEquals("n", torture.things().retrieve("t").name());
        }
        assertEquals(2, calls.get(), "the middleware sees every attempt");
        assertEquals(List.of(503, 200), statusesSeen);
        assertEquals(2, requests.size());
    }

    @Test
    void cancellingAnAsyncCallStopsItsRetries() throws Exception {
        statuses.add(503);
        retryAfterMs = "500";
        CompletableFuture<Thing> call = client().async().things().retrieve("t");
        for (int i = 0; i < 100 && requests.isEmpty(); i++) {
            Thread.sleep(20);
        }
        assertEquals(1, requests.size());
        assertTrue(call.cancel(true));
        Thread.sleep(900);
        assertEquals(1, requests.size(), "no attempt after the cancellation");
        assertTrue(call.isCancelled());
    }

    @Test
    void anAsyncCallCanBeCancelledWhileItWaitsForTheServer() throws Exception {
        slowServer();
        CompletableFuture<Thing> call = client().async().things().retrieve("t");
        Thread.sleep(100);
        assertTrue(call.cancel(true));
        assertThrows(java.util.concurrent.CancellationException.class, call::get);
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
