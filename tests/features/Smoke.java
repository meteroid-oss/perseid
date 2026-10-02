import com.fasterxml.jackson.databind.JsonNode;
import com.fasterxml.jackson.databind.ObjectMapper;
import com.fasterxml.jackson.databind.node.TextNode;
import com.features.ApiResponse;
import com.features.AsyncPage;
import com.features.Features;
import com.features.FeaturesOptions;
import com.features.Page;
import com.features.Paginator;
import com.features.RequestOptions;
import com.features.api.EncodingListScenariosHeadersOptions;
import com.features.api.Streaming;
import com.features.api.StreamingRetrieveEventsStreamOptions;
import com.features.api.WireBetaSearchOptions;
import com.features.api.WireSearchOptions;
import com.features.exceptions.ApiConnectionException;
import com.features.exceptions.ApiException;
import com.features.exceptions.AuthenticationException;
import com.features.exceptions.BadRequestException;
import com.features.exceptions.ConflictException;
import com.features.exceptions.FeaturesException;
import com.features.exceptions.InternalServerException;
import com.features.exceptions.InvalidDataException;
import com.features.exceptions.NotFoundException;
import com.features.exceptions.PermissionDeniedException;
import com.features.exceptions.UnprocessableEntityException;
import com.features.internal.Utils;
import com.features.models.Bag;
import com.features.models.BigBox;
import com.features.models.Blob;
import com.features.models.Charge;
import com.features.models.ChargeItemsItem;
import com.features.models.ChargeShipping;
import com.features.models.ChargeShippingAddress;
import com.features.models.CompletionChunk;
import com.features.models.CompletionRequest;
import com.features.models.DateBox;
import com.features.models.Entry;
import com.features.models.EnumValueMode;
import com.features.models.Event;
import com.features.models.Filter;
import com.features.models.FilterAmount;
import com.features.models.Gadget;
import com.features.models.Health;
import com.features.models.Item;
import com.features.models.ItemPatch;
import com.features.models.NullBag;
import com.features.models.Paint;
import com.features.models.Payment;
import com.features.models.SearchRange;
import com.features.models.Widget;
import com.features.models.WidgetList;
import com.features.streaming.EventStream;
import com.features.streaming.SseEvent;
import com.features.streaming.Upload;
import java.io.ByteArrayInputStream;
import java.net.URI;
import java.net.URLEncoder;
import java.net.http.HttpRequest;
import java.net.http.HttpResponse;
import java.nio.charset.StandardCharsets;
import java.time.Duration;
import java.time.OffsetDateTime;
import java.time.ZoneOffset;
import java.util.ArrayList;
import java.util.Arrays;
import java.util.Base64;
import java.util.LinkedHashMap;
import java.util.List;
import java.util.Map;
import java.util.Objects;
import java.util.Optional;
import java.util.concurrent.CompletableFuture;
import java.util.concurrent.CompletionException;
import java.util.concurrent.atomic.AtomicInteger;
import java.util.function.Function;
import java.util.stream.Collectors;

public class Smoke {
    static void expect(Object got, Object want) {
        if (!Objects.equals(got, want)) {
            throw new AssertionError("got " + got + ", want " + want);
        }
    }

    static <T> List<String> ids(Paginator<T> items, Function<T, String> id) {
        return items.stream().map(id).collect(Collectors.toList());
    }

    static byte[] bytes(String text) {
        return text.getBytes(StandardCharsets.UTF_8);
    }

    static FeaturesOptions.Builder options() {
        return FeaturesOptions.builder().baseUrl(System.getenv("FEATURES_URL"));
    }

    public static void main(String[] args) throws Exception {
        try (Features client = new Features("tok", options().build())) {
            run(client);
            scenarios(client);
            asyncScenarios();
        }
        System.out.println("java smoke test passed");
        // Clients that were never closed keep their idle OkHttp threads alive for a minute.
        System.exit(0);
    }

