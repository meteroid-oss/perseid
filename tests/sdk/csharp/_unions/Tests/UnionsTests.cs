using System.Net;
using System.Text;
using System.Text.Json;
using System.Text.Json.Nodes;
using UnionsSdk;
using UnionsSdk.Models;

/// <summary>Decoding and encoding of unions sharing a JSON type, best-match object unions, union bodies
/// and open enums, on the SDK generated from tests/fixtures/edge-unions.yaml.</summary>
public class UnionsTests
{
    private static readonly UnionsSdkJsonContext Context = UnionsSdkJsonContext.Default;

    private const string Completed =
        """{"id":"c","model":"alpha-1","created_at":"2024-01-02T03:04:05Z","choices":[]""";

    private static Completion Decode(string extra = "") =>
        JsonSerializer.Deserialize(Completed + extra + "}", Context.Completion)!;

    private static void AssertJson(string expected, string actual) =>
        Assert.True(
            JsonNode.DeepEquals(JsonNode.Parse(expected), JsonNode.Parse(actual)),
            $"expected {expected}, got {actual}"
        );

    /// <summary>Answers every call with <see cref="Reply"/>, keeps the request bodies, and plays the token endpoint.</summary>
    private sealed class Server : HttpMessageHandler
    {
        public string Reply { get; set; } = "{}";

        public List<string> Bodies { get; } = [];

        protected override async Task<HttpResponseMessage> SendAsync(
            HttpRequestMessage request,
            CancellationToken cancellationToken
        )
        {
            var body = request.Content is null ? "" : await request.Content.ReadAsStringAsync(cancellationToken);
            if (request.RequestUri!.AbsolutePath.EndsWith("/oauth/token", StringComparison.Ordinal))
            {
                return Respond("""{"access_token":"at","token_type":"Bearer","expires_in":3600}""");
            }
            lock (Bodies)
            {
                Bodies.Add(body);
            }
            return Respond(Reply);
        }

        private static HttpResponseMessage Respond(string text) =>
            new(HttpStatusCode.OK) { Content = new StringContent(text, Encoding.UTF8, "application/json") };
    }
    private static UnionsSdkClient Client(Server server) =>
        new(
            null,
            new UnionsSdkClientOptions
            {
                BaseUrl = "https://unions.test/v1",
                ClientId = "id",
                ClientSecret = "secret",
                MaxRetries = 0,
                HttpMessageHandler = server,
            }
        );

    [Fact]
    public void PromptsPickTheVariantOfTheirJsonValue()
    {
        Assert.Equal("hi", Assert.IsType<CompletionPrompt.StringValue>(Decode(",\"prompt\":\"hi\"").Prompt).Value);
        Assert.Equal(
            new List<string> { "a", "b" },
            Assert.IsType<CompletionPrompt.ArrayOfStrings>(Decode(",\"prompt\":[\"a\",\"b\"]").Prompt).Value
        );
        Assert.Equal(
            new List<long> { 1, 2 },
            Assert.IsType<CompletionPrompt.ArrayOfIntegers>(Decode(",\"prompt\":[1,2]").Prompt).Value
        );
        Assert.Equal(
            new List<List<long>> { new() { 1 }, new() { 2, 3 } },
            Assert.IsType<CompletionPrompt.ArrayOfIntegerArrays>(Decode(",\"prompt\":[[1],[2,3]]").Prompt).Value
        );
    }

    [Fact]
    public void AnEmptyArrayIsTheFirstArrayVariant() =>
        Assert.IsType<CompletionPrompt.ArrayOfStrings>(Decode(",\"prompt\":[]").Prompt);

    [Fact]
    public void ValuesNoVariantFitsAreKeptAsReceived()
    {
        var prompt = Assert.IsType<CompletionPrompt.Unrecognized>(Decode(",\"prompt\":[true]").Prompt);
        Assert.Equal("[true]", prompt.Raw.GetRawText());
        // System.Text.Json writes the UTC offset of a DateTimeOffset as +00:00, not Z.
        AssertJson(
            Completed.Replace("05Z", "05+00:00", StringComparison.Ordinal) + ",\"prompt\":[true]}",
            JsonSerializer.Serialize(Decode(",\"prompt\":[true]"), Context.Completion)
        );
    }

    [Fact]
    public void ADateTimeFallsBackToAString()
    {
        Assert.Equal(
            new DateTimeOffset(2024, 1, 2, 3, 4, 5, TimeSpan.Zero),
            Assert.IsType<CompletionCreatedAt.DateTime>(Decode().CreatedAt).Value
        );
        var later = JsonSerializer.Deserialize(
            """{"id":"c","model":"alpha-1","created_at":"yesterday","choices":[]}""",
            Context.Completion
        )!;
        Assert.Equal("yesterday", Assert.IsType<CompletionCreatedAt.StringValue>(later.CreatedAt).Value);
    }

    [Fact]
    public void OpenEnumsKeepUnknownValues()
    {
        var known = Decode();
        Assert.Equal("alpha-1", known.Model.Value);
        Assert.True(known.Model.IsKnown);
        var unknown = JsonSerializer.Deserialize(
            """{"id":"c","model":"gamma-9","created_at":"2024-01-02T03:04:05Z","choices":[]}""",
            Context.Completion
        )!;
        Assert.Equal("gamma-9", unknown.Model.Value);
        Assert.False(unknown.Model.IsKnown);
        Assert.Contains("\"model\":\"gamma-9\"", JsonSerializer.Serialize(unknown, Context.Completion));
    }

