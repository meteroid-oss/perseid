package com.forms;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertInstanceOf;
import static org.junit.jupiter.api.Assertions.assertThrows;
import static org.junit.jupiter.api.Assertions.assertTrue;

import com.forms.api.Transcriptions;
import com.forms.api.Uploads;
import com.forms.exceptions.ApiException;
import com.forms.exceptions.RateLimitException;
import com.forms.models.Delta;
import com.forms.models.TranscriptionDelta;
import com.forms.streaming.EventStream;
import com.forms.streaming.Upload;
import com.sun.net.httpserver.HttpServer;
import java.io.ByteArrayInputStream;
import java.io.UncheckedIOException;
import java.net.InetSocketAddress;
import java.nio.charset.StandardCharsets;
import java.nio.file.Files;
import java.nio.file.NoSuchFileException;
import java.nio.file.Path;
import java.util.ArrayList;
import java.util.Collections;
import java.util.Deque;
import java.util.List;
import java.util.concurrent.ConcurrentLinkedDeque;
import org.junit.jupiter.api.AfterEach;
import org.junit.jupiter.api.BeforeEach;
import org.junit.jupiter.api.Test;

/** A form operation that also answers an event stream, on the SDK of tests/sdk/java/_forms. */
class FormsTest {
    private HttpServer server;
    private final List<String> bodies = Collections.synchronizedList(new ArrayList<>());
    private final Deque<Integer> statuses = new ConcurrentLinkedDeque<>();
    private volatile String events = "";

    @BeforeEach
    void start() throws Exception {
        server = HttpServer.create(new InetSocketAddress("127.0.0.1", 0), 0);
        server.createContext("/deltas", exchange -> {
            byte[] reply = events.getBytes(StandardCharsets.UTF_8);
            exchange.getResponseHeaders().add("content-type", "text/event-stream");
            exchange.sendResponseHeaders(200, reply.length);
            exchange.getResponseBody().write(reply);
            exchange.close();
        });
        server.createContext("/uploads", exchange -> {
            bodies.add(new String(exchange.getRequestBody().readAllBytes(), StandardCharsets.UTF_8));
            Integer status = statuses.poll();
            byte[] reply = "{\"text\":\"ok\"}".getBytes(StandardCharsets.UTF_8);
            exchange.getResponseHeaders().add("content-type", "application/json");
            exchange.getResponseHeaders().add("retry-after", "0");
            exchange.sendResponseHeaders(status == null ? 200 : status, reply.length);
            exchange.getResponseBody().write(reply);
            exchange.close();
        });
        server.createContext("/", exchange -> {
            String body = new String(exchange.getRequestBody().readAllBytes(), StandardCharsets.UTF_8);
            bodies.add(body);
            boolean stream = body.contains("name=\"stream\"\r\nContent-Length: 4\r\n\r\ntrue");
            String deltas = events.isEmpty() ? "data: {\"delta\":\"hel\"}\n\ndata: {\"delta\":\"lo\"}\n\n" : events;
            byte[] reply = (stream ? deltas : "{\"text\":\"hello\"}")
                    .getBytes(StandardCharsets.UTF_8);
            exchange.getResponseHeaders().add("content-type", stream ? "text/event-stream" : "application/json");
            exchange.sendResponseHeaders(200, reply.length);
            exchange.getResponseBody().write(reply);
            exchange.close();
        });
        server.start();
    }

    @AfterEach
    void stop() {
        server.stop(0);
    }

    private Forms client() {
        return new Forms("key", FormsOptions.builder().baseUrl("http://127.0.0.1:" + server.getAddress().getPort()).build());
    }

    @Test
    void theStreamTwinOfAFormAsksForTheStream() throws Exception {
        Path audio = Files.createTempFile("speech", ".MP3");
        Files.write(audio, new byte[] {1, 2, 3});
        try (Forms client = client()) {
            Transcriptions.CreateStreamBody body =
                    Transcriptions.CreateStreamBody.builder().file(Upload.of(audio)).model("m").build();
            List<String> deltas = new ArrayList<>();
            try (EventStream<TranscriptionDelta> events = client.transcriptions().createStream(body)) {
                events.forEach(event -> deltas.add(event.delta()));
            }
            assertEquals(List.of("hel", "lo"), deltas);
            Upload plain = Upload.of(audio.toFile());
            assertEquals("hello", client.transcriptions()
                    .create(Transcriptions.CreateBody.builder().file(plain).model("m").build()).text());
        } finally {
            Files.delete(audio);
        }
        String sent = bodies.get(0);
        assertTrue(sent.contains("filename=\"" + audio.getFileName() + "\"\r\nContent-Type: audio/mpeg"), sent);
        assertTrue(!bodies.get(1).contains("name=\"stream\""), bodies.get(1));
    }