    static void run(Features client) throws Exception {
        expect(client.account().checkHealth().status(), "||");
        expect(client.account().retrieveMachine().status(), "Bearer tok||");
        expect(ids(client.widgets().listIter(), Widget::id), List.of("w1", "w2", "w3"));
        expect(ids(client.widgets().listEventsIter("w1", "created"), Event::id), List.of("e1", "e2", "e3"));
        expect(ids(client.gadgets().listIter(), Gadget::id), List.of("g1", "g2", "g3"));
        expect(ids(client.records().listIter(), Entry::id), List.of("r1", "r2", "r3"));

        Page<Widget> first = client.widgets().listIter().firstPage();
        expect(first.items().stream().map(Widget::id).collect(Collectors.toList()), List.of("w1", "w2"));
        expect(first.hasNextPage(), true);
        Page<Widget> second = first.nextPage();
        expect(second.items().get(0).id(), "w3");
        expect(second.hasNextPage(), false);
        List<Integer> sizes = new ArrayList<>();
        client.gadgets().listIter().pages().forEach(page -> sizes.add(page.items().size()));
        expect(sizes, List.of(2, 1));

        Widget widget = client.widgets().list().data().get(0);
        expect(widget.additionalProperties().get("color"), TextNode.valueOf("red"));
        expect(widget.toJson(), "{\"id\":\"w1\",\"name\":\"w1\",\"color\":\"red\"}");
        expect(Widget.builder().id("w9").name("n").build().toBuilder().name("m").build().name(), "m");
        try {
            Widget.builder().id("w9").build();
            throw new AssertionError("expected a missing name");
        } catch (IllegalStateException expected) {
            expect(expected.getMessage(), "`name` is required, but was not set");
        }

        ApiResponse<WidgetList> raw = client.withRawResponse().widgets().list();
        expect(raw.statusCode(), 200);
        expect(raw.headers().get("x-request-id"), "req_mock");
        expect(raw.requestId().orElseThrow(), "req_mock");
        expect(raw.body().data().size(), 2);

        expect(client.async().widgets().listIter().toList().get().stream().map(Widget::id).collect(Collectors.toList()), List.of("w1", "w2", "w3"));
        AsyncPage<Event> events = client.async().widgets().listEventsIter("w1", "created").firstPage().get();
        expect(events.nextPage().get().items().get(0).id(), "e3");
        expect(client.async().withRawResponse().account().checkHealth().get().requestId().orElseThrow(), "req_mock");
        try {
            new Features(null, options().build()).async().widgets().list().join();
            throw new AssertionError("expected an authentication error");
        } catch (CompletionException expected) {
            expect(((AuthenticationException) expected.getCause()).statusCode(), 401);
        }

        expect(new Features(null, options().basicAuth("u", "p").build()).account().createSession().status(), "Basic dTpw||");
        expect(new Features(null, options().tokenProvider(() -> "fresh").build()).account().retrieveMachine().status(), "Bearer fresh||");
        expect(ids(new Features(null, options().putApiKey("api_key", "k").build()).widgets().listIter(), Widget::id), List.of("w1", "w2", "w3"));
        try {
            new Features(null, options().build()).widgets().listIter().iterator().hasNext();
            throw new AssertionError("expected an authentication error");
        } catch (AuthenticationException expected) {
            expect(expected.statusCode(), 401);
            expect(expected.error().orElseThrow().toString(), "{\"error\":\"unauthorized\"}");
        }

        StreamingRetrieveEventsStreamOptions topic = StreamingRetrieveEventsStreamOptions.builder().topic("news").build();
        try (EventStream<SseEvent> stream = client.streaming().retrieveEventsStream(topic)) {
            expect(
                    stream.stream().collect(Collectors.toList()),
                    List.of(
                            new SseEvent("greeting", "news", "1", null),
                            new SseEvent("message", "line1\nline2", "1", null),
                            new SseEvent("message", "{\"n\": 3}", "3", Duration.ofMillis(1500))));
        }
        CompletionRequest prompt = CompletionRequest.builder().prompt("ab").build();
        expect(client.streaming().createCompletion(prompt).text(), "AB");
        try (EventStream<CompletionChunk> chunks = client.streaming().createCompletionStream(prompt)) {
            List<String> deltas = new ArrayList<>();
            for (CompletionChunk chunk : chunks) {
                deltas.add(chunk.delta());
                expect(chunks.lastEvent().event(), "message");
            }
            expect(deltas, List.of("a", "b"));
        }
        expect(prompt.stream().isPresent(), false);
        try (EventStream<CompletionChunk> chunks = client.async().streaming().createCompletionStream(prompt).get()) {
            expect(chunks.stream().map(c -> c.additionalProperties().get("index").asInt()).collect(Collectors.toList()), List.of(0, 1));
        }

        Streaming.UploadFileBody body = Streaming.UploadFileBody.builder()
                .file(Upload.of(bytes("hello")).withFilename("a.txt").withContentType("text/plain"))
                .name("doc")
                .count(2)
                .meta(Health.builder().status("ok").build())
                .tags(List.of("a", "b"))
                .build();
        expect(
                client.streaming().uploadFile(body).status(),
                "count=::2;file=a.txt:text/plain:hello;meta=:application/json:{\"status\":\"ok\"};name=::doc;tags=::a;tags=::b");
        expect(client.streaming().uploadContent("f1", Upload.of(bytes("raw"))).status(), "application/octet-stream:raw");
        Upload streamed = Upload.of(new ByteArrayInputStream(bytes("streamed")), -1);
        expect(client.streaming().uploadContent("f1", streamed).status(), "application/octet-stream:streamed");

        WireSearchOptions search = WireSearchOptions.builder()
                .filter(Filter.builder().status("open").amount(FilterAmount.builder().gte(5L).build()).build())
                .expand(List.of("a", "b"))
                .metadata(Map.of("k", "v"))
                .ids(WireSearchOptions.Ids.ofArrayOfStrings(List.of("x", "y")))
                .tags(List.of("t1", "t2"))
                .range(SearchRange.builder().gte(1L).lt(9L).build())
                .build();
        expect(
                client.wire().search(search).status(),
                "expand[]=a&expand[]=b&filter[amount][gte]=5&filter[status]=open&ids=x&ids=y"
                        + "&metadata[k]=v&range[gte]=1&range[lt]=9&tags=t1,t2");
        Charge charge = Charge.builder()
                .amount(100L)
                .capture(true)
                .metadata(Map.of("order", "7"))
                .items(List.of(
                        ChargeItemsItem.builder().price("p1").quantity(2L).build(),
                        ChargeItemsItem.builder().price("p2").build()))
                .expand(List.of("customer"))
                .statuses(List.of("a", "b"))
                .codes(List.of("c1", "c2"))
                .shipping(ChargeShipping.builder()
                        .address(ChargeShippingAddress.builder().line1("1 Main").city("Paris").build())
                        .build())
                .build();
        expect(
                client.wire().createCharge(charge).status(),
                "application/x-www-form-urlencoded|amount=100&capture=true&codes=c1,c2&expand[]=customer"
                        + "&items[0][price]=p1&items[0][quantity]=2&items[1][price]=p2&metadata[order]=7"
                        + "&shipping[address][city]=Paris&shipping[address][line1]=1 Main&statuses=a&statuses=b");
        WireBetaSearchOptions beta = WireBetaSearchOptions.builder().limit(2).features("x,y").build();
        expect(client.wire().betaSearch(beta).status(), "beta=true&limit=2|features=x,y");
        expect(client.wire().updateImage("42", Upload.of(bytes("png"))).status(), "42:image/png:png");
    }