    [Fact]
    public void OneFieldNoSdkCanTypeDoesNotUntypeItsModel()
    {
        var model = JsonSerializer.Deserialize("""{"name":"n","mixed":"auto","count":2}""", Context.Untypable)!;
        Assert.Equal("n", model.Name);
        Assert.Equal(2, model.Count);
        Assert.Equal("auto", model.Mixed?.ToString());
    }

    [Fact]
    public void ARequiredOnlyAnyOfLeavesTheModelAStruct()
    {
        var image = JsonSerializer.Deserialize("""{"image_url":"u"}""", Context.ImageRef)!;
        Assert.Equal("u", image.ImageUrl);
        Assert.Null(image.FileId);
    }

    [Fact]
    public async Task RequestsEncodeTheVariantTheCallerPicked()
    {
        var server = new Server { Reply = Completed + "}" };
        using var client = Client(server);
        await client.Completions.CreateAsync(
            new CreateCompletionRequest
            {
                Model = "alpha-1",
                Prompt = new CreateCompletionRequestPrompt.ArrayOfIntegerArrays([[1], [2, 3]]),
                Stop = new CreateCompletionRequestStop.ArrayOfStrings(["x", "y"]),
                Weight = 1.5,
                Timeout = new CreateCompletionRequestTimeout.IntegerValue(30),
            }
        );
        await client.Completions.CreateAsync(
            new CreateCompletionRequest { Model = "alpha-1", Prompt = new CreateCompletionRequestPrompt.StringValue("hi") }
        );
        Assert.Equal(2, server.Bodies.Count);
        AssertJson(
            """{"model":"alpha-1","prompt":[[1],[2,3]],"stop":["x","y"],"weight":1.5,"timeout":30}""",
            server.Bodies[0]
        );
        AssertJson("""{"model":"alpha-1","prompt":"hi"}""", server.Bodies[1]);
    }

    [Fact]
    public void ObjectsWithoutADiscriminatorAreDecodedByBestMatch()
    {
        Response Read(string choice) =>
            JsonSerializer.Deserialize(
                $$"""{"id":"r","model":"alpha-1","tool_choice":{{choice}}}""",
                Context.Response
            )!;

        Assert.Equal("auto", Assert.IsType<ResponseToolChoice.ToolChoiceEnum>(Read("\"auto\"").ToolChoice).Value.Value);
        Assert.Equal(
            "f",
            Assert.IsType<ResponseToolChoice.FunctionTool>(Read("""{"name":"f","arguments":"{}"}""").ToolChoice).Value.Name
        );
        Assert.Equal(
            "auto",
            Assert.IsType<ResponseToolChoice.AllowedTools>(Read("""{"mode":"auto","tools":[]}""").ToolChoice).Value.Mode
        );
        Assert.IsType<ResponseToolChoice.HostedTool>(Read("""{"type":"web_search"}""").ToolChoice);
        Assert.IsType<ResponseToolChoice.Unrecognized>(Read("""{"nothing":"fits"}""").ToolChoice);
    }

    [Fact]
    public void ObjectVariantsEncodeAsTheirModel()
    {
        var response = new Response
        {
            Id = "r",
            Model = "alpha-1",
            ToolChoice = new ResponseToolChoice.FunctionTool(new FunctionTool { Name = "f" }),
        };
        AssertJson(
            """{"id":"r","model":"alpha-1","tool_choice":{"name":"f"}}""",
            JsonSerializer.Serialize(response, Context.Response)
        );
    }

    [Fact]
    public async Task UnionResponseBodiesAreTyped()
    {
        var server = new Server { Reply = """{"text":"t"}""" };
        using var client = Client(server);
        var request = new CreateTranscriptionRequest { Model = "alpha-1", FileId = "f" };
        var plain = await client.Transcriptions.CreateAsync(request);
        Assert.Equal("t", Assert.IsType<CreateTranscriptionResponse.Transcription>(plain).Value.Text);

        server.Reply = """{"text":"t","duration":1.5,"language":"en"}""";
        var verbose = await client.Transcriptions.CreateAsync(request);
        Assert.Equal("en", Assert.IsType<CreateTranscriptionResponse.TranscriptionVerbose>(verbose).Value.Language);
    }

    [Fact]
    public async Task NamedUnionBodiesAreTypedInLists()
    {
        var server = new Server { Reply = """[{"text":"a"},{"text":"b","duration":1,"language":"en"}]""" };
        using var client = Client(server);
        var results = await client.Transcriptions.ListAsync();
        Assert.Equal(2, results.Count);
        Assert.IsType<TranscriptionResult.Transcription>(results[0]);
        Assert.IsType<TranscriptionResult.TranscriptionVerbose>(results[1]);
    }

    [Fact]
    public async Task UnionRequestBodiesTakeTheirVariants()
    {
        var server = new Server { Reply = """{"id":"g"}""" };
        using var client = Client(server);
        await client.Grades.CreateAsync(new CreateGradeRequest.GradeByText(new GradeByText { Text = "t" }));
        await client.Grades.CreateAsync(new GradeByScore { Score = 0.5 });
        Assert.Equal(2, server.Bodies.Count);
        AssertJson("""{"text":"t"}""", server.Bodies[0]);
        AssertJson("""{"score":0.5}""", server.Bodies[1]);
    }
}
