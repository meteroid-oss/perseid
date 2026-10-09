package com.tags;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertThrows;
import static org.junit.jupiter.api.Assertions.assertTrue;

import com.fasterxml.jackson.databind.ObjectMapper;
import com.tags.exceptions.InvalidDataException;
import com.tags.models.AssistantTurn;
import com.tags.models.Conversation;
import com.tags.models.SimpleTurn;
import com.tags.models.Turn;
import com.tags.models.UserTurn;
import java.util.List;
import org.junit.jupiter.api.Test;

/**
 * Union variants sharing a tag, on the SDK generated from tests/sdk/rust/tags/openapi.yaml: three
 * {@code message} variants, two of them from a nested union, and no mapping.
 */
class TagsTest {
    private static final ObjectMapper PLAIN = new ObjectMapper();

    @Test
    void variantsSharingATagAreSentWithIt() throws Exception {
        List<Turn> turns =
                List.of(
                        Turn.Message.of(SimpleTurn.builder().content("hi").build()),
                        Turn.UserTurn.of(UserTurn.builder().role("user").parts(List.of("a")).build()),
                        Turn.AssistantTurn.of(AssistantTurn.builder().id("m1").parts(List.of()).build()));
        for (Turn turn : turns) {
            assertEquals("message", PLAIN.readTree(turn.toJson()).get("type").asText());
            assertEquals("message", turn.type());
        }
    }

    @Test
    void aSharedTagDecodesAsTheVariantTheDataFits() {
        assertTrue(Turn.fromJson("{\"type\":\"message\",\"content\":\"hi\"}").isMessage());
        Turn user = Turn.fromJson("{\"type\":\"message\",\"role\":\"user\",\"parts\":[\"a\"]}");
        assertEquals(List.of("a"), user.asUserTurn().data().parts());
        Turn assistant = Turn.fromJson("{\"type\":\"message\",\"id\":\"m1\",\"parts\":[]}");
        assertEquals("m1", assistant.asAssistantTurn().data().id());
        assertTrue(Turn.fromJson("{\"type\":\"tool\",\"output\":\"42\"}").isTool());
    }

    @Test
    void dataNoVariantFitsDecodesAsTheOneTheTagNames() {
        Turn turn = Turn.fromJson("{\"type\":\"message\"}");
        InvalidDataException error =
                assertThrows(InvalidDataException.class, () -> turn.asMessage().data().content());
        assertTrue(error.getMessage().contains("content"), error.getMessage());
    }

    @Test
    void theNameOfAVariantIsNotItsTag() {
        Turn turn = Turn.fromJson("{\"type\":\"UserTurn\",\"role\":\"user\",\"parts\":[]}");
        assertTrue(turn.isUnrecognized());
        assertEquals("UserTurn", turn.type());
    }

    @Test
    void sharedTagsRoundTrip() throws Exception {
        String json =
                "{\"turns\":["
                        + "{\"type\":\"message\",\"content\":\"hi\"},"
                        + "{\"type\":\"message\",\"role\":\"user\",\"parts\":[\"a\"]},"
                        + "{\"type\":\"message\",\"id\":\"m1\",\"parts\":[]},"
                        + "{\"type\":\"tool\",\"output\":\"42\"}]}";
        Conversation conversation = Conversation.fromJson(json);
        assertEquals(PLAIN.readTree(json), PLAIN.readTree(conversation.toJson()));
        assertEquals(conversation, Conversation.fromJson(conversation.toJson()));
    }
}
