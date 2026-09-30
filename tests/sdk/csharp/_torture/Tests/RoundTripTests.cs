using System.Text.Json;
using System.Text.Json.Nodes;
using System.Text.Json.Serialization.Metadata;
using Torture;
using Torture.Models;

public class RoundTripTests
{
    private static readonly TortureJsonContext Context = TortureJsonContext.Default;

    private const string Thing = """
        {"id":"t1","name":"n","created_at":"2024-01-02T03:04:05.1234567+02:00","kind":"beta-2",
        "count":9007199254740993,"unsigned":18446744073709551615,"nullable_required":null,
        "tags":["a"],"metadata":{"k":"v"},"attrs":{"x":[1,2]},"amount":"12.50","birthday":"2024-02-29",
        "priority":-1,"nested_map":{"a":[{"line1":"l"}]},"anyof_nullable_ref":{"line1":"x"},"mode":"only"}
        """;

    private static void AssertRoundTrip<T>(JsonTypeInfo<T> typeInfo, string json, string? expected = null)
    {
        var parsed = JsonSerializer.Deserialize(json, typeInfo)!;
        var written = JsonSerializer.Serialize(parsed, typeInfo);
        Assert.True(
            JsonNode.DeepEquals(JsonNode.Parse(expected ?? json), JsonNode.Parse(written)),
            $"{typeof(T).Name}: {written}"
        );
    }

    [Fact]
    public void PayloadsSurviveARoundTrip()
    {
        AssertRoundTrip(Context.Thing, Thing);
        AssertRoundTrip(Context.Shape, """{"type":"circle","radius":1.5}""");
        AssertRoundTrip(Context.Shape, """{"type":"triangle","a":1,"nested":{"b":[true]}}""");
        AssertRoundTrip(Context.Pet, """{"pet_type":"Cat","meow":true}""");
        AssertRoundTrip(Context.Activity, """{"kind":"opened","at":"2024-01-02T03:04:05+00:00"}""");
        AssertRoundTrip(Context.Activity, """{"kind":"reopened"}""");
        AssertRoundTrip(Context.Activity, """{"kind":"closed","by":"me"}""");
        AssertRoundTrip(Context.InlineEvent, """{"event":"deleted","reason":null}""");
        AssertRoundTrip(
            Context.Composed,
            """{"id":"b1","created_at":"2024-01-02T03:04:05+00:00","extra":"e","sibling_prop":"s"}"""
        );
        AssertRoundTrip(
            Context.TreeNode,
            """{"value":"root","children":[{"value":"c","children":[]}],"next":{"value":"n","children":[]}}"""
        );
        AssertRoundTrip(
            Context.UnionHolder,
            """
            {"shape":{"type":"circle","radius":1.5},"shapes":[{"type":"square","side":1.5}],
            "shape_map":{"k":{"type":"square","side":3.5}},"inline_union":["a",1],"empty":{},
            "free_form":{"any":1},"counts":{"a":9007199254740993},"nested":{"id":"n","depth":2}}
            """
        );
        AssertRoundTrip(
            Context.Reserved,
            """{"type":"t","class":"c","default":"d","import":"i","null":"n","kebab-case":"k","with space":"w","$dollar":"d","1leading":"l"}"""
        );
        AssertRoundTrip(Context.WidgetReactions, """{"+1":3,"-1":1}""");
    }

    [Fact]
    public void OptionalNullsAreDroppedButRequiredNullsKept()
    {
        var withNull = Thing.Replace("\"mode\":\"only\"", "\"mode\":\"only\",\"nullable_optional\":null");
        AssertRoundTrip(Context.Thing, withNull, Thing);
    }

    [Fact]
    public void LargeIntegersDecimalsAndOffsetsAreExact()
    {
        var thing = JsonSerializer.Deserialize(Thing, Context.Thing)!;
        Assert.Equal(9007199254740993L, thing.Count);
        Assert.Equal(ulong.MaxValue, thing.Unsigned);
        Assert.Equal(12.50m, thing.Amount);
        Assert.Equal("12.50", thing.Amount!.Value.ToString(System.Globalization.CultureInfo.InvariantCulture));
        Assert.Equal(TimeSpan.FromHours(2), thing.CreatedAt.Offset);
    }

    [Fact]
    public void UnknownEnumValuesAreKept()
    {
        var thing = JsonSerializer.Deserialize(
            Thing.Replace("\"beta-2\"", "\"brand-new\"").Replace("\"priority\":-1", "\"priority\":99"),
            Context.Thing
        )!;
        Assert.False(thing.Kind.IsKnown);
        Assert.Equal("brand-new", thing.Kind.Value);
        Assert.False(thing.Priority!.Value.IsKnown);
        Assert.Equal(99, thing.Priority.Value.Value);
        Assert.True(Kind.Beta2.IsKnown);
        Assert.Equal(Kind.Beta2, Kind.FromValue("beta-2"));
        Assert.Equal(Priority.Negative, Priority.FromValue(-1));
        Assert.Contains("\"kind\":\"brand-new\"", JsonSerializer.Serialize(thing, Context.Thing));
    }

    [Fact]
    public void UnknownVariantsKeepTheirJson()
    {
        var shape = JsonSerializer.Deserialize("""{"type":"triangle","a":1}""", Context.Shape);
        var unknown = Assert.IsType<Shape.Unrecognized>(shape);
        Assert.Equal("triangle", unknown.Type);
        Assert.Equal(1, unknown.Raw.GetProperty("a").GetInt32());
    }

    [Fact]
    public void VariantsWrapTheirModelAndWriteTheTagOnce()
    {
        Shape shape = new Shape.Circle(new Circle { Type = "circle", Radius = 2.5 });
        var json = JsonSerializer.Serialize(shape, Context.Shape);
        Assert.Equal("""{"type":"circle","radius":2.5}""", json);
        var parsed = Assert.IsType<Shape.Circle>(JsonSerializer.Deserialize(json, Context.Shape));
        Assert.Equal(2.5, parsed.Value.Radius);
        var reopened = JsonSerializer.Deserialize("""{"kind":"reopened"}""", Context.Activity);
        Assert.Equal("reopened", Assert.IsType<Activity.Reopened>(reopened).Kind);
    }

    [Fact]
    public void PatchBodiesTellAbsentFromNull()
    {
        Assert.Equal("{}", JsonSerializer.Serialize(new ThingPatch(), Context.ThingPatch));
        Assert.Equal(
            """{"description":null}""",
            JsonSerializer.Serialize(new ThingPatch { Description = null }, Context.ThingPatch)
        );
        Assert.Equal(
            """{"count":3,"name":"n"}""",
            JsonSerializer.Serialize(new ThingPatch { Count = 3L, Name = "n" }, Context.ThingPatch)
        );
        var cleared = JsonSerializer.Deserialize("""{"description":null}""", Context.ThingPatch)!;
        Assert.True(cleared.Description.IsSet);
        Assert.Null(cleared.Description.Value);
        Assert.False(cleared.Count.IsSet);
        Assert.NotEqual(new ThingPatch(), cleared);
    }
}
