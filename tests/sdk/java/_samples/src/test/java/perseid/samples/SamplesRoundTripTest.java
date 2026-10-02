package perseid.samples;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertFalse;
import static org.junit.jupiter.api.Assertions.assertNotNull;
import static org.junit.jupiter.api.Assertions.assertTrue;
import static org.junit.jupiter.api.DynamicTest.dynamicTest;

import com.fasterxml.jackson.databind.DeserializationFeature;
import com.fasterxml.jackson.databind.JsonNode;
import com.fasterxml.jackson.databind.ObjectMapper;
import java.io.IOException;
import java.math.BigDecimal;
import java.nio.file.Files;
import java.nio.file.Path;
import java.nio.file.Paths;
import java.time.OffsetDateTime;
import java.time.format.DateTimeParseException;
import java.util.ArrayList;
import java.util.Iterator;
import java.util.List;
import java.util.Map;
import java.util.Set;
import java.util.TreeMap;
import java.util.TreeSet;
import java.util.concurrent.atomic.AtomicInteger;
import java.util.regex.Pattern;
import java.util.stream.Collectors;
import java.util.stream.Stream;
import org.junit.jupiter.api.DynamicTest;
import org.junit.jupiter.api.Test;
import org.junit.jupiter.api.TestFactory;

/**
 * Decodes and re-encodes the `perseid samples` of every model of the SDK in the working directory.
 *
 * <p>tests/sdk/run.sh generates the SDK of the torture fixture and of every edge fixture, writes
 * samples.json next to build.gradle (see src/samples.rs) and runs this test. Every sample of every
 * model is decoded into the generated class with the SDK's own mapper and encoded again; the JSON
 * must come back equal, up to the documented normalizations of {@link #differences}: date-times
 * compare as instants and decimal strings as numbers. Integers (int64 included) compare exactly.
 *
 * <p>The classes are found by reflection, from the files of the models package, so that the test
 * knows no model and no package name. Java has no type aliases: a schema that is an alias (of a
 * string, a list, a map or a union of primitives) has no class, and the SDK inlines it.
 */
class SamplesRoundTripTest {
    private static final Pattern DECIMAL = Pattern.compile("-?\\d+(?:\\.\\d+)?");
    private static final AtomicInteger DECODED = new AtomicInteger();

    /** Reads JSON with exact numbers: no float rounds an integer or a decimal. */
    private static ObjectMapper exact() {
        return new ObjectMapper().enable(DeserializationFeature.USE_BIG_DECIMAL_FOR_FLOATS);
    }

    private static boolean sameInstant(String want, String got) {
        try {
            return OffsetDateTime.parse(want).isEqual(OffsetDateTime.parse(got));
        } catch (DateTimeParseException e) {
            return false;
        }
    }

    private static boolean sameDecimal(String want, String got) {
        return DECIMAL.matcher(want).matches()
                && DECIMAL.matcher(got).matches()
                && new BigDecimal(want).compareTo(new BigDecimal(got)) == 0;
    }

    /** Where {@code got} differs from {@code want}, as JSON, beyond the documented normalizations. */
    static List<String> differences(JsonNode want, JsonNode got, String path) {
        List<String> out = new ArrayList<>();
        String mismatch = path + ": expected " + want + ", got " + got;
        if (want.isNumber() && got.isNumber()) {
            boolean same = want.isIntegralNumber() && got.isIntegralNumber()
                    ? want.bigIntegerValue().equals(got.bigIntegerValue())
                    : want.decimalValue().compareTo(got.decimalValue()) == 0;
            if (!same) {
                out.add(mismatch);
            }
        } else if (want.isTextual() && got.isTextual()) {
            String a = want.textValue();
            String b = got.textValue();
            if (!a.equals(b) && !sameInstant(a, b) && !sameDecimal(a, b)) {
                out.add(mismatch);
            }
        } else if (want.isObject() && got.isObject()) {
            Set<String> names = new TreeSet<>();
            for (Iterator<String> it = want.fieldNames(); it.hasNext(); ) {
                names.add(it.next());
            }
            for (Iterator<String> it = got.fieldNames(); it.hasNext(); ) {
                names.add(it.next());
            }
            for (String name : names) {
                if (!got.has(name)) {
                    out.add(path + "." + name + ": missing, expected " + want.get(name));
                } else if (!want.has(name)) {
                    out.add(path + "." + name + ": unexpected " + got.get(name));
                } else {
                    out.addAll(differences(want.get(name), got.get(name), path + "." + name));
                }
            }
        } else if (want.isArray() && got.isArray()) {
            if (want.size() != got.size()) {
                out.add(path + ": expected " + want.size() + " items, got " + got.size());
            } else {
                for (int i = 0; i < want.size(); i++) {
                    out.addAll(differences(want.get(i), got.get(i), path + "[" + i + "]"));
                }
            }
        } else if (!want.equals(got)) {
            out.add(mismatch);
        }
        return out;
    }

