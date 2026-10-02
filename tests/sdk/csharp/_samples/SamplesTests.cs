using System.Globalization;
using System.Reflection;
using System.Text.Json;
using System.Text.Json.Serialization;
using System.Text.RegularExpressions;

/// <summary>
/// Decodes and re-encodes the <c>perseid samples</c> of every model of one fixture. Each sample of
/// each model must decode into the generated type and encode back to the same JSON, up to the
/// documented normalizations of <see cref="Differences"/>: date-times compare as instants, decimal
/// strings as numbers, and an optional property set to <c>null</c> may be dropped (the models cannot
/// tell it from an unset one). Integers, int64 and uint64 included, compare exactly.
/// <c>PERSEID_SAMPLES</c> names the samples.json, <c>PERSEID_SDK</c> the assembly of the SDK.
/// </summary>
public class SamplesTests
{
    private static readonly Regex Instant = new(
        @"^\d{4}-\d\d-\d\d[Tt]\d\d:\d\d:\d\d(\.\d+)?([Zz]|[+-]\d\d:\d\d)$"
    );
    private static readonly Regex DecimalText = new(@"^-?\d+(\.\d+)?$");

    private static bool TryDecimal(string text, out decimal value) =>
        decimal.TryParse(text, NumberStyles.Float, CultureInfo.InvariantCulture, out value);

    private static bool SameDecimal(string a, string b) =>
        TryDecimal(a, out var x) && TryDecimal(b, out var y) && x == y;

    private static bool SameNumber(string a, string b)
    {
        if (TryDecimal(a, out var x) && TryDecimal(b, out var y))
        {
            return x == y;
        }
        return a == b
            || (
                double.TryParse(a, NumberStyles.Float, CultureInfo.InvariantCulture, out var d)
                && double.TryParse(b, NumberStyles.Float, CultureInfo.InvariantCulture, out var e)
                && d == e
            );
    }

    private static bool SameInstant(string a, string b) =>
        Instant.IsMatch(a)
        && Instant.IsMatch(b)
        && DateTimeOffset.TryParse(a, CultureInfo.InvariantCulture, DateTimeStyles.None, out var x)
        && DateTimeOffset.TryParse(b, CultureInfo.InvariantCulture, DateTimeStyles.None, out var y)
        && x.UtcTicks == y.UtcTicks;

    /// <summary>Where <paramref name="got"/> differs from <paramref name="want"/>, beyond the normalizations.</summary>
    private static List<string> Differences(JsonElement want, JsonElement got, string path = "$")
    {
        var mismatch = new List<string> { $"{path}: expected {want.GetRawText()}, got {got.GetRawText()}" };
        switch (want.ValueKind)
        {
            case JsonValueKind.Object when got.ValueKind == JsonValueKind.Object:
            {
                var found = new List<string>();
                foreach (var property in want.EnumerateObject())
                {
                    if (got.TryGetProperty(property.Name, out var other))
                    {
                        found.AddRange(Differences(property.Value, other, $"{path}.{property.Name}"));
                    }
                    else if (property.Value.ValueKind != JsonValueKind.Null)
                    {
                        found.Add($"{path}.{property.Name}: missing, expected {property.Value.GetRawText()}");
                    }
                }
                foreach (var property in got.EnumerateObject())
                {
                    if (!want.TryGetProperty(property.Name, out _))
                    {
                        found.Add($"{path}.{property.Name}: unexpected {property.Value.GetRawText()}");
                    }
                }
                return found;
            }
            case JsonValueKind.Array when got.ValueKind == JsonValueKind.Array:
            {
                var wanted = want.EnumerateArray().ToList();
                var gotten = got.EnumerateArray().ToList();
                if (wanted.Count != gotten.Count)
                {
                    return new List<string> { $"{path}: expected {wanted.Count} items, got {gotten.Count}" };
                }
                var found = new List<string>();
                for (var index = 0; index < wanted.Count; index++)
                {
                    found.AddRange(Differences(wanted[index], gotten[index], $"{path}[{index}]"));
                }
                return found;
            }
            case JsonValueKind.String:
            {
                var text = want.GetString()!;
                if (got.ValueKind == JsonValueKind.String)
                {
                    var other = got.GetString()!;
                    var same = text == other || SameInstant(text, other) || (DecimalText.IsMatch(text) && DecimalText.IsMatch(other) && SameDecimal(text, other));
                    return same ? new List<string>() : mismatch;
                }
                var number = got.ValueKind == JsonValueKind.Number
                    && DecimalText.IsMatch(text)
                    && SameDecimal(text, got.GetRawText());
                return number ? new List<string>() : mismatch;
            }
            case JsonValueKind.Number:
            {
                if (got.ValueKind == JsonValueKind.Number)
                {
                    return SameNumber(want.GetRawText(), got.GetRawText()) ? new List<string>() : mismatch;
                }
                var text = got.ValueKind == JsonValueKind.String ? got.GetString()! : "";
                var decimalText = DecimalText.IsMatch(text) && SameDecimal(want.GetRawText(), text);
                return decimalText ? new List<string>() : mismatch;
            }
            case JsonValueKind.True:
            case JsonValueKind.False:
            case JsonValueKind.Null:
                return want.ValueKind == got.ValueKind ? new List<string>() : mismatch;
            default:
                return mismatch;
        }
    }

