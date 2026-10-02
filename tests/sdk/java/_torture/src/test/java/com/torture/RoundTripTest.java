package com.torture;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertFalse;
import static org.junit.jupiter.api.Assertions.assertInstanceOf;
import static org.junit.jupiter.api.Assertions.assertNotEquals;
import static org.junit.jupiter.api.Assertions.assertThrows;
import static org.junit.jupiter.api.Assertions.assertTrue;

import com.fasterxml.jackson.databind.JsonNode;
import com.fasterxml.jackson.databind.ObjectMapper;
import com.fasterxml.jackson.databind.node.IntNode;
import com.torture.exceptions.InvalidDataException;
import com.torture.internal.Utils;
import com.torture.models.Account;
import com.torture.models.Activity;
import com.torture.models.Circle;
import com.torture.models.Composed;
import com.torture.models.Kind;
import com.torture.models.ObjectUnions;
import com.torture.models.ObjectUnions.ObjectUnionsAccount;
import com.torture.models.ObjectUnions.ObjectUnionsDocument;
import com.torture.models.Priority;
import com.torture.models.Reserved;
import com.torture.models.Shape;
import com.torture.models.Thing;
import com.torture.models.ThingCreate;
import com.torture.models.ThingList;
import com.torture.models.ThingPatch;
import com.torture.models.UnionHolder;
import com.torture.models.UnionHolder.UnionHolderStrOrInt;
import java.math.BigDecimal;
import java.util.ArrayList;
import java.util.List;
import org.junit.jupiter.api.Test;
import org.junit.jupiter.params.ParameterizedTest;
import org.junit.jupiter.params.provider.CsvSource;

class RoundTripTest {
    private static final ObjectMapper PLAIN = new ObjectMapper();
    private static final String THING = "{\"id\":\"i\",\"name\":\"n\",\"created_at\":\"2024-01-02T03:04:05Z\",\"kind\":\"alpha\","
            + "\"count\":1,\"nullable_required\":null,\"tags\":[],\"metadata\":{},\"attrs\":{}";

    private static void assertRoundTrip(Class<?> type, String json) throws Exception {
        Object parsed = Utils.getObjectMapper().readValue(json, type);
        assertEquals(PLAIN.readTree(json), PLAIN.readTree(Utils.json(parsed)), type.getSimpleName());
    }