    @Test
    void onlyDocumentedNormalizationsAreAccepted() throws IOException {
        ObjectMapper json = exact();
        assertTrue(differences(json.readTree("\"2024-01-02T03:04:05Z\""), json.readTree("\"2024-01-02T04:04:05.000+01:00\""), "$").isEmpty());
        assertTrue(differences(json.readTree("\"12.50\""), json.readTree("\"12.5\""), "$").isEmpty());
        assertTrue(differences(json.readTree("1"), json.readTree("1.0"), "$").isEmpty());
        assertFalse(differences(json.readTree("9007199254740993"), json.readTree("9007199254740992.0"), "$").isEmpty());
        assertFalse(differences(json.readTree("9007199254740993"), json.readTree("9007199254740992"), "$").isEmpty());
        assertFalse(differences(json.readTree("true"), json.readTree("1"), "$").isEmpty());
        assertFalse(differences(json.readTree("null"), json.readTree("0"), "$").isEmpty());
        assertFalse(differences(json.readTree("\"a\""), json.readTree("\"b\""), "$").isEmpty());
        assertFalse(differences(json.readTree("{\"a\":null}"), json.readTree("{}"), "$").isEmpty());
        assertFalse(differences(json.readTree("{}"), json.readTree("{\"a\":null}"), "$").isEmpty());
        assertFalse(differences(json.readTree("[1,2]"), json.readTree("[1]"), "$").isEmpty());
    }

    /** The qualified class of every model, by simple name: the files of the `models` packages. */
    private static Map<String, String> modelClasses() throws IOException {
        Path sources = Paths.get("src", "main", "java");
        Map<String, String> classes = new TreeMap<>();
        try (Stream<Path> files = Files.walk(sources)) {
            for (Path file : files.filter(f -> f.toString().endsWith(".java")).collect(Collectors.toList())) {
                Path parent = file.getParent();
                if (parent.getFileName().toString().equals("models")) {
                    String pkg = sources.relativize(parent).toString().replace(java.io.File.separatorChar, '.');
                    String name = file.getFileName().toString().replaceAll("\\.java$", "");
                    classes.put(name, pkg + "." + name);
                }
            }
        }
        return classes;
    }

    /** The SDK's own mapper, which lives in the `internal` package next to `models`. */
    private static ObjectMapper sdkMapper(String qualifiedModel) throws ReflectiveOperationException {
        String pkg = qualifiedModel.substring(0, qualifiedModel.lastIndexOf(".models."));
        Class<?> utils = Class.forName(pkg + ".internal.Utils");
        return (ObjectMapper) utils.getMethod("getObjectMapper").invoke(null);
    }

    private static void roundTrip(String qualified, JsonNode sample) throws Exception {
        Class<?> type = Class.forName(qualified);
        ObjectMapper sdk = sdkMapper(qualified);
        String text = sample.toString();
        Object parsed;
        try {
            parsed = sdk.readValue(text, type);
        } catch (Exception e) {
            throw new AssertionError("cannot decode " + text + " as " + type.getSimpleName() + ": " + e, e);
        }
        assertNotNull(parsed, text);
        String encoded = sdk.writeValueAsString(parsed);
        List<String> found = differences(sample, exact().readTree(encoded), "$");
        assertTrue(found.isEmpty(), type.getSimpleName() + " " + text + " was encoded as " + encoded + ": " + found);
        // Decoding what was encoded gives the same value again.
        assertEquals(parsed, sdk.readValue(encoded, type), encoded);
        DECODED.incrementAndGet();
    }

    @TestFactory
    Stream<DynamicTest> everySampleOfEveryModelRoundTrips() throws IOException {
        Path file = Paths.get("samples.json");
        assertTrue(Files.exists(file), "tests/sdk/run.sh writes samples.json next to build.gradle");
        JsonNode models = exact().readTree(Files.readAllBytes(file));
        Map<String, String> classes = modelClasses();
        List<DynamicTest> tests = new ArrayList<>();
        Map<String, JsonNode> sorted = new TreeMap<>();
        models.fields().forEachRemaining(entry -> sorted.put(entry.getKey(), entry.getValue()));
        for (Map.Entry<String, JsonNode> model : sorted.entrySet()) {
            String schema = model.getKey();
            String kind = model.getValue().get("kind").asText();
            String name = model.getValue().get("names").get("java").asText();
            JsonNode samples = model.getValue().get("samples");
            String qualified = classes.get(name);
            if (qualified == null) {
                tests.add(dynamicTest(schema + " has no class (" + kind + ")", () -> assertTrue(
                        kind.equals("alias") || kind.equals("union"),
                        "the " + kind + " schema " + schema + " must have a class named " + name + ", has: " + classes.keySet())));
                continue;
            }
            assertTrue(samples.size() > 0, schema + " has no samples");
            for (JsonNode sample : samples) {
                String id = sample.get("name").asText();
                tests.add(dynamicTest(schema + " (" + name + ") " + id, () -> roundTrip(qualified, sample.get("json"))));
            }
        }
        tests.add(dynamicTest("the models were decoded", () -> assertTrue(
                DECODED.get() > 0 || classes.isEmpty(), "no sample was decoded")));
        return tests.stream();
    }
}
