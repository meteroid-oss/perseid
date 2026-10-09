package com.torture;

import static org.junit.jupiter.api.Assertions.assertArrayEquals;
import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertThrows;
import static org.junit.jupiter.api.Assertions.assertTrue;

import com.sun.net.httpserver.HttpExchange;
import com.sun.net.httpserver.HttpServer;
import com.torture.exceptions.ApiConnectionException;
import com.torture.exceptions.ApiTimeoutException;
import com.torture.exceptions.NotFoundException;
import com.torture.streaming.BinaryResponse;
import java.io.IOException;
import java.io.InputStream;
import java.io.OutputStream;
import java.net.InetSocketAddress;
import java.nio.charset.StandardCharsets;
import java.nio.file.Files;
import java.nio.file.Path;
import java.time.Duration;
import java.util.List;
import java.util.concurrent.CountDownLatch;
import java.util.concurrent.ExecutorService;
import java.util.concurrent.Executors;
import java.util.concurrent.TimeUnit;
import java.util.concurrent.atomic.AtomicInteger;
import org.junit.jupiter.api.AfterEach;
import org.junit.jupiter.api.Test;
import org.junit.jupiter.api.io.TempDir;

class BinaryTest {
    /** Answers one exchange; the number of the attempt is given. */
    private interface Handler {
        void handle(int attempt, HttpExchange exchange) throws Exception;
    }

    private HttpServer server;
    private ExecutorService executor;
    private final AtomicInteger attempts = new AtomicInteger();

    @AfterEach
    void stop() {
        server.stop(0);
        executor.shutdownNow();
    }

    private Torture serve(Handler handler) throws IOException {
        server = HttpServer.create(new InetSocketAddress("127.0.0.1", 0), 0);
        executor = Executors.newCachedThreadPool();
        server.setExecutor(executor);
        server.createContext("/", exchange -> {
            try {
                handler.handle(attempts.incrementAndGet(), exchange);
            } catch (Exception ignored) {
                // The client went away.
            } finally {
                exchange.close();
            }
        });
        server.start();
        String url = "http://127.0.0.1:" + server.getAddress().getPort() + "/v1";
        return new Torture("token", TortureOptions.builder().baseUrl(url).retrySchedule(List.of(1L, 1L)).build());
    }

    /** Sends the parts chunked, flushing each, after {@code pause} returns for all but the first. */
    private static void chunks(HttpExchange exchange, Runnable pause, String... parts) throws IOException {
        exchange.getResponseHeaders().add("content-type", "application/pdf");
        exchange.getResponseHeaders().add("content-disposition", "attachment; filename=\"t1.pdf\"");
        exchange.sendResponseHeaders(200, 0);
        OutputStream out = exchange.getResponseBody();
        for (int i = 0; i < parts.length; i++) {
            if (i > 0) {
                pause.run();
            }
            out.write(parts[i].getBytes(StandardCharsets.UTF_8));
            out.flush();
        }
    }

    private static Runnable sleep(long millis) {
        return () -> {
            try {
                Thread.sleep(millis);
            } catch (InterruptedException e) {
                Thread.currentThread().interrupt();
            }
        };
    }

    @Test
    void theBodyStreamsBeforeItEnds() throws Exception {
        CountDownLatch gate = new CountDownLatch(1);
        Torture client = serve((n, exchange) -> chunks(exchange, () -> {
            try {
                gate.await(5, TimeUnit.SECONDS);
            } catch (InterruptedException e) {
                Thread.currentThread().interrupt();
            }
        }, "%PDF-", "rest"));
        try (BinaryResponse body = client.things().download("t1")) {
            assertEquals("application/pdf", body.contentType().orElseThrow());
            assertTrue(body.headers().get("content-disposition").contains("t1.pdf"));
            InputStream in = body.inputStream();
            assertEquals("%PDF-", new String(in.readNBytes(5), StandardCharsets.UTF_8));
            gate.countDown();
            assertEquals("rest", new String(body.bytes(), StandardCharsets.UTF_8));
        }
    }

    @Test
    void theBodyIsReadWholeOrWrittenToAFile(@TempDir Path dir) throws Exception {
        Torture client = serve((n, exchange) -> chunks(exchange, () -> { }, "%PDF-", "1.7"));
        assertArrayEquals("%PDF-1.7".getBytes(StandardCharsets.UTF_8), client.things().download("t1").bytes());
        Path file = dir.resolve("t1.pdf");
        client.things().download("t1").writeTo(file);
        assertEquals("%PDF-1.7", Files.readString(file));
        assertEquals("%PDF-1.7", new String(client.async().things().download("t1").get().bytes(), StandardCharsets.UTF_8));
        try (BinaryResponse raw = client.withRawResponse().things().download("t1").body()) {
            assertEquals(200, raw.statusCode());
        }
    }

    @Test
    void aFailedDownloadLeavesTheFileAsItWas(@TempDir Path dir) throws Exception {
        Torture client = serve((n, exchange) -> {
            exchange.getResponseHeaders().add("content-type", "application/pdf");
            exchange.sendResponseHeaders(200, n == 1 ? 100 : 8);
            exchange.getResponseBody().write("%PDF-1.7".getBytes(StandardCharsets.UTF_8));
        });
        Path file = dir.resolve("t1.pdf");
        Files.writeString(file, "old");
        assertThrows(ApiConnectionException.class, () -> client.things().download("t1").writeTo(file));
        assertEquals("old", Files.readString(file));
        client.things().download("t1").writeTo(file);
        assertEquals("%PDF-1.7", Files.readString(file));
        try (var files = Files.list(dir)) {
            assertEquals(List.of(file), files.collect(java.util.stream.Collectors.toList()));
        }
    }

    @Test
    void anErrorStatusIsThrownBeforeTheBody() throws Exception {
        Torture client = serve((n, exchange) -> {
            byte[] body = "{\"message\":\"no such thing\"}".getBytes(StandardCharsets.UTF_8);
            exchange.getResponseHeaders().add("content-type", "application/json");
            exchange.sendResponseHeaders(404, body.length);
            exchange.getResponseBody().write(body);
        });
        NotFoundException error = assertThrows(NotFoundException.class, () -> client.things().download("t1"));
        assertTrue(error.getMessage().contains("no such thing"), error.getMessage());
        assertEquals(1, attempts.get());
    }

    @Test
    void retriesStopOnceTheHeadersArrive() throws Exception {
        Torture client = serve((n, exchange) -> {
            if (n == 1) {
                exchange.sendResponseHeaders(503, -1);
                return;
            }
            // The body is cut short of its declared length.
            exchange.sendResponseHeaders(200, 10);
            exchange.getResponseBody().write("%PDF-".getBytes(StandardCharsets.UTF_8));
            exchange.getResponseBody().flush();
        });
        BinaryResponse body = client.things().download("t1");
        assertEquals(2, attempts.get());
        assertThrows(ApiConnectionException.class, body::bytes);
        assertEquals(2, attempts.get());
    }

    @Test
    void theTimeoutBoundsEachReadNotTheDownload() throws Exception {
        Torture client = serve((n, exchange) -> chunks(exchange, sleep(n == 1 ? 50 : 1000), "x", "x", "x", "x", "x", "x"));
        RequestOptions timeout = RequestOptions.builder().timeout(Duration.ofMillis(200)).maxRetries(0).build();
        assertEquals(6, client.things().download("t1", timeout).bytes().length);
        BinaryResponse stalled = client.things().download("t1", timeout);
        assertThrows(ApiTimeoutException.class, stalled::bytes);
    }
}