    private static Path tempFile(String suffix) throws Exception {
        Path file = Files.createTempFile("upload", suffix);
        Files.writeString(file, "{}");
        file.toFile().deleteOnExit();
        return file;
    }

    @Test
    void aFormWithAStreamedPartIsNotResent() {
        statuses.add(429);
        Upload once = Upload.of(new ByteArrayInputStream("PDFDATA".getBytes(StandardCharsets.UTF_8)), 7);
        try (Forms client = client()) {
            assertThrows(RateLimitException.class,
                    () -> client.uploads().create(Uploads.CreateBody.builder().image(once).build()));
        }
        assertEquals(1, bodies.size());
    }

    @Test
    void aFormOfReplayablePartsIsResent() {
        statuses.add(429);
        try (Forms client = client()) {
            Upload bytes = Upload.of("PDFDATA".getBytes(StandardCharsets.UTF_8));
            assertEquals("ok", client.uploads().create(Uploads.CreateBody.builder().image(bytes).build()).text());
        }
        assertEquals(2, bodies.size());
        assertTrue(bodies.get(1).contains("PDFDATA"), bodies.get(1));
    }

    @Test
    void aPartIsTypedByTheSpecUnlessOctetStreamThenByTheExtension() throws Exception {
        Path json = tempFile(".json");
        try (Forms client = client()) {
            client.uploads().create(Uploads.CreateBody.builder().image(Upload.of(json)).doc(Upload.of(json)).build());
            client.uploads().create(Uploads.CreateBody.builder()
                    .image(Upload.of(json).withContentType("text/csv"))
                    .doc(Upload.of(new byte[] {1}).withFilename("a.bin"))
                    .build());
        }
        String name = json.getFileName().toString();
        assertTrue(bodies.get(0).contains("name=\"image\"; filename=\"" + name + "\"\r\nContent-Type: image/png"), bodies.get(0));
        assertTrue(bodies.get(0).contains("name=\"doc\"; filename=\"" + name + "\"\r\nContent-Type: application/json"), bodies.get(0));
        assertTrue(bodies.get(1).contains("name=\"image\"; filename=\"" + name + "\"\r\nContent-Type: text/csv"), bodies.get(1));
        assertTrue(bodies.get(1).contains("filename=\"a.bin\"\r\nContent-Type: application/octet-stream"), bodies.get(1));
    }

    @Test
    void aMissingFileFailsWhenTheUploadIsMade() {
        Path missing = Path.of("no-such-dir", "missing.pdf");
        UncheckedIOException error = assertThrows(UncheckedIOException.class, () -> Upload.of(missing));
        assertInstanceOf(NoSuchFileException.class, error.getCause());
        assertThrows(UncheckedIOException.class, () -> Upload.of(missing.toFile()));
        assertThrows(UncheckedIOException.class, () -> Upload.of(Path.of(System.getProperty("java.io.tmpdir"))));
    }

    private List<String> deltas(Forms client) {
        List<String> texts = new ArrayList<>();
        try (EventStream<Delta> stream = client.deltas().list()) {
            stream.forEach(delta -> texts.add(delta.text().orElse("") + "/" + delta.error().orElse("")));
        }
        return texts;
    }

    @Test
    void anErrorPropertyTheEventDeclaresIsData() {
        events = "data: {\"text\":\"a\",\"error\":\"partial\"}\n\ndata: {\"text\":\"b\"}\n\n";
        try (Forms client = client()) {
            assertEquals(List.of("a/partial", "b/"), deltas(client));
        }
    }

    @Test
    void anErrorTheEventDoesNotDeclareIsThrownWhateverTheEventName() {
        events = "event: response.failed\ndata: {\"error\":{\"message\":\"boom\"}}\n\ndata: {\"delta\":\"x\"}\n\n";
        try (Forms client = client();
                EventStream<TranscriptionDelta> stream = client.transcriptions().createStream(
                        Transcriptions.CreateStreamBody.builder().file(Upload.of(new byte[] {1})).model("m").build())) {
            ApiException error = assertThrows(ApiException.class, () -> stream.forEach(delta -> { }));
            assertTrue(error.getMessage().endsWith("streamed an error: boom"), error.getMessage());
        }
        events = "event: error\ndata: {\"text\":\"gone\"}\n\n";
        try (Forms client = client()) {
            assertThrows(ApiException.class, () -> deltas(client));
        }
    }
}
