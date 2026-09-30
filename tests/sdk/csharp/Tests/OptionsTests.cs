using Petstore;

public class OptionsTests
{
    [Fact]
    public void TheTimeoutDefaultsToTheConfiguredOne()
    {
        Assert.Equal(TimeSpan.FromSeconds(60), PetstoreClientOptions.DefaultTimeout);
        Assert.Equal(PetstoreClientOptions.DefaultTimeout, new PetstoreClientOptions().Timeout);
    }
}
