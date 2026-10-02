using System.Diagnostics;
using System.Net;
using System.Text;
using Microsoft.Extensions.DependencyInjection;
using Petstore;
using Petstore.Models;

public class ClientTests
{
    private const string PetJson =
        """{"id":"1","name":"Rex","created_at":"2024-01-01T00:00:00Z"}""";

    private sealed class Origin : HttpMessageHandler
    {
        public List<HttpRequestMessage> Seen { get; } = [];

        protected override Task<HttpResponseMessage> SendAsync(
            HttpRequestMessage request,
            CancellationToken cancellationToken
        )
        {
            Seen.Add(request);
            return Task.FromResult(
                new HttpResponseMessage(HttpStatusCode.OK)
                {
                    Content = new StringContent(PetJson, Encoding.UTF8, "application/json"),
                }
            );
        }
    }

    [Fact]
    public async Task DependencyInjectionGivesATypedClientOfTheHttpClientFactory()
    {
        var origin = new Origin();
        var services = new ServiceCollection();
        services
            .AddPetstoreClient(options =>
            {
                options.Token = "tok";
                options.BaseUrl = "https://petstore.test/v1";
            })
            .ConfigurePrimaryHttpMessageHandler(() => origin);
        using var provider = services.BuildServiceProvider();
        var client = provider.GetRequiredService<IPetstoreClient>();
        Assert.Equal("Rex", (await client.Pets.RetrieveAsync("1")).Name);
        Assert.Equal("https://petstore.test/v1/pets/1", origin.Seen[0].RequestUri!.ToString());
        Assert.Equal("Bearer tok", origin.Seen[0].Headers.Authorization!.ToString());
        Assert.NotSame(client, provider.GetRequiredService<IPetstoreClient>());
    }

    [Fact]
    public async Task TheServerOfTheSpecIsTheDefaultBaseUrl()
    {
        Assert.Equal("https://petstore.example.com", PetstoreClientOptions.DefaultBaseUrl);
        var origin = new Origin();
        using var client = new PetstoreClient("tok", new() { HttpMessageHandler = origin });
        await client.Pets.RetrieveAsync("1");
        Assert.Equal("https://petstore.example.com/pets/1", origin.Seen[0].RequestUri!.ToString());
    }

    [Fact]
    public void TheFactoryHttpClientLeavesTimeoutsToTheSdk()
    {
        var services = new ServiceCollection();
        services.AddPetstoreClient();
        using var provider = services.BuildServiceProvider();
        var http = provider.GetRequiredService<IHttpClientFactory>().CreateClient(nameof(PetstoreClient));
        Assert.Equal(Timeout.InfiniteTimeSpan, http.Timeout);
    }

    private sealed class FakePets : IPetsApi
    {
        public IPetsApiWithRawResponse WithRawResponse => throw new NotSupportedException();

        public Task<PetList> ListAsync(
            PetsListOptions? options = null,
            RequestOptions? requestOptions = null,
            CancellationToken cancellationToken = default
        ) => Task.FromResult(new PetList { Data = [] });

        public Task<Pet> CreateAsync(
            PetCreate petCreate,
            RequestOptions? requestOptions = null,
            CancellationToken cancellationToken = default
        ) => throw new NotSupportedException();

        public Task<Pet> RetrieveAsync(
            string petId,
            RequestOptions? requestOptions = null,
            CancellationToken cancellationToken = default
        ) =>
            Task.FromResult(
                new Pet
                {
                    Id = petId,
                    Name = "Fake",
                    CreatedAt = DateTimeOffset.UnixEpoch,
                }
            );

        public Task DeleteAsync(
            string petId,
            RequestOptions? requestOptions = null,
            CancellationToken cancellationToken = default
        ) => Task.CompletedTask;
    }

    private sealed class FakeClient : IPetstoreClient
    {
        public IPetsApi Pets { get; } = new FakePets();

        public void Dispose() { }
    }

    private static async Task<string> NameOf(IPetstoreClient client, string id) =>
        (await client.Pets.RetrieveAsync(id)).Name;

    [Fact]
    public async Task CodeDependingOnTheInterfacesTakesAFake()
    {
        Assert.Equal("Fake", await NameOf(new FakeClient(), "7"));
    }

    [Fact]
    public async Task EachCallIsTracedAsAnActivity()
    {
        var stopped = new System.Collections.Concurrent.ConcurrentBag<Activity>();
        using var listener = new ActivityListener
        {
            ShouldListenTo = source => source.Name == PetstoreClientOptions.ActivitySourceName,
            Sample = (ref ActivityCreationOptions<ActivityContext> _) =>
                ActivitySamplingResult.AllDataAndRecorded,
            ActivityStopped = stopped.Add,
        };
        ActivitySource.AddActivityListener(listener);
        using var client = new PetstoreClient(
            "tok",
            new PetstoreClientOptions { HttpMessageHandler = new Origin() }
        );
        await client.Pets.RetrieveAsync("traced");
        var activity = Assert.Single(
            stopped,
            a => (a.GetTagItem("url.full") as string)?.EndsWith("/pets/traced") == true
        );
        Assert.Equal("pets.retrieve", activity.OperationName);
        Assert.Equal(ActivityKind.Client, activity.Kind);
        Assert.Equal(200, activity.GetTagItem("http.response.status_code"));
    }

    [Fact]
    public void ModelsAreImmutableRecordsWithValueEquality()
    {
        var pet = new Pet
        {
            Id = "1",
            Name = "Rex",
            CreatedAt = DateTimeOffset.UnixEpoch,
        };
        Assert.Equal(pet, pet with { });
        Assert.NotEqual(pet, pet with { Name = "Max" });
        Assert.Equal(new PetList { Data = [pet] }, new PetList { Data = [pet with { }] });
    }
}