    // ---- Scenarios of tests/features/SCENARIOS.md ----

    private static final ObjectMapper MAPPER = new ObjectMapper();
    private static final String URL = System.getenv("FEATURES_URL");
    private static final AtomicInteger SCENARIO_IDS = new AtomicInteger();
    private static final java.net.http.HttpClient PLAIN = java.net.http.HttpClient.newHttpClient();
    private static final byte[] HELLO = {'h', 'e', 'l', 'l', 'o', (byte) 0xFB, (byte) 0xFF, (byte) 0xFE};
    private static final OffsetDateTime INSTANT = OffsetDateTime.of(2024, 1, 2, 3, 4, 5, 250_000_000, ZoneOffset.UTC);
    private static final List<SseEvent> TICKS = List.of(
            new SseEvent("message", "first", null, null),
            new SseEvent("tick", "line1\nline2", "7", null),
            new SseEvent("message", "{\"n\": 3}", "7", Duration.ofMillis(2500)),
            new SseEvent("message", "tail", "7", null));

    /** A call that may throw. */
    interface Call {
        void run() throws Exception;
    }

    /** Uses a client that is closed afterwards. */
    interface Use<T> {
        T apply(Features client) throws Exception;
    }

    /** The error `call` throws, which must be a `kind`. */
    static <E extends Throwable> E raises(Class<E> kind, Call call) {
        try {
            call.run();
        } catch (Throwable error) {
            if (kind.isInstance(error)) {
                return kind.cast(error);
            }
            throw new AssertionError("expected " + kind.getSimpleName() + ", got " + error, error);
        }
        throw new AssertionError("expected " + kind.getSimpleName());
    }

