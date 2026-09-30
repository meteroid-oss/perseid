package com.torture;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertFalse;
import static org.junit.jupiter.api.Assertions.assertInstanceOf;
import static org.junit.jupiter.api.Assertions.assertNull;
import static org.junit.jupiter.api.Assertions.assertTrue;

import com.fasterxml.jackson.databind.JsonNode;
import com.fasterxml.jackson.databind.ObjectMapper;
import com.torture.models.Activity;
import com.torture.models.Circle;
import com.torture.models.Composed;
import com.torture.models.Kind;
import com.torture.models.Pet;
import com.torture.models.Priority;
import com.torture.models.Reserved;
import com.torture.models.Shape;
import com.torture.models.Thing;
import com.torture.models.ThingPatch;
import com.torture.models.TreeNode;
import com.torture.models.UnionHolder;
import com.torture.models.WidgetReactions;
import java.math.BigDecimal;
import org.junit.jupiter.api.Test;
import org.junit.jupiter.params.ParameterizedTest;
import org.junit.jupiter.params.provider.CsvSource;

class RoundTripTest {
    private static final ObjectMapper PLAIN = new ObjectMapper();

    private static void assertRoundTrip(Class<?> type, String json) throws Exception {
        Object parsed = Utils.getObjectMapper().readValue(json, type);
        assertEquals(PLAIN.readTree(json), PLAIN.readTree(Utils.toJson(parsed)), type.getSimpleName());
    }

    @ParameterizedTest
    @CsvSource(
            delimiter = '|',
            value = {
                "Thing|{\"id\":\"6f1c\",\"name\":\"n\",\"created_at\":\"2024-01-02T03:04:05.123456789+02:00\",\"kind\":\"beta-2\",\"count\":9007199254740993,\"nullable_required\":null,\"tags\":[\"a\"],\"metadata\":{\"k\":\"v\"},\"attrs\":{\"x\":[1,2]},\"amount\":\"12.50\",\"birthday\":\"2024-02-29\",\"nullable_ref\":null,\"nullable_optional\":null,\"priority\":-1,\"nested_map\":{\"a\":[{\"line1\":\"l\"}]},\"anyof_nullable_ref\":{\"line1\":\"x\"},\"mode\":\"only\"}",
                "Shape|{\"type\":\"circle\",\"radius\":1.5}",
                "Shape|{\"type\":\"square\",\"side\":2.5}",
                "Shape|{\"type\":\"triangle\",\"a\":1,\"nested\":{\"b\":[true]}}",
                "Pet|{\"pet_type\":\"Cat\",\"meow\":true}",
                "Activity|{\"kind\":\"opened\",\"at\":\"2024-01-02T03:04:05Z\"}",
                "Activity|{\"kind\":\"reopened\"}",
                "Activity|{\"kind\":\"closed\",\"by\":\"me\"}",
                "Composed|{\"id\":\"b1\",\"created_at\":\"2024-01-02T03:04:05Z\",\"extra\":\"e\",\"sibling_prop\":\"s\"}",
                "TreeNode|{\"value\":\"root\",\"children\":[{\"value\":\"c\",\"children\":[]}],\"parent\":null,\"next\":{\"value\":\"n\",\"children\":[]}}",
                "UnionHolder|{\"shape\":{\"type\":\"circle\",\"radius\":1.5},\"shapes\":[{\"type\":\"square\",\"side\":1.5}],\"maybe_shape\":null,\"shape_map\":{\"k\":{\"type\":\"square\",\"side\":3.5}},\"inline_union\":[\"a\"],\"empty\":{},\"free_form\":{\"any\":1},\"counts\":{\"a\":9007199254740993},\"nested\":{\"id\":\"n\",\"depth\":2}}",
                "ThingPatch|{\"description\":null}",
                "Reserved|{\"type\":\"t\",\"class\":\"c\",\"default\":\"d\",\"import\":\"i\",\"null\":\"n\",\"kebab-case\":\"k\",\"with space\":\"w\",\"$dollar\":\"d\",\"1leading\":\"l\"}",
                "WidgetReactions|{\"+1\":3,\"-1\":1}",
            })
    void payloadsSurviveARoundTrip(String model, String json) throws Exception {
        assertRoundTrip(Class.forName("com.torture.models." + model), json);
    }

