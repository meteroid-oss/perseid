package com.vault;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertFalse;
import static org.junit.jupiter.api.Assertions.assertNull;
import static org.junit.jupiter.api.Assertions.assertThrows;
import static org.junit.jupiter.api.Assertions.assertTrue;

import com.sun.net.httpserver.HttpExchange;
import com.sun.net.httpserver.HttpServer;
import com.vault.exceptions.AuthenticationException;
import com.vault.exceptions.InvalidDataException;
import java.io.IOException;
import java.net.InetSocketAddress;
import java.net.URLDecoder;
import java.nio.charset.StandardCharsets;
import java.util.ArrayList;
import java.util.Base64;
import java.util.Collections;
import java.util.HashSet;
import java.util.List;
import java.util.Set;
import java.util.concurrent.CompletableFuture;
import java.util.concurrent.ExecutorService;
import java.util.concurrent.Executors;
import java.util.concurrent.atomic.AtomicInteger;
import org.junit.jupiter.api.AfterEach;
import org.junit.jupiter.api.BeforeEach;
import org.junit.jupiter.api.Test;

/**
 * The OAuth2 client credentials flow of the SDK generated from tests/fixtures/oauth.yaml, against a
 * local server that plays the token endpoint and the API.
 */
class OAuthTest {
    /** One request the server got. */
    private static final class Seen {
        final String method;
        final String path;
        final String authorization;
        final String contentType;
        final String body;

        Seen(String method, String path, String authorization, String contentType, String body) {
            this.method = method;
            this.path = path;
            this.authorization = authorization;
            this.contentType = contentType;
            this.body = body;
        }
    }

    private HttpServer server;
    private ExecutorService executor;
    private String base;
    private final List<Seen> seen = Collections.synchronizedList(new ArrayList<>());
    private final Set<String> revoked = Collections.synchronizedSet(new HashSet<>());
    private final List<Integer> scripted = Collections.synchronizedList(new ArrayList<>());
    private final AtomicInteger issued = new AtomicInteger();
    private volatile String expiresIn = "3600";
    private volatile String tokenBody;
    private volatile long tokenDelayMillis;

    @BeforeEach
    void start() throws IOException {
        executor = Executors.newCachedThreadPool();
        server = HttpServer.create(new InetSocketAddress("127.0.0.1", 0), 0);
        server.setExecutor(executor);
        server.createContext("/", this::handle);
        server.start();
        base = "http://127.0.0.1:" + server.getAddress().getPort() + "/v1";
    }

    @AfterEach
    void stop() {
        server.stop(0);
        executor.shutdownNow();
    }

    private void handle(HttpExchange exchange) throws IOException {
        String body = new String(exchange.getRequestBody().readAllBytes(), StandardCharsets.UTF_8);
        String path = exchange.getRequestURI().getPath();
        seen.add(
                new Seen(
                        exchange.getRequestMethod(),
                        path,
                        exchange.getRequestHeaders().getFirst("Authorization"),
                        exchange.getRequestHeaders().getFirst("Content-Type"),
                        body));
        int status = 200;
        String reply;
        if (path.equals("/v1/oauth/token")) {
            sleep(tokenDelayMillis);
            if (!scripted.isEmpty()) {
                status = scripted.remove(0);
                reply = "{\"error\":\"scripted\"}";
            } else if (tokenBody != null) {
                reply = tokenBody;
            } else {
                reply =
                        "{\"access_token\":\"at-"
                                + issued.incrementAndGet()
                                + "\",\"token_type\":\"Bearer\",\"expires_in\":"
                                + expiresIn
                                + "}";
            }
        } else {
            String authorization = exchange.getRequestHeaders().getFirst("Authorization");
            String token = authorization == null ? "anonymous" : authorization.replaceFirst("^Bearer ", "");
            if (revoked.contains(token)) {
                status = 401;
                reply = "{\"message\":\"revoked\"}";
            } else {
                reply = "{\"status\":\"" + token + "\"}";
            }
        }
        byte[] bytes = reply.getBytes(StandardCharsets.UTF_8);
        exchange.getResponseHeaders().add("Content-Type", "application/json");
        exchange.sendResponseHeaders(status, bytes.length);
        exchange.getResponseBody().write(bytes);
        exchange.close();
    }

    private static void sleep(long millis) {
        try {
            Thread.sleep(millis);
        } catch (InterruptedException e) {
            Thread.currentThread().interrupt();
        }
    }

    private VaultOptions.Builder options() {
        return VaultOptions.builder().baseUrl(base).clientCredentials("id", "secret").maxRetries(0);
    }

    private List<Seen> tokenRequests() {
        List<Seen> tokens = new ArrayList<>();
        synchronized (seen) {
            for (Seen request : seen) {
                if (request.path.equals("/v1/oauth/token")) {
                    tokens.add(request);
                }
            }
        }
        return tokens;
    }

    private static String basic(String id, String secret) {
        String credentials = id + ":" + secret;
        return "Basic " + Base64.getEncoder().encodeToString(credentials.getBytes(StandardCharsets.UTF_8));
    }

    private static String decoded(String form) {
        return URLDecoder.decode(form, StandardCharsets.UTF_8);
    }

