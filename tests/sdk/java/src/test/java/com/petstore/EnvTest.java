package com.petstore;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertTrue;

import com.sun.net.httpserver.HttpServer;
import java.io.File;
import java.net.InetSocketAddress;
import java.nio.charset.StandardCharsets;
import java.util.ArrayList;
import java.util.List;
import org.junit.jupiter.api.AfterEach;
import org.junit.jupiter.api.BeforeEach;
import org.junit.jupiter.api.Test;

class EnvTest {
    private static final String PET = "{\"id\":\"1\",\"name\":\"Rex\",\"created_at\":\"2024-01-01T00:00:00Z\"}";

    private HttpServer server;
    private final List<String> authorizations = new ArrayList<>();

    /** Runs in a child JVM, whose environment the test sets. */
    public static void main(String[] args) {
        Petstore petstore = args.length == 0
                ? Petstore.fromEnv()
                : new Petstore("explicit", PetstoreOptions.builder().baseUrl(args[0]).build());
        try (petstore) {
            System.out.print(petstore.pets().retrieve("1").name());
        }
    }

    @BeforeEach
    void start() throws Exception {
        server = HttpServer.create(new InetSocketAddress("127.0.0.1", 0), 0);
        server.createContext("/", exchange -> {
            authorizations.add(exchange.getRequestHeaders().getFirst("Authorization"));
            byte[] body = PET.getBytes(StandardCharsets.UTF_8);
            exchange.getResponseHeaders().add("content-type", "application/json");
            exchange.sendResponseHeaders(200, body.length);
            exchange.getResponseBody().write(body);
            exchange.close();
        });
        server.start();
    }

    @AfterEach
    void stop() {
        server.stop(0);
    }

    private String run(String baseUrlEnv, String... args) throws Exception {
        List<String> command = new ArrayList<>(List.of(
                System.getProperty("java.home") + File.separator + "bin" + File.separator + "java",
                "-cp",
                System.getProperty("java.class.path"),
                EnvTest.class.getName()));
        command.addAll(List.of(args));
        ProcessBuilder child = new ProcessBuilder(command).redirectError(ProcessBuilder.Redirect.DISCARD);
        child.environment().put("PETSTORE_API_KEY", "from-env");
        child.environment().put("PETSTORE_BASE_URL", baseUrlEnv);
        Process process = child.start();
        String output = new String(process.getInputStream().readAllBytes(), StandardCharsets.UTF_8);
        assertEquals(0, process.waitFor(), output);
        return output;
    }

    @Test
    void theApiKeyAndBaseUrlComeFromTheEnvironment() throws Exception {
        String url = "http://127.0.0.1:" + server.getAddress().getPort();
        assertEquals("Rex", run(url));
        assertEquals(List.of("Bearer from-env"), authorizations);
    }

    @Test
    void theServerOfTheSpecIsTheDefaultBaseUrl() {
        assertTrue(Petstore.DEFAULT_BASE_URL.startsWith("https://petstore.example.com"), Petstore.DEFAULT_BASE_URL);
    }

    @Test
    void explicitSettingsWinOverTheEnvironment() throws Exception {
        String url = "http://127.0.0.1:" + server.getAddress().getPort();
        assertEquals("Rex", run("http://127.0.0.1:1", url));
        assertEquals(List.of("Bearer explicit"), authorizations);
    }
}
