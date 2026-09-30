import com.features.Features;
import com.features.FeaturesOptions;
import com.features.Paginator;
import com.features.api.Streaming;
import com.features.api.StreamingStreamEventsOptions;
import com.features.api.WidgetsListWidgetEventsOptions;
import com.features.models.Event;
import com.features.models.Gadget;
import com.features.models.Health;
import com.features.models.Entry;
import com.features.models.Widget;
import com.features.streaming.EventStream;
import com.features.streaming.SseEvent;
import com.features.streaming.Upload;
import java.io.ByteArrayInputStream;
import java.nio.charset.StandardCharsets;
import java.time.Duration;
import java.util.List;
import java.util.Map;
import java.util.Objects;
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

    static FeaturesOptions options() {
        FeaturesOptions options = new FeaturesOptions();
        options.setServerUrl(System.getenv("FEATURES_URL"));
        return options;
    }

    public static void main(String[] args) throws Exception {
        Features client = new Features("tok", options());
        expect(client.getAccount().health().getStatus(), "||");
        expect(client.getAccount().machineStatus().getStatus(), "Bearer tok||");
        expect(ids(client.getWidgets().listWidgetsIter(), Widget::getId), List.of("w1", "w2", "w3"));
        WidgetsListWidgetEventsOptions events = new WidgetsListWidgetEventsOptions();
        events.setKind("created");
        expect(ids(client.getWidgets().listWidgetEventsIter("w1", events), Event::getId), List.of("e1", "e2", "e3"));
        expect(ids(client.getGadgets().listGadgetsIter(), Gadget::getId), List.of("g1", "g2", "g3"));
        expect(ids(client.getRecords().listRecordsIter(), Entry::getId), List.of("r1", "r2", "r3"));

        FeaturesOptions basic = options();
        basic.setBasicAuth("u", "p");
        expect(new Features(null, basic).getAccount().createSession().getStatus(), "Basic dTpw||");
        FeaturesOptions provided = options();
        provided.setTokenProvider(() -> "fresh");
        expect(new Features(null, provided).getAccount().machineStatus().getStatus(), "Bearer fresh||");
        FeaturesOptions keyed = options();
        keyed.setApiKeys(Map.of("api_key", "k"));
        expect(ids(new Features(null, keyed).getWidgets().listWidgetsIter(), Widget::getId), List.of("w1", "w2", "w3"));
        try {
            new Features(null, options()).getWidgets().listWidgetsIter().iterator().hasNext();
            throw new AssertionError("expected an authentication error");
        } catch (Paginator.PaginationException expected) {
        }

        StreamingStreamEventsOptions topic = new StreamingStreamEventsOptions();
        topic.setTopic("news");
        try (EventStream stream = client.getStreaming().streamEvents(topic)) {
            expect(
                    stream.stream().collect(Collectors.toList()),
                    List.of(
                            new SseEvent("greeting", "news", "1", null),
                            new SseEvent("message", "line1\nline2", "1", null),
                            new SseEvent("message", "{\"n\": 3}", "3", Duration.ofMillis(1500))));
        }
        Streaming.UploadFileBody body = new Streaming.UploadFileBody();
        body.setFile(Upload.of(bytes("hello")).withFilename("a.txt").withContentType("text/plain"));
        body.setName("doc");
        body.setCount(2);
        Health meta = new Health();
        meta.setStatus("ok");
        body.setMeta(meta);
        expect(
                client.getStreaming().uploadFile(body).getStatus(),
                "count=::2;file=a.txt:text/plain:hello;meta=:application/json:{\"status\":\"ok\"};name=::doc");
        expect(client.getStreaming().uploadContent("f1", Upload.of(bytes("raw"))).getStatus(), "application/octet-stream:raw");
        Upload streamed = Upload.of(new ByteArrayInputStream(bytes("streamed")), -1);
        expect(client.getStreaming().uploadContent("f1", streamed).getStatus(), "application/octet-stream:streamed");
        System.out.println("java smoke test passed");
    }
}
