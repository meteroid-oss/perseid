import com.fasterxml.jackson.databind.node.TextNode;
import com.features.ApiResponse;
import com.features.AsyncPage;
import com.features.Features;
import com.features.FeaturesOptions;
import com.features.Page;
import com.features.Paginator;
import com.features.api.Streaming;
import com.features.api.StreamingRetrieveEventsStreamOptions;
import com.features.api.WireBetaSearchOptions;
import com.features.api.WireSearchOptions;
import com.features.exceptions.AuthenticationException;
import com.features.exceptions.InvalidDataException;
import com.features.models.Charge;
import com.features.models.ChargeItemsItem;
import com.features.models.ChargeShipping;
import com.features.models.ChargeShippingAddress;
import com.features.models.CompletionChunk;
import com.features.models.CompletionRequest;
import com.features.models.Entry;
import com.features.models.Event;
import com.features.models.Filter;
import com.features.models.FilterAmount;
import com.features.models.Gadget;
import com.features.models.Health;
import com.features.models.SearchRange;
import com.features.models.Widget;
import com.features.models.WidgetList;
import com.features.streaming.EventStream;
import com.features.streaming.SseEvent;
import com.features.streaming.Upload;
import java.io.ByteArrayInputStream;
import java.nio.charset.StandardCharsets;
import java.time.Duration;
import java.util.ArrayList;
import java.util.List;
import java.util.Map;
import java.util.Objects;
import java.util.concurrent.CompletionException;
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
        }
        System.out.println("java smoke test passed");
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
                .ids(WireSearchOptions.Ids.ofList(List.of("x", "y")))
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
}