    static <T> T using(String key, FeaturesOptions.Builder options, Use<T> use) throws Exception {
        try (Features client = new Features(key, options.build())) {
            return use.apply(client);
        }
    }

    /** A scenario id of the server, unique to this run of one scenario. */
    static String scenarioId(String name) {
        return "java-" + name + "-" + SCENARIO_IDS.incrementAndGet();
    }

    /** What the server saw for a scenario: its attempts and idempotency keys. */
    static JsonNode serverState(String scenario) throws Exception {
        HttpRequest request = HttpRequest.newBuilder(
                        URI.create(URL + "/__server/attempts/" + URLEncoder.encode(scenario, StandardCharsets.UTF_8)))
                .build();
        return MAPPER.readTree(PLAIN.send(request, HttpResponse.BodyHandlers.ofString()).body());
    }

    static long attempts(String scenario) throws Exception {
        return serverState(scenario).get("attempts").asLong();
    }

    /** Whether a result is the absence of a value: null, or an empty Optional. */
    static boolean isNothing(Object value) {
        return value == null || Optional.empty().equals(value);
    }

    /**
     * The JSON body of an API error, which is parsed too: as the declared error model when the
     * operation declares one, else as plain JSON.
     */
    static JsonNode errorJson(ApiException error, boolean declared) throws Exception {
        JsonNode body = MAPPER.readTree(error.body());
        Object parsed = error.error().orElseThrow(() -> new AssertionError("no parsed error body: " + error));
        expect(parsed instanceof JsonNode, !declared);
        expect(Utils.getObjectMapper().valueToTree(parsed), body);
        expect(error.requestId().orElseThrow(), "req_mock");
        return body;
    }

