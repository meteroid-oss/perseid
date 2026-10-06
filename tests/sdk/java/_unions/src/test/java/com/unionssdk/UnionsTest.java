package com.unionssdk;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertFalse;
import static org.junit.jupiter.api.Assertions.assertTrue;

import com.fasterxml.jackson.databind.JsonNode;
import com.fasterxml.jackson.databind.ObjectMapper;
import com.sun.net.httpserver.HttpExchange;
import com.sun.net.httpserver.HttpServer;
import com.unionssdk.api.Grades;
import com.unionssdk.api.Transcriptions;
import com.unionssdk.models.Completion;
import com.unionssdk.models.CreateCompletionRequest;
import com.unionssdk.models.CreateTranscriptionRequest;
import com.unionssdk.models.GradeByScore;
import com.unionssdk.models.GradeByText;
import com.unionssdk.models.ImageRef;
import com.unionssdk.models.Include;
import com.unionssdk.models.ModelIds;
import com.unionssdk.models.ModelIdsWithRef;
import com.unionssdk.models.ResponseModel;
import com.unionssdk.models.Untypable;
import com.unionssdk.models.Voice;
import com.unionssdk.models.VoiceWithOpen;
import java.io.IOException;
import java.net.InetSocketAddress;
import java.nio.charset.StandardCharsets;
import java.util.ArrayList;
import java.util.Collections;
import java.util.List;
import java.util.concurrent.ExecutorService;
import java.util.concurrent.Executors;
import org.junit.jupiter.api.AfterEach;
import org.junit.jupiter.api.BeforeEach;
import org.junit.jupiter.api.Test;

/**
 * Decoding and encoding of unions sharing a JSON type, best-match object unions, union bodies and
 * open enums, on the SDK generated from tests/fixtures/edge-unions.yaml.
 */
class UnionsTest {
    private static final ObjectMapper PLAIN = new ObjectMapper();

