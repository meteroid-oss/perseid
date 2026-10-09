package com.forms;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertTrue;

import com.forms.api.Transcriptions;
import com.forms.models.TranscriptionDelta;
import com.forms.streaming.EventStream;
import com.forms.streaming.Upload;
import com.sun.net.httpserver.HttpServer;
import java.net.InetSocketAddress;
import java.nio.charset.StandardCharsets;
import java.nio.file.Files;
import java.nio.file.Path;
import java.util.ArrayList;
import java.util.Collections;
import java.util.List;
import org.junit.jupiter.api.AfterEach;
import org.junit.jupiter.api.BeforeEach;
import org.junit.jupiter.api.Test;

/** A form operation that also answers an event stream, on the SDK of tests/sdk/java/_forms. */
class FormsTest {
    private HttpServer server;
    private final List<String> bodies = Collections.synchronizedList(new ArrayList<>());

    @BeforeEach
    void start() throws Exception {
        server = HttpServer.create(new InetSocketAddress("127.0.0.1", 0), 0);
        server.createContext("/", exchange -> {
            String body = new String(exchange.getRequestBody().readAllBytes(), StandardCharsets.UTF_8);
            bodies.add(body);
            boolean stream = body.contains("name=\"stream\"\r\nContent-Length: 4\r\n\r\ntrue");
            byte[] reply = (stream ? "data: {\"delta\":\"hel\"}\n\ndata: {\"delta\":\"lo\"}\n\n" : "{\"text\":\"hello\"}")
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
}