    @ParameterizedTest
    @CsvSource(
            delimiter = '|',
            value = {
                "Thing|{\"id\":\"6f1c\",\"name\":\"n\",\"created_at\":\"2024-01-02T03:04:05.123456789+02:00\",\"kind\":\"beta-2\",\"count\":9007199254740993,\"nullable_required\":null,\"tags\":[\"a\"],\"metadata\":{\"k\":\"v\"},\"attrs\":{\"x\":[1,2]},\"amount\":\"12.50\",\"birthday\":\"2024-02-29\",\"nullable_ref\":null,\"nullable_optional\":null,\"priority\":-1,\"nested_map\":{\"a\":[{\"line1\":\"l\"}]},\"anyof_nullable_ref\":{\"line1\":\"x\"},\"mode\":\"only\"}",
                "Thing|{\"id\":\"t\",\"nullable_required\":null,\"brand_new\":{\"deep\":[1,\"x\",null]}}",
                "Shape|{\"type\":\"circle\",\"radius\":1.5}",
                "Shape|{\"type\":\"circle\",\"radius\":1.5,\"color\":\"red\"}",
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
    void unknownPropertiesAreKeptAndSentBack() {
        Thing thing = Thing.fromJson(THING + ",\"brand_new\":7}");
        assertEquals(IntNode.valueOf(7), thing.additionalProperties().get("brand_new"));
        Thing changed = thing.toBuilder().name("n").build();
        assertEquals(IntNode.valueOf(7), changed.additionalProperties().get("brand_new"));
        assertTrue(changed.toJson().contains("\"brand_new\":7"));
        assertThrows(UnsupportedOperationException.class, () -> thing.additionalProperties().clear());
    }

    @Test
    void buildersCheckRequiredPropertiesAndModelsAreImmutable() {
        IllegalStateException missing =
                assertThrows(IllegalStateException.class, () -> ThingCreate.builder().name("n").build());
        assertEquals("`kind` is required, but was not set", missing.getMessage());
        java.util.Map<String, String> metadata = new java.util.HashMap<>(java.util.Map.of("a", "1"));
        ThingCreate created = ThingCreate.builder().name("n").kind(Kind.ALPHA).metadata(metadata).build();
        metadata.put("b", "2");
        assertEquals(java.util.Map.of("a", "1"), created.metadata().orElseThrow());
        assertThrows(UnsupportedOperationException.class, () -> created.metadata().orElseThrow().put("c", "3"));
        ThingCreate more = created.toBuilder().putMetadataItem("c", "3").build();
        assertEquals(java.util.Map.of("a", "1", "c", "3"), more.metadata().orElseThrow());
        assertEquals(java.util.Map.of("a", "1"), created.metadata().orElseThrow());
        assertNotEquals(created, more);
        assertEquals(created, created.toBuilder().build());
        assertEquals(created.hashCode(), created.toBuilder().build().hashCode());
    }

    @Test
    void decimalsAreStringsAndOffsetsAreKept() {
        Thing thing = Thing.fromJson(THING + "}")
                .toBuilder()
                .amount(new BigDecimal("1E+3"))
                .build();
        assertTrue(thing.toJson().contains("\"amount\":\"1000\""));
        Thing parsed = Thing.fromJson("{\"created_at\":\"2024-01-02T03:04:05+02:00\"}");
        assertEquals("+02:00", parsed.createdAt().getOffset().toString());
    }

    @Test
    void unknownEnumValuesAreKeptAndSentBack() throws Exception {
        Thing thing = Thing.fromJson("{\"kind\":\"brand-new\",\"priority\":99}");
        assertEquals("brand-new", thing.kind().getValue());
        assertFalse(thing.kind().isKnown());
        assertEquals(Kind.Known.UNRECOGNIZED, thing.kind().known());
        assertEquals(99L, thing.priority().orElseThrow().getValue());
        assertEquals(
                PLAIN.readTree("{\"kind\":\"brand-new\",\"priority\":99,\"nullable_required\":null}"),
                PLAIN.readTree(thing.toJson()));
        assertEquals(Kind.of("brand-new"), thing.kind());
    }

    @Test
    void knownEnumValuesAreTheConstants() {
        assertTrue(Kind.of("beta-2") == Kind.BETA_2);
        assertTrue(Kind.BETA_2.isKnown());
        assertEquals("BETA_2", Kind.BETA_2.name());
        assertTrue(Kind.valueOf("BETA_2") == Kind.BETA_2);
        assertEquals(Priority.NEGATIVE, Priority.of(-1));
        String label;
        switch (Kind.of("alpha").known()) {
            case ALPHA:
                label = "a";
                break;
            default:
                label = "other";
        }
        assertEquals("a", label);
    }

    @Test
    void pagesKeepUnknownEnumValues() {
        ThingList first = ThingList.fromJson("{\"data\":[{\"kind\":\"brand-new\"}],\"next_cursor\":\"c\"}");
        ThingList last = ThingList.fromJson("{\"data\":[{\"kind\":\"alpha\"}]}");
        Paginator<Thing> things = new Paginator<>(
                () -> Page.of(first.data(), () -> Page.of(last.data(), null)));
        List<Kind> kinds = new ArrayList<>();
        things.forEach(thing -> kinds.add(thing.kind()));
        assertEquals(List.of(Kind.of("brand-new"), Kind.ALPHA), kinds);
        assertEquals(2, things.stream().count());
    }

    @Test
    void primitiveOrObjectUnionsAreTypedByJsonType() {
        UnionHolder text = UnionHolder.fromJson("{\"str_or_int\":\"a\"}");
        assertEquals("a", text.strOrInt().orElseThrow().asString());
        UnionHolder number = UnionHolder.fromJson("{\"str_or_int\":7}");
        assertTrue(number.strOrInt().orElseThrow().isInteger());
        assertEquals(7L, number.strOrInt().orElseThrow().asInteger());
        assertThrows(IllegalStateException.class, () -> number.strOrInt().orElseThrow().asString());
        Shape shape = Shape.Circle.of(Circle.builder().radius(1.0).build());
        String json = UnionHolder.builder()
                .shape(shape)
                .shapes(List.of())
                .strOrInt(UnionHolderStrOrInt.ofInteger(7L))
                .build()
                .toJson();
        assertTrue(json.contains("\"str_or_int\":7"), json);
        assertEquals(UnionHolderStrOrInt.ofString("a"), text.strOrInt().orElseThrow());
        assertThrows(InvalidDataException.class, () -> UnionHolder.fromJson("{\"str_or_int\":true}"));
    }

    @Test
    void variantModelsCarryTheirDiscriminatorValue() {
        assertEquals("circle", Circle.builder().radius(1.0).build().type());
        assertEquals("{\"radius\":1.0,\"type\":\"circle\"}", Circle.builder().radius(1.0).build().toJson());
    }

    @Test
    void unknownVariantsKeepTheirProperties() {
        Shape shape = Shape.fromJson("{\"type\":\"triangle\",\"a\":1}");
        Shape.Unrecognized unknown = assertInstanceOf(Shape.Unrecognized.class, shape);
        assertEquals("triangle", unknown.type());
        assertEquals(1, unknown.properties().get("a").asInt());
    }

    @Test
    void variantsWrapTheirModelAndWriteTheDiscriminatorOnce() {
        Shape shape = new Shape.Circle(Circle.builder().radius(2.5).build());
        assertEquals("{\"radius\":2.5,\"type\":\"circle\"}", shape.toJson());
        Shape.Circle parsed = assertInstanceOf(Shape.Circle.class, Shape.fromJson(shape.toJson()));
        assertEquals(2.5, parsed.data().radius());
        assertEquals("circle", parsed.data().type());
        assertEquals(shape.hashCode(), Shape.Circle.of(Circle.builder().radius(2.5).build()).hashCode());

        Activity reopened = Activity.fromJson("{\"kind\":\"reopened\"}");
        assertEquals("reopened", assertInstanceOf(Activity.Reopened.class, reopened).kind());
    }

    @Test
    void nullableFieldsDistinguishUnsetFromNull() {
        assertEquals("{}", ThingPatch.builder().build().toJson());
        assertEquals("{\"description\":null}", ThingPatch.builder().description(null).build().toJson());
        assertEquals("{\"count\":3}", ThingPatch.builder().count(3L).build().toJson());
        ThingPatch cleared = ThingPatch.fromJson("{\"description\":null}");
        assertTrue(cleared.description().isEmpty());
        assertFalse(cleared.equals(ThingPatch.builder().build()));
        assertEquals("{\"description\":null}", cleared.toBuilder().build().toJson());
    }

    @Test
    void reservedWordsBecomeUsableAccessors() {
        Reserved reserved = Reserved.builder().class_("c").type("t").default_("d").import_("i").null_("n").build();
        assertEquals("c", reserved.class_());
        assertEquals("d", reserved.default_().orElseThrow());
        assertEquals("n", reserved.null_().orElseThrow());
        assertTrue(reserved.toString().startsWith("Reserved{"));
    }

    @Test
    void allOfPartsAreFlattened() throws Exception {
        Composed composed = Composed.fromJson("{\"id\":\"b1\",\"extra\":\"e\",\"sibling_prop\":\"s\"}");
        assertEquals("b1", composed.base().id());
        JsonNode json = PLAIN.readTree(composed.toJson());
        assertFalse(json.has("base"));
        assertEquals("b1", json.get("id").asText());

        String json2 = "{\"id\":\"b1\",\"extra\":\"e\",\"sibling_prop\":\"s\",\"color\":\"red\"}";
        Composed colored = Composed.fromJson(json2);
        assertEquals(java.util.Set.of("color"), colored.additionalProperties().keySet());
        assertEquals(PLAIN.readTree(json2), PLAIN.readTree(colored.toJson()));
        Composed built = Composed.builder().base(colored.base()).extra("e").siblingProp("s").build();
        assertEquals(colored, built);
    }

    @ParameterizedTest
    @CsvSource(
            delimiter = '|',
            value = {
                "\"a0\"|string|a0",
                "{\"id\":\"a1\",\"object\":\"account\",\"email\":\"e\"}|account|a1",
                "{\"deleted\":true,\"id\":\"a2\",\"object\":\"account\"}|deleted|a2",
                "{\"object\":\"account_v2\",\"id\":\"a3\"}|unrecognized|",
            })
    void unionsOfObjectsPickTheirVariant(String json, String variant, String id) throws Exception {
        ObjectUnionsAccount account = Utils.getObjectMapper().readValue(json, ObjectUnionsAccount.class);
        assertEquals(variant.equals("string"), account.isString());
        assertEquals(variant.equals("account"), account.isAccount());
        assertEquals(variant.equals("deleted"), account.isDeletedAccount());
        assertEquals(variant.equals("unrecognized"), account.isUnrecognized());
        assertEquals(id, account.id());
        assertEquals(PLAIN.readTree(json), PLAIN.readTree(Utils.json(account)));
    }

    @Test
    void unionsOfObjectsDecodeAsAnotherVariantAndKeepUnknownShapes() throws Exception {
        ObjectUnionsAccount deleted = Utils.getObjectMapper()
                .readValue("{\"deleted\":true,\"id\":\"a2\",\"object\":\"account\"}", ObjectUnionsAccount.class);
        assertEquals("a2", deleted.decodeAs(Account.class).id());

        String json = "{\"source\":{\"file_id\":\"f\"},\"sources\":[{\"url\":\"u\",\"detail\":\"d\"},{\"path\":\"p\"}],"
                + "\"document\":{\"title\":\"t\",\"author\":\"a\"},\"loose\":{\"title\":\"t\"}}";
        ObjectUnions unions = ObjectUnions.fromJson(json);
        assertTrue(unions.source().orElseThrow().isFileSource());
        assertTrue(unions.sources().orElseThrow().get(0).isUrlSource());
        assertTrue(unions.sources().orElseThrow().get(1).isUnrecognized());
        assertTrue(unions.document().orElseThrow().isArticle());
        assertInstanceOf(java.util.Map.class, unions.loose().orElseThrow());
        assertEquals(PLAIN.readTree(json), PLAIN.readTree(unions.toJson()));
        ObjectUnions draft = ObjectUnions.fromJson("{\"document\":{\"title\":\"t\"}}");
        assertTrue(draft.document().orElseThrow().isDraft(), "ties go to the first variant");
        ObjectUnionsDocument untitled = ObjectUnions.fromJson("{\"document\":{\"body\":\"b\"}}").document().orElseThrow();
        assertTrue(untitled.isUnrecognized());
    }
}