    private HttpServer server;
    private ExecutorService executor;
    private String base;
    private final List<String> bodies = Collections.synchronizedList(new ArrayList<>());
    private volatile String reply = "{}";

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
        String text;
        // The spec declares tokenUrl "/oauth/token", an absolute-path reference that resolves
        // against the origin of the base URL, not under its "/v1" path.
        if (exchange.getRequestURI().getPath().equals("/oauth/token")) {
            text = "{\"access_token\":\"at\",\"token_type\":\"Bearer\",\"expires_in\":3600}";
        } else {
            bodies.add(body);
            text = reply;
        }
        byte[] bytes = text.getBytes(StandardCharsets.UTF_8);
        exchange.getResponseHeaders().add("Content-Type", "application/json");
        exchange.sendResponseHeaders(200, bytes.length);
        exchange.getResponseBody().write(bytes);
        exchange.close();
    }

    private UnionsSdk client() {
        return new UnionsSdk(
                null, UnionsSdkOptions.builder().baseUrl(base).clientCredentials("id", "secret").maxRetries(0).build());
    }

    private static Completion completion(String extra) {
        return Completion.fromJson(
                "{\"id\":\"c\",\"model\":\"alpha-1\",\"created_at\":\"2024-01-02T03:04:05Z\",\"choices\":[]"
                        + extra
                        + "}");
    }

    private static void assertJson(String expected, String actual) throws IOException {
        assertEquals(PLAIN.readTree(expected), PLAIN.readTree(actual));
    }

    @Test
    void promptsPickTheVariantOfTheirJsonValue() {
        assertTrue(completion(",\"prompt\":\"hi\"").prompt().get().isString());
        assertEquals("hi", completion(",\"prompt\":\"hi\"").prompt().get().asString());
        assertTrue(completion(",\"prompt\":[\"a\",\"b\"]").prompt().get().isArrayOfStrings());
        assertEquals(List.of("a", "b"), completion(",\"prompt\":[\"a\",\"b\"]").prompt().get().asArrayOfStrings());
        assertTrue(completion(",\"prompt\":[1,2]").prompt().get().isArrayOfIntegers());
        assertEquals(List.of(1L, 2L), completion(",\"prompt\":[1,2]").prompt().get().asArrayOfIntegers());
        assertTrue(completion(",\"prompt\":[[1],[2,3]]").prompt().get().isArrayOfIntegerArrays());
        assertEquals(
                List.of(List.of(1L), List.of(2L, 3L)),
                completion(",\"prompt\":[[1],[2,3]]").prompt().get().asArrayOfIntegerArrays());
    }

    @Test
    void anEmptyArrayIsTheFirstArrayVariant() {
        assertTrue(completion(",\"prompt\":[]").prompt().get().isArrayOfStrings());
    }

    @Test
    void valuesNoVariantFitsAreKeptAsReceived() throws IOException {
        Completion c = completion(",\"prompt\":[true]");
        assertTrue(c.prompt().get().isUnrecognized());
        assertEquals(PLAIN.readTree("[true]"), PLAIN.readTree(c.toJson()).get("prompt"));
    }

    @Test
    void promptsEncodeAsTheVariantTheyHold() throws IOException {
        CreateCompletionRequest.CreateCompletionRequestPrompt[] prompts = {
            CreateCompletionRequest.CreateCompletionRequestPrompt.ofString("hi"),
            CreateCompletionRequest.CreateCompletionRequestPrompt.ofArrayOfStrings(List.of("a")),
            CreateCompletionRequest.CreateCompletionRequestPrompt.ofArrayOfIntegers(List.of(1L, 2L)),
            CreateCompletionRequest.CreateCompletionRequestPrompt.ofArrayOfIntegerArrays(List.of(List.of(1L), List.of(2L))),
        };
        String[] wire = {"\"hi\"", "[\"a\"]", "[1,2]", "[[1],[2]]"};
        for (int i = 0; i < prompts.length; i++) {
            CreateCompletionRequest request =
                    CreateCompletionRequest.builder().model(ModelIds.of("alpha-1")).prompt(prompts[i]).build();
            JsonNode body = PLAIN.readTree(request.toJson());
            assertEquals(PLAIN.readTree(wire[i]), body.get("prompt"));
            assertEquals(prompts[i], CreateCompletionRequest.fromJson(request.toJson()).prompt());
        }
    }

    @Test
    void dateTimesFallBackToStrings() {
        assertTrue(completion("").createdAt().isDateTime());
        Completion later = Completion.fromJson(
                "{\"id\":\"c\",\"model\":\"alpha-1\",\"created_at\":\"yesterday\",\"choices\":[]}");
        assertTrue(later.createdAt().isString());
        assertEquals("yesterday", later.createdAt().asString());
        assertTrue(later.toJson().contains("\"created_at\":\"yesterday\""));
    }

    @Test
    void typeArraysBecomeUnionsOrNumbers() {
        Completion c = completion(",\"stop\":5,\"ratio\":1,\"score\":2.5,\"limit\":null");
        assertTrue(c.stop().get().isInteger());
        assertEquals(1.0, c.ratio().get());
        assertEquals(2.5, c.score().get());
        assertFalse(c.limit().isPresent());
        assertTrue(completion(",\"stop\":\"x\"").stop().get().isString());
    }

    @Test
    void objectUnionsPickTheBestMatchingVariant() throws IOException {
        String[] cases = {
            "\"auto\"",
            "{\"mode\":\"auto\",\"tools\":[{\"name\":\"f\"}]}",
            "{\"type\":\"web_search\"}",
            "{\"name\":\"f\",\"arguments\":\"{}\"}",
        };
        ResponseModel[] picked = new ResponseModel[cases.length];
        for (int i = 0; i < cases.length; i++) {
            picked[i] = ResponseModel.fromJson("{\"id\":\"r\",\"model\":\"alpha-1\",\"tool_choice\":" + cases[i] + "}");
            assertJson(cases[i], PLAIN.readTree(picked[i].toJson()).get("tool_choice").toString());
        }
        assertTrue(picked[0].toolChoice().get().isToolChoiceEnum());
        assertTrue(picked[1].toolChoice().get().isAllowedTools());
        assertTrue(picked[2].toolChoice().get().isHostedTool());
        assertTrue(picked[3].toolChoice().get().isFunctionTool());
        assertEquals("f", picked[3].toolChoice().get().asFunctionTool().name());
    }

    @Test
    void objectsNoVariantMatchesAreKept() {
        ResponseModel r = ResponseModel.fromJson("{\"id\":\"r\",\"model\":\"alpha-1\",\"tool_choice\":{\"unrelated\":true}}");
        assertTrue(r.toolChoice().get().isUnrecognized());
        assertTrue(r.toJson().contains("\"unrelated\":true"));
    }

    @Test
    void openEnumsKeepUnknownValues() {
        assertTrue(ModelIds.of("alpha-1").isKnown());
        assertTrue(ModelIds.of("beta-1").isKnown());
        assertFalse(ModelIds.of("gamma").isKnown());
        assertEquals("gamma", ModelIds.of("gamma").asString());
        assertTrue(ModelIdsWithRef.of("chat-large").isKnown());
        assertFalse(ModelIdsWithRef.of("other").isKnown());
    }

    @Test
    void enumsOfConstsAndEnumsMergeTheirValues() {
        for (String value : List.of("logprobs", "usage", "sources")) {
            assertTrue(Include.of(value).isKnown(), value);
        }
        assertFalse(Include.of("new").isKnown());
        for (String value : List.of("alloy", "ash", "coral", "sage")) {
            assertTrue(Voice.of(value).isKnown(), value);
            assertTrue(VoiceWithOpen.of(value).isKnown(), value);
        }
        assertFalse(Voice.of("verse").isKnown());
        assertEquals("verse", VoiceWithOpen.of("verse").toString());
    }

    @Test
    void openEnumFieldsDecodeInModels() {
        Completion c = completion(",\"include\":[\"usage\",\"future\"],\"voice_open\":\"sage\"");
        assertEquals("alpha-1", c.model().asString());
        assertTrue(c.include().get().get(0).isKnown());
        assertFalse(c.include().get().get(1).isKnown());
        assertEquals("sage", c.voiceOpen().get().asString());
        Completion other = Completion.fromJson(
                "{\"id\":\"c\",\"model\":\"never-seen\",\"created_at\":\"x\",\"choices\":[]}");
        assertEquals("never-seen", other.model().asString());
        assertTrue(other.toJson().contains("\"model\":\"never-seen\""));
    }

    @Test
    void anUntypableFieldDoesNotUntypeItsModel() {
        Untypable u = Untypable.fromJson("{\"name\":\"n\",\"mixed\":\"auto\",\"count\":3}");
        assertEquals("n", u.name());
        assertTrue(String.valueOf(u.mixed().get()).contains("auto"));
        assertEquals(3L, ((Number) u.count().get()).longValue());
    }

    @Test
    void requiredOnlyAlternativesLeaveAStruct() throws IOException {
        ImageRef image = ImageRef.fromJson("{\"file_id\":\"f\"}");
        assertEquals("f", image.fileId().get());
        assertFalse(image.imageUrl().isPresent());
        assertJson("{\"file_id\":\"f\"}", image.toJson());
    }

    @Test
    void unionResponseBodiesDecodeAsTheirVariant() {
        try (UnionsSdk client = client()) {
            reply = "{\"text\":\"hello\"}";
            Transcriptions.CreateTranscriptionResponse plain =
                    client.transcriptions()
                            .create(CreateTranscriptionRequest.builder().model(ModelIds.of("alpha-1")).fileId("f").build());
            assertTrue(plain.isTranscription());
            assertEquals("hello", plain.asTranscription().text());

            reply = "{\"text\":\"hello\",\"duration\":1.5,\"language\":\"en\"}";
            Transcriptions.CreateTranscriptionResponse verbose =
                    client.transcriptions()
                            .create(CreateTranscriptionRequest.builder().model(ModelIds.of("alpha-1")).fileId("f").build());
            assertTrue(verbose.isTranscriptionVerbose());
            assertEquals("en", verbose.asTranscriptionVerbose().language());
        }
    }

    @Test
    void listsOfUnionsDecodeEachItem() {
        try (UnionsSdk client = client()) {
            reply = "[{\"text\":\"a\"},{\"text\":\"b\",\"duration\":2,\"language\":\"fr\"}]";
            List<Transcriptions.TranscriptionResult> results = client.transcriptions().list();
            assertEquals(2, results.size());
            assertTrue(results.get(0).isTranscription());
            assertTrue(results.get(1).isTranscriptionVerbose());
        }
    }

    @Test
    void unionRequestBodiesAreSentAsTheirVariant() throws IOException {
        try (UnionsSdk client = client()) {
            reply = "{\"id\":\"g\"}";
            client.grades().create(Grades.CreateGradeRequest.ofGradeByText(GradeByText.builder().text("t").build()));
            client.grades().create(Grades.CreateGradeRequest.ofGradeByScore(GradeByScore.builder().score(0.5).build()));
        }
        assertEquals(2, bodies.size());
        assertJson("{\"text\":\"t\"}", bodies.get(0));
        assertJson("{\"score\":0.5}", bodies.get(1));
    }
}