    private static List<string> Differences(string want, string got)
    {
        using var wanted = JsonDocument.Parse(want);
        using var gotten = JsonDocument.Parse(got);
        return Differences(wanted.RootElement, gotten.RootElement);
    }

    [Fact]
    public void OnlyTheDocumentedNormalizationsAreAccepted()
    {
        Assert.Empty(Differences("\"2024-01-02T03:04:05Z\"", "\"2024-01-02T04:04:05.000+01:00\""));
        Assert.Empty(Differences("\"12.50\"", "12.5"));
        Assert.Empty(Differences("12.5", "\"12.50\""));
        Assert.Empty(Differences("1", "1.0"));
        Assert.Empty(Differences("{\"a\":null}", "{}"));
        Assert.Empty(Differences("18446744073709551615", "18446744073709551615"));
        Assert.NotEmpty(Differences("9007199254740993", "9007199254740992"));
        Assert.NotEmpty(Differences("9007199254740993", "9007199254740992.0"));
        Assert.NotEmpty(Differences("-9223372036854775808", "-9223372036854775807"));
        Assert.NotEmpty(Differences("\"2024-01-02T03:04:05Z\"", "\"2024-01-02T03:04:06Z\""));
        Assert.NotEmpty(Differences("true", "1"));
        Assert.NotEmpty(Differences("null", "0"));
        Assert.NotEmpty(Differences("\"a\"", "\"b\""));
        Assert.NotEmpty(Differences("{}", "{\"a\":null}"));
        Assert.NotEmpty(Differences("{\"a\":1}", "{}"));
        Assert.NotEmpty(Differences("[1,2]", "[1]"));
        Assert.NotEmpty(Differences("[null]", "[]"));
    }

    [Fact]
    public void EverySampleOfEveryModelRoundTrips()
    {
        var samplesPath = Environment.GetEnvironmentVariable("PERSEID_SAMPLES");
        var sdk = Environment.GetEnvironmentVariable("PERSEID_SDK");
        if (string.IsNullOrEmpty(samplesPath) || string.IsNullOrEmpty(sdk))
        {
            throw new InvalidOperationException(
                "set PERSEID_SAMPLES to the samples.json of the fixture and PERSEID_SDK to the assembly name of its SDK"
            );
        }
        var assembly = Assembly.Load(sdk);
        var contextType = assembly
            .GetTypes()
            .First(type =>
                !type.IsAbstract
                && typeof(JsonSerializerContext).IsAssignableFrom(type)
                && type.Name.EndsWith("JsonContext", StringComparison.Ordinal)
            );
        var context = (JsonSerializerContext)
            contextType.GetProperty("Default", BindingFlags.Public | BindingFlags.Static)!.GetValue(null)!;

        using var document = JsonDocument.Parse(File.ReadAllText(samplesPath));
        var failures = new List<string>();
        var skipped = new List<string>();
        var models = 0;
        var samples = 0;
        foreach (var model in document.RootElement.EnumerateObject())
        {
            var kind = model.Value.GetProperty("kind").GetString()!;
            var name = model.Value.GetProperty("names").GetProperty("csharp").GetString()!;
            var list = model.Value.GetProperty("samples");
            if (list.GetArrayLength() == 0)
            {
                failures.Add($"{model.Name}: no samples");
                continue;
            }
            var type = assembly.GetType($"{contextType.Namespace}.{name}");
            if (type is null)
            {
                // Aliases and unions of plain values have no type of their own: the models that
                // use them carry them as JSON or as a union nested in their own type.
                if (kind is "alias" or "union")
                {
                    skipped.Add(model.Name);
                }
                else
                {
                    failures.Add($"{model.Name} ({kind}): the SDK has no type {name}");
                }
                continue;
            }
            if (context.GetTypeInfo(type) is null)
            {
                failures.Add($"{model.Name}: {type.FullName} is missing from the JSON context");
                continue;
            }
            models++;
            foreach (var sample in list.EnumerateArray())
            {
                var label = $"{model.Name}/{sample.GetProperty("name").GetString()}";
                samples++;
                try
                {
                    foreach (var difference in RoundTrip(context, type, sample.GetProperty("json")))
                    {
                        failures.Add($"{label}: {difference}");
                    }
                }
                catch (Exception e)
                {
                    failures.Add($"{label}: {e.GetType().Name}: {e.Message}");
                }
            }
        }
        Assert.True(
            failures.Count == 0,
            $"{failures.Count} failures over {samples} samples of {models} models ({skipped.Count} without a type: {string.Join(", ", skipped)}):\n{string.Join("\n", failures)}"
        );
        Assert.True(models > 0, "the fixture has models with a type");
    }

    private static List<string> RoundTrip(JsonSerializerContext context, Type type, JsonElement sample)
    {
        var parsed =
            JsonSerializer.Deserialize(sample.GetRawText(), type, context)
            ?? throw new InvalidOperationException("the sample decoded to null");
        var encoded = JsonSerializer.Serialize(parsed, type, context);
        using var first = JsonDocument.Parse(encoded);
        var found = Differences(sample, first.RootElement);
        // Decoding what was encoded and encoding it again gives the same JSON.
        var reparsed =
            JsonSerializer.Deserialize(encoded, type, context)
            ?? throw new InvalidOperationException("the encoded sample decoded to null");
        using var second = JsonDocument.Parse(JsonSerializer.Serialize(reparsed, type, context));
        found.AddRange(Differences(first.RootElement, second.RootElement, "$ (second pass)"));
        return found;
    }
}