    static void scenarios(Features client) throws Exception {
        // Items: methods and empty responses.
        Item item = client.items().retrieve("i1");
        expect(item.id(), "i1");
        expect(item.name(), "first");
        expect(item.note().orElseThrow(), "hi");
        Item patched = client.items().update("i1", ItemPatch.builder().name("renamed").build());
        expect(patched.name(), "renamed");
        expect(patched.note().isPresent(), false);
        client.items().delete("i1");

        // Content and decoding.
        expect(isNothing(client.content().retrieveScenariosNullableBody()), true);
        expect(client.content().retrieveScenariosText(), "hello text\n");
        expect(client.content().retrieveScenariosCsv(), "id,name\n1,alpha\n2,\"be,ta\"\n");
        byte[] blob = client.content().downloadBlob();
        expect(blob.length, 256);
        for (int i = 0; i < 256; i++) {
            expect((int) blob[i] & 0xFF, i);
        }
        byte[] image = new byte[24];
        System.arraycopy(new byte[] {(byte) 0x89, 'P', 'N', 'G', '\r', '\n', 0x1A, '\n'}, 0, image, 0, 8);
        for (int i = 0; i < 4; i++) {
            System.arraycopy(new byte[] {0x00, 0x01, (byte) 0xFE, (byte) 0xFF}, 0, image, 8 + 4 * i, 4);
        }
        expect(Arrays.equals(client.content().downloadImage(), image), true);
        raises(InvalidDataException.class, () -> client.content().retrieveScenariosMalformed());
        raises(InvalidDataException.class, () -> client.content().retrieveScenariosEmptyBody());
        Health extras = client.content().listScenariosExtraFields();
        expect(extras.status(), "ok");
        expect(extras.additionalProperties().get("extra").asInt(), 1);
        expect(extras.additionalProperties().get("nested").get("a").get(2).has("b"), true);

        NullBag nulls = client.content().listScenariosNulls();
        expect(nulls.name().isPresent(), false);
        expect(nulls.tags(), Arrays.asList("a", null, "b"));
        Map<String, Long> counts = new LinkedHashMap<>();
        counts.put("x", 1L);
        counts.put("y", null);
        expect(nulls.counts(), counts);
        expect(nulls.note().isPresent(), false);
        Map<String, Long> sentCounts = new LinkedHashMap<>();
        sentCounts.put("x", null);
        sentCounts.put("y", 2L);
        NullBag sent = NullBag.builder().name(null).tags(Arrays.asList("a", null)).counts(sentCounts).build();
        JsonNode sentJson = MAPPER.readTree("{\"name\":null,\"tags\":[\"a\",null],\"counts\":{\"x\":null,\"y\":2}}");
        expect(MAPPER.readTree(sent.toJson()), sentJson);
        NullBag echoed = client.content().createScenariosNull(sent);
        expect(echoed.name().isPresent(), false);
        expect(echoed.tags(), Arrays.asList("a", null));
        expect(echoed.counts(), sentCounts);
        expect(echoed.note().isPresent(), false);
        expect(MAPPER.readTree(echoed.toJson()), sentJson);

        Bag bag = client.content().retrieveScenariosBag();
        expect(bag.id(), "b1");
        expect(bag.additionalProperties().get("a").asInt(), 1);
        expect(bag.additionalProperties().get("b").asInt(), 2);
        expect(bag.additionalProperties().size(), 2);
        Map<String, ? extends Number> typedBag = bag.typedAdditionalProperties();
        expect(typedBag.get("a").longValue(), 1L);
        expect(typedBag.get("b").longValue(), 2L);
        Object labels = client.content().listScenariosLabels();
        expect(labels, Map.of("k", "v", "z", "y"));
        Paint known = client.content().retrieveScenariosEnum(EnumValueMode.of("known"));
        expect(known.kind().asString(), "red");
        expect(known.kind().isKnown(), true);
        expect(known.kinds().orElseThrow().stream().map(k -> k.asString()).collect(Collectors.toList()), List.of("green", "blue"));
        Paint unknown = client.content().retrieveScenariosEnum(EnumValueMode.of("unknown"));
        expect(unknown.kind().asString(), "magenta");
        expect(unknown.kind().isKnown(), false);
        expect(unknown.kinds().orElseThrow().stream().map(k -> k.asString()).collect(Collectors.toList()), List.of("red", "magenta"));
        expect(unknown.kinds().orElseThrow().get(0).isKnown(), true);

        // Encoding.
        BigBox big = client.encoding().scenariosBigint(BigBox.builder().value(9007199254740993L).min(Long.MIN_VALUE).build());
        expect(big.value(), 9007199254740993L);
        expect(big.min(), Long.MIN_VALUE);
        String base64 = Base64.getEncoder().encodeToString(HELLO);
        expect(base64, "aGVsbG/7//4=");
        expect(Arrays.equals(Base64.getDecoder().decode(client.encoding().createScenariosByte(Blob.builder().data(base64).build()).data()), HELLO), true);
        expect(Arrays.equals(Base64.getDecoder().decode(client.encoding().listScenariosBytes().data()), HELLO), true);
        for (ZoneOffset zone : List.of(ZoneOffset.UTC, ZoneOffset.ofHours(2), ZoneOffset.ofHoursMinutes(-5, -30))) {
            OffsetDateTime at = INSTANT.withOffsetSameInstant(zone);
            DateBox queried = client.encoding().retrieveScenariosDatetime(at, "2024-01-02");
            expect(queried.at().toInstant(), INSTANT.toInstant());
            expect(queried.day(), "2024-01-02");
            DateBox posted = client.encoding().scenariosDatetime(DateBox.builder().at(at).day("2024-01-02").build());
            expect(posted.at().toInstant(), INSTANT.toInstant());
            expect(posted.day(), "2024-01-02");
        }
        for (String value : List.of("plain", "sp ace", "sl/ash", "q?mark", "per%cent", "ha#sh", "lit%25eral", "a+b", "héllo wörld ✓")) {
            expect(client.encoding().retrieveScenarioPath(value).status(), value);
        }
        for (String value : List.of("plain", "sp ace", "a&b=c+d", "100%", "slash/qm?", "héllo wörld ✓")) {
            expect(client.encoding().retrieveScenariosQuery(value).status(), value);
        }
        expect(client.encoding().retrieveScenariosMulti(List.of("b", "a", "c"), true).status(), "b,a,c");
        expect(client.encoding().listScenariosHeaders("acme").status(), "acme|");
        EncodingListScenariosHeadersOptions traced = EncodingListScenariosHeadersOptions.builder().xTraceId("t1").build();
        expect(client.encoding().listScenariosHeaders("acme", traced).status(), "acme|t1");

        // Cookies.
        expect(client.cookies().retrieveScenariosCookie("abc123").status(), "abc123");
        expect(using(null, options().putApiKey("api_key_cookie", "ck1"), c -> c.cookies().retrieveScenariosCookieAuth().status()), "ck1");
        raises(AuthenticationException.class, () -> using(null, options(), c -> c.cookies().retrieveScenariosCookieAuth()));

        // OAuth2 client credentials.
        String secret = "p@ss word";
        try (Features cached = new Features(null, options().clientCredentials("java-oauth", secret).build())) {
            expect(cached.account().retrieveMachine().status(), "Bearer at-java-oauth-1||");
            expect(cached.account().retrieveMachine().status(), "Bearer at-java-oauth-1||");
        }
        expect(attempts("java-oauth"), 1L);
        expect(using(null, options().clientCredentials("java-oauth-body", secret).clientAuthInBody(true), c -> c.account().retrieveMachine().status()), "Bearer at-java-oauth-body-1||");
        expect(using(null, options().clientCredentials("java-oauth-revoked", secret).maxRetries(0), c -> c.account().retrieveMachine().status()), "Bearer at-java-oauth-revoked-2||");
        expect(attempts("java-oauth-revoked"), 2L);
        AuthenticationException invalid = raises(AuthenticationException.class, () -> using(
                null, options().clientCredentials("java-oauth-bad", "wrong").maxRetries(0), c -> c.account().retrieveMachine()));
        expect(MAPPER.readTree(invalid.body()).get("error").asText(), "invalid_client");
        expect(using("tok", options().clientCredentials("java-oauth", secret), c -> c.account().retrieveMachine().status()), "Bearer tok||");

        // Retries and idempotency.
        String flaky = scenarioId("flaky");
        long started = System.nanoTime();
        expect(client.retries().retrieveScenariosFlaky(flaky).status(), "attempt=2");
        expect((System.nanoTime() - started) / 1_000_000 >= 900, true);
        expect(attempts(flaky), 2L);
        RequestOptions never = RequestOptions.builder().maxRetries(0).build();
        String unretried = scenarioId("flaky-once");
        InternalServerException once = raises(InternalServerException.class, () -> client.retries().retrieveScenariosFlaky(unretried, never));
        expect(once.statusCode(), 503);
        expect(errorJson(once, false).get("error").asText(), "unavailable");
        expect(attempts(unretried), 1L);
        String limited = scenarioId("rate-limited");
        started = System.nanoTime();
        expect(client.retries().retrieveScenariosRateLimited(limited).status(), "attempt=2");
        expect((System.nanoTime() - started) / 1_000_000 >= 500, true);
        expect(attempts(limited), 2L);
        String down = scenarioId("unavailable");
        RequestOptions twice = RequestOptions.builder().maxRetries(2).build();
        InternalServerException exhausted = raises(InternalServerException.class, () -> client.retries().retrieveScenariosUnavailable(down, twice));
        expect(exhausted.statusCode(), 503);
        expect(attempts(down), 3L);
        String idempotent = scenarioId("idempotent");
        expect(client.retries().scenariosIdempotent(idempotent, "idem-1", Payment.builder().amount(5).build()).status(), "attempts=2;key=idem-1");
        expect(serverState(idempotent).get("keys"), MAPPER.readTree("[\"idem-1\",\"idem-1\"]"));

        // Errors and pagination.
        Map<Integer, Class<? extends ApiException>> statusErrors = new LinkedHashMap<>();
        statusErrors.put(400, BadRequestException.class);
        statusErrors.put(401, AuthenticationException.class);
        statusErrors.put(403, PermissionDeniedException.class);
        statusErrors.put(404, NotFoundException.class);
        statusErrors.put(409, ConflictException.class);
        statusErrors.put(422, UnprocessableEntityException.class);
        List<String> sentRequests = new ArrayList<>();
        FeaturesOptions.Builder counting = options().addInterceptor(chain -> {
            sentRequests.add(chain.request().url().encodedPath());
            return chain.proceed(chain.request());
        });
        try (Features counted = new Features("tok", counting.build())) {
            for (Map.Entry<Integer, Class<? extends ApiException>> entry : statusErrors.entrySet()) {
                sentRequests.clear();
                int code = entry.getKey();
                ApiException error = raises(ApiException.class, () -> counted.errors().retrieveScenarioStatus(String.valueOf(code)));
                expect(error.getClass(), entry.getValue());
                expect(error.statusCode(), code);
                JsonNode body = errorJson(error, true);
                expect(body.get("error").asText(), "status " + code);
                expect(body.get("code").asInt(), code);
                expect(sentRequests.size(), 1);
            }
        }
        List<String> seen = new ArrayList<>();
        ConflictException gone = raises(ConflictException.class, () -> {
            for (Widget widget : client.errors().listScenariosPagesIter()) {
                seen.add(widget.id());
            }
        });
        expect(seen, List.of("p1", "p2"));
        expect(gone.statusCode(), 409);
        JsonNode goneBody = errorJson(gone, true);
        expect(goneBody.get("error").asText(), "page_gone");
        expect(goneBody.get("code").asInt(), 409);

        // Streaming.
        try (EventStream<SseEvent> sse = client.streaming().retrieveScenariosSse()) {
            expect(sse.stream().collect(Collectors.toList()), TICKS);
        }
        PermissionDeniedException denied = raises(PermissionDeniedException.class, () -> client.streaming().retrieveScenariosSseError());
        expect(denied.statusCode(), 403);
        JsonNode deniedBody = errorJson(denied, true);
        expect(deniedBody.get("error").asText(), "forbidden");
        expect(deniedBody.get("code").asInt(), 403);

        // A failed connection is not an API error.
        ApiConnectionException refused = raises(ApiConnectionException.class, () -> using(
                "tok", FeaturesOptions.builder().baseUrl("http://127.0.0.1:9").maxRetries(0), c -> c.account().checkHealth()));
        expect(refused instanceof FeaturesException, true);
        expect(((Object) refused) instanceof ApiException, false);
        // The strict mock answers an unserved path with what it received, base URL path included.
        NotFoundException wrongPath = raises(NotFoundException.class, () -> using(
                "tok", options().baseUrl(URL + "/v0"), c -> c.account().checkHealth()));
        expect(MAPPER.readTree(wrongPath.body()).get("received").get("path").asText(), "/v0/health");
    }