    @Test
    void aTokenIsFetchedOnFirstUseAndKept() {
        try (Vault client = new Vault(null, options().build())) {
            assertEquals("at-1", client.account().retrieveMachine().status());
            assertEquals("at-1", client.account().retrieveMachine().status());
        }
        List<Seen> tokens = tokenRequests();
        assertEquals(1, tokens.size());
        assertEquals("POST", tokens.get(0).method);
        assertTrue(tokens.get(0).contentType.startsWith("application/x-www-form-urlencoded"));
        assertEquals(basic("id", "secret"), tokens.get(0).authorization);
        assertEquals(
                "grant_type=client_credentials&scope=secrets.read secrets.write",
                decoded(tokens.get(0).body));
        assertEquals("Bearer at-1", seen.get(1).authorization);
    }

    @Test
    void publicOperationsSendNoCredentialsAndFetchNoToken() {
        try (Vault client = new Vault(null, options().build())) {
            assertEquals("anonymous", client.account().checkHealth().status());
            assertEquals(0, tokenRequests().size());
            assertNull(seen.get(0).authorization);
            assertEquals("at-1", client.account().createSession().status());
        }
    }

    @Test
    void credentialsAreFormEncodedBeforeTheBasicHeaderOrSentInTheBody() {
        VaultOptions odd = options().clientCredentials("a b", "p@ss:word").build();
        try (Vault client = new Vault(null, odd)) {
            client.account().retrieveMachine();
        }
        assertEquals(basic("a+b", "p%40ss%3Aword"), tokenRequests().get(0).authorization);

        seen.clear();
        try (Vault client = new Vault(null, options().clientAuthInBody(true).build())) {
            client.account().retrieveMachine();
        }
        Seen token = tokenRequests().get(0);
        assertNull(token.authorization);
        assertTrue(decoded(token.body).contains("client_id=id&client_secret=secret"), token.body);
    }

    @Test
    void anExpiredTokenIsRenewed() {
        expiresIn = "0";
        try (Vault client = new Vault(null, options().build())) {
            assertEquals("at-1", client.account().retrieveMachine().status());
            assertEquals("at-2", client.account().retrieveMachine().status());
        }
        assertEquals(2, tokenRequests().size());
    }

    @Test
    void aTokenTheApiRejectsIsReplacedOnce() {
        revoked.add("at-1");
        try (Vault client = new Vault(null, options().build())) {
            assertEquals("at-2", client.account().retrieveMachine().status());
            assertEquals(2, tokenRequests().size());
            assertEquals(2, seen.size() - tokenRequests().size());
            assertEquals("at-2", client.account().retrieveMachine().status());
            assertEquals(2, tokenRequests().size());
        }

        seen.clear();
        issued.set(0);
        revoked.addAll(List.of("at-1", "at-2", "at-3"));
        try (Vault client = new Vault(null, options().build())) {
            assertThrows(AuthenticationException.class, () -> client.account().retrieveMachine());
        }
        assertEquals(2, tokenRequests().size(), "a second 401 is the caller's");
    }

    @Test
    void aTokenTheApiRejectsIsReplacedInAsyncCallsToo() throws Exception {
        revoked.add("at-1");
        try (Vault client = new Vault(null, options().build())) {
            assertEquals("at-2", client.async().account().retrieveMachine().get().status());
        }
        assertEquals(2, tokenRequests().size());
    }

    @Test
    void theTokenRequestIsRetriedLikeAnyRequest() {
        scripted.add(503);
        try (Vault client = new Vault(null, options().retrySchedule(List.of(1L)).build())) {
            assertEquals("at-1", client.account().retrieveMachine().status());
        }
        assertEquals(2, tokenRequests().size());

        seen.clear();
        scripted.add(401);
        try (Vault client = new Vault(null, options().build())) {
            AuthenticationException error =
                    assertThrows(AuthenticationException.class, () -> client.account().retrieveMachine());
            assertTrue(error.body().contains("scripted"));
        }
        assertEquals(1, seen.size(), "the API is not called without a token");

        tokenBody = "{\"token_type\":\"Bearer\"}";
        try (Vault client = new Vault(null, options().build())) {
            assertThrows(InvalidDataException.class, () -> client.account().retrieveMachine());
        }
    }

    @Test
    void concurrentCallsShareOneTokenRequest() throws Exception {
        tokenDelayMillis = 200;
        try (Vault client = new Vault(null, options().build())) {
            List<CompletableFuture<String>> calls = new ArrayList<>();
            for (int i = 0; i < 4; i++) {
                calls.add(CompletableFuture.supplyAsync(() -> client.account().retrieveMachine().status()));
            }
            for (CompletableFuture<String> call : calls) {
                assertEquals("at-1", call.get());
            }
        }
        assertEquals(1, tokenRequests().size());
    }

    @Test
    void aTokenOrAProviderWinsOverTheClientCredentials() {
        try (Vault client = new Vault(null, options().tokenProvider(() -> "mine").build())) {
            assertEquals("mine", client.account().retrieveMachine().status());
        }
        try (Vault client = new Vault("static", options().build())) {
            assertEquals("static", client.account().retrieveMachine().status());
        }
        assertFalse(seen.stream().anyMatch(request -> request.path.endsWith("/oauth/token")));
    }
}
