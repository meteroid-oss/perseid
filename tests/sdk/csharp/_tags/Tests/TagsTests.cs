using System.Text.Json;
using System.Text.Json.Nodes;
using Tags.Models;

/// <summary>Union variants sharing a tag, on the SDK generated from tests/sdk/rust/tags/openapi.yaml.</summary>
public class TagsTests
{
    private static readonly TagsJsonContext Context = TagsJsonContext.Default;

    private static Turn Decode(string json) => JsonSerializer.Deserialize(json, Context.Turn)!;

    [Fact]
    public void VariantsSharingATagAreSentWithIt()
    {
        Turn[] turns =
        [
            new SimpleTurn { Content = "hi" },
            new UserTurn { Parts = ["a"] },
            new AssistantTurn { Id = "m1", Parts = [] },
        ];
        foreach (var turn in turns)
        {
            var json = JsonNode.Parse(JsonSerializer.Serialize(turn, Context.Turn))!;
            Assert.Equal("message", (string?)json["type"]);
            Assert.Equal("message", turn.Type);
        }
    }

    [Fact]
    public void ASharedTagDecodesAsTheVariantKnowingTheData()
    {
        Assert.IsType<Turn.Message>(Decode("""{"type": "message", "content": "hi"}"""));
        var user = Assert.IsType<Turn.UserTurn>(Decode("""{"type": "message", "role": "user", "parts": ["a"]}"""));
        Assert.Equal(["a"], user.Value.Parts);
        var assistant = Assert.IsType<Turn.AssistantTurn>(Decode("""{"type": "message", "id": "m1", "parts": []}"""));
        Assert.Equal("m1", assistant.Value.Id);
        var error = Assert.Throws<JsonException>(() => Decode("""{"type": "message"}"""));
        Assert.Contains("content", error.Message, StringComparison.Ordinal);
    }

    [Fact]
    public void SharedTagsRoundTrip()
    {
        const string json = """
            {"turns": [
                {"type": "message", "content": "hi"},
                {"type": "message", "role": "user", "parts": ["a"]},
                {"type": "message", "id": "m1", "parts": []},
                {"type": "tool", "output": "42"}
            ]}
            """;
        var conversation = JsonSerializer.Deserialize(json, Context.Conversation)!;
        Assert.True(
            JsonNode.DeepEquals(JsonNode.Parse(json), JsonNode.Parse(JsonSerializer.Serialize(conversation, Context.Conversation)))
        );
    }

    [Fact]
    public void AVariantModelConvertsToItsUnion()
    {
        Turn turn = new ToolTurn { Output = "42" };
        Assert.Equal(new ToolTurn { Output = "42" }, Assert.IsType<Turn.Tool>(turn).Value);
    }
}