    static void asyncScenarios() throws Exception {
        try (Features owner = new Features(null, options().clientCredentials("java-oauth-async-revoked", "p@ss word").build())) {
            expect(owner.async().account().retrieveMachine().get().status(), "Bearer at-java-oauth-async-revoked-2||");
        }
        try (Features owner = new Features("tok", options().build())) {
            com.features.FeaturesAsync client = owner.async();
            expect(client.items().retrieve("i1").get().name(), "first");
            expect(client.items().update("i1", ItemPatch.builder().name("renamed").build()).get().note().isPresent(), false);
            client.items().delete("i1").get();
            expect(isNothing(client.content().retrieveScenariosNullableBody().get()), true);
            expect(client.content().retrieveScenariosText().get(), "hello text\n");
            expect(client.content().downloadBlob().get().length, 256);
            expect(asyncCause(client.content().retrieveScenariosMalformed()) instanceof InvalidDataException, true);
            expect(asyncCause(client.content().retrieveScenariosEmptyBody()) instanceof InvalidDataException, true);
            expect(client.content().listScenariosExtraFields().get().status(), "ok");
            BigBox big = client.encoding().scenariosBigint(BigBox.builder().value(9007199254740993L).min(Long.MIN_VALUE).build()).get();
            expect(big.value(), 9007199254740993L);
            expect(big.min(), Long.MIN_VALUE);
            for (String value : List.of("sp ace", "sl/ash", "q?mark", "per%cent", "héllo wörld ✓")) {
                expect(client.encoding().retrieveScenarioPath(value).get().status(), value);
            }
            expect(client.encoding().retrieveScenariosQuery("a&b=c+d").get().status(), "a&b=c+d");
            String flaky = scenarioId("async-flaky");
            long started = System.nanoTime();
            expect(client.retries().retrieveScenariosFlaky(flaky).get().status(), "attempt=2");
            expect((System.nanoTime() - started) / 1_000_000 >= 900, true);
            expect(attempts(flaky), 2L);
            String idempotent = scenarioId("async-idempotent");
            expect(client.retries().scenariosIdempotent(idempotent, "idem-1", Payment.builder().amount(5).build()).get().status(), "attempts=2;key=idem-1");
            expect(serverState(idempotent).get("keys"), MAPPER.readTree("[\"idem-1\",\"idem-1\"]"));
            expect(asyncCause(client.errors().retrieveScenarioStatus("403")) instanceof PermissionDeniedException, true);
            expect(asyncCause(client.errors().retrieveScenarioStatus("404")) instanceof NotFoundException, true);
            expect(asyncCause(client.errors().retrieveScenarioStatus("422")) instanceof UnprocessableEntityException, true);
            List<String> seen = new ArrayList<>();
            Throwable gone = asyncCause(client.errors().listScenariosPagesIter().forEach(widget -> seen.add(widget.id())));
            expect(gone instanceof ConflictException, true);
            expect(((ConflictException) gone).statusCode(), 409);
            expect(seen, List.of("p1", "p2"));
            try (EventStream<SseEvent> sse = client.streaming().retrieveScenariosSse().get()) {
                expect(sse.stream().collect(Collectors.toList()), TICKS);
            }
            expect(asyncCause(client.streaming().retrieveScenariosSseError()) instanceof PermissionDeniedException, true);

            // Cancelling a call that waits to retry stops it: the server sees one attempt only.
            String cancelled = scenarioId("async-cancel");
            CompletableFuture<Health> pending = client.retries().retrieveScenariosFlaky(cancelled);
            for (int i = 0; i < 100 && attempts(cancelled) < 1; i++) {
                Thread.sleep(20);
            }
            expect(attempts(cancelled), 1L);
            pending.cancel(true);
            Thread.sleep(1400);
            expect(attempts(cancelled), 1L);
            expect(pending.isCancelled(), true);
        }
        expect(using(null, options().tokenProvider(() -> "async-fresh"), c -> c.async().account().retrieveMachine().get().status()), "Bearer async-fresh||");
    }

    /** The exception a future fails with. */
    static Throwable asyncCause(CompletableFuture<?> future) {
        try {
            future.join();
        } catch (CompletionException error) {
            return error.getCause();
        }
        throw new AssertionError("expected the future to fail");
    }
}