    @Test
    void decimalsAreStringsAndOffsetsAreKept() throws Exception {
        Thing thing = new Thing().amount(new BigDecimal("1E+3"));
        assertEquals("{\"amount\":\"1000\",\"nullable_required\":null}", thing.toJson());
        Thing parsed = Thing.fromJson("{\"created_at\":\"2024-01-02T03:04:05+02:00\"}");
        assertEquals("+02:00", parsed.getCreatedAt().getOffset().toString());
    }

    @Test
    void unknownEnumValuesParseAsUnrecognized() throws Exception {
        Thing thing = Thing.fromJson("{\"kind\":\"brand-new\",\"priority\":99}");
        assertEquals(Kind.UNRECOGNIZED, thing.getKind());
        assertEquals(Priority.UNRECOGNIZED, thing.getPriority());
        assertEquals(Kind.BETA_2, Kind.fromValue("beta-2"));
        assertEquals(Priority.NEGATIVE, Priority.fromValue(-1));
    }

    @Test
    void unknownVariantsKeepTheirProperties() throws Exception {
        Shape shape = Shape.fromJson("{\"type\":\"triangle\",\"a\":1}");
        Shape.Unrecognized unknown = assertInstanceOf(Shape.Unrecognized.class, shape);
        assertEquals("triangle", unknown.getType());
        assertEquals(1, unknown.getProperties().get("a"));
    }

    @Test
    void variantsWrapTheirModelAndWriteTheDiscriminatorOnce() throws Exception {
        Shape shape = new Shape.Circle(new Circle().radius(2.5));
        assertEquals("{\"radius\":2.5,\"type\":\"circle\"}", shape.toJson());
        Shape.Circle parsed = assertInstanceOf(Shape.Circle.class, Shape.fromJson(shape.toJson()));
        assertEquals(2.5, parsed.getData().getRadius());
        assertEquals("circle", parsed.getData().getType());
        assertEquals(shape.hashCode(), new Shape.Circle(new Circle().radius(2.5)).hashCode());

        Activity reopened = Activity.fromJson("{\"kind\":\"reopened\"}");
        assertEquals("reopened", assertInstanceOf(Activity.Reopened.class, reopened).getKind());
    }

    @Test
    void nullableFieldsDistinguishUnsetFromNull() throws Exception {
        assertEquals("{}", new ThingPatch().toJson());
        assertEquals("{\"description\":null}", new ThingPatch().description(null).toJson());
        assertEquals("{\"count\":3}", new ThingPatch().count(3L).toJson());
        ThingPatch cleared = ThingPatch.fromJson("{\"description\":null}");
        assertNull(cleared.getDescription());
        assertFalse(cleared.equals(new ThingPatch()));
    }

    @Test
    void reservedWordsBecomeUsableAccessors() {
        Reserved reserved = new Reserved().class_("c").default_("d").import_("i").null_("n");
        assertEquals("c", reserved.getClass_());
        assertEquals("d", reserved.getDefault());
        assertEquals("n", reserved.getNull());
        assertTrue(reserved.toString().startsWith("Reserved{"));
    }

    @Test
    void allOfPartsAreFlattened() throws Exception {
        Composed composed = Composed.fromJson("{\"id\":\"b1\",\"extra\":\"e\",\"sibling_prop\":\"s\"}");
        assertEquals("b1", composed.getBase().getId());
        JsonNode json = PLAIN.readTree(composed.toJson());
        assertFalse(json.has("base"));
        assertEquals("b1", json.get("id").asText());
    }
}
