//! Schema names that cannot be type names in a generated SDK, and the rule that renames them.
//!
//! A schema whose UpperCamelCase name is reserved gets a `Model` suffix, one starting with a digit
//! an `N` prefix, so every schema name is a valid, unshadowed identifier in every language.
//! Reserved names are the types and modules the runtime declares (read from the embedded runtime
//! files), plus the standard-library and dependency types generated code uses unqualified.

use std::collections::BTreeSet;

use heck::ToUpperCamelCase as _;

/// The type name to give a schema called `name`: UpperCamelCase, prefixed with `N` when it starts
/// with a digit, `Schema` when nothing is left, and suffixed with `Model` when `reserved`.
/// Names that become equal are told apart by the caller.
pub(crate) fn safe_type_name(name: &str, reserved: &BTreeSet<String>) -> String {
    let mut safe = name.to_upper_camel_case();
    if safe.is_empty() {
        safe = "Schema".to_owned();
    } else if safe.starts_with(|c: char| c.is_ascii_digit()) {
        safe.insert(0, 'N');
    }
    if reserved.contains(&safe) {
        safe.push_str("Model");
    }
    safe
}

/// Type names that generated code uses unqualified next to the models, per language.
pub(crate) fn type_names(language: &str, client: &str) -> BTreeSet<String> {
    let external: &[&str] = match language {
        "rust" => RUST,
        "typescript" => TYPESCRIPT,
        "python" => PYTHON,
        "go" => GO,
        "java" => JAVA,
        "csharp" => CSHARP,
        _ => &[],
    };
    let own: &[&str] = match language {
        "rust" => &[""],
        "typescript" => &["", "Request", "RequestContext"],
        "java" => &["", "HttpClient", "Options"],
        "csharp" => &["Client", "ClientOptions", "JsonContext"],
        "python" => &["", "Async", "Error", "Options"],
        "go" => &[""],
        _ => &[],
    };
    let mut names: BTreeSet<String> = external.iter().map(|n| (*n).to_owned()).collect();
    names.extend(own.iter().map(|suffix| format!("{client}{suffix}")));
    names.extend(runtime_names(language, client));
    names
}

/// Types the runtime of `language` declares at the top level of its files, and for Go, where one
/// directory holds models and runtime alike, the types whose file would replace a runtime file.
fn runtime_names(language: &str, client: &str) -> BTreeSet<String> {
    const KEYWORDS: &[&str] = &[
        "type",
        "struct",
        "enum",
        "trait",
        "class",
        "interface",
        "record",
        "func",
        "const",
        "var",
        "def",
        "fn",
        "static",
    ];
    let prefix = format!("runtime/{language}");
    let mut names = BTreeSet::new();
    for (path, content) in crate::assets::under(&prefix) {
        let text = String::from_utf8_lossy(content);
        let text = text.replace("@@CLIENT_NAME@@", client);
        if language == "go" && !path.contains('/') {
            let stem = path.trim_end_matches(".go");
            if !stem.contains("@@") {
                names.insert(stem.to_upper_camel_case());
            }
        }
        for line in text.lines().filter(|l| !l.starts_with(char::is_whitespace)) {
            let mut tokens = line
                .split(|c: char| !(c.is_alphanumeric() || c == '_'))
                .filter(|t| !t.is_empty());
            while let Some(token) = tokens.next() {
                if !KEYWORDS.contains(&token) {
                    continue;
                }
                if let Some(next) = tokens.next()
                    && next.starts_with(char::is_uppercase)
                    && next.chars().any(char::is_lowercase)
                {
                    names.insert(next.to_owned());
                }
                break;
            }
        }
    }
    names
}

const RUST: &[&str] = &[
    "Arc",
    "Box",
    "Clone",
    "Codec",
    "Cow",
    "Configuration",
    "Copy",
    "Credentials",
    "Debug",
    "Default",
    "Deserialize",
    "Deserializer",
    "Drop",
    "Duration",
    "Eq",
    "Err",
    "Error",
    "Fn",
    "FnMut",
    "FnOnce",
    "From",
    "Hash",
    "HttpClient",
    "Into",
    "Iterator",
    "None",
    "Ok",
    "Option",
    "Ord",
    "PartialEq",
    "PartialOrd",
    "Result",
    "Self",
    "Send",
    "Serialize",
    "Serializer",
    "Sized",
    "Some",
    "String",
    "Sync",
    "ToString",
    "TryFrom",
    "TryInto",
    "UnionRules",
    "Vec",
];

const TYPESCRIPT: &[&str] = &[
    "AbortController",
    "AbortSignal",
    "Array",
    "ArrayBuffer",
    "BigInt",
    "BinaryResponse",
    "Blob",
    "Boolean",
    "Date",
    "Error",
    "EventStream",
    "Exclude",
    "Extract",
    "File",
    "FormData",
    "Function",
    "Headers",
    "JSON",
    "Map",
    "Math",
    "Middleware",
    "MultipartBody",
    "NonNullable",
    "Number",
    "Object",
    "Omit",
    "Partial",
    "Pick",
    "Promise",
    "ReadableStream",
    "Readonly",
    "Record",
    "RequestInit",
    "RequestOptions",
    "Required",
    "Response",
    "ReturnType",
    "Security",
    "SecurityScheme",
    "Set",
    "String",
    "Symbol",
    "TextDecoder",
    "TextEncoder",
    "URL",
    "URLSearchParams",
    "Uint8Array",
    "Upload",
    "UploadBody",
    "XOR",
];

const PYTHON: &[&str] = &[
    "Any",
    "ApiBaseAsync",
    "ApiBaseSync",
    "ApiRequest",
    "AsyncBinaryResponse",
    "AsyncEventStream",
    "AsyncPage",
    "AsyncPaginator",
    "AsyncStream",
    "BaseModel",
    "BinaryResponse",
    "Decimal",
    "Discriminator",
    "Enum",
    "EventStream",
    "False",
    "FileInput",
    "Final",
    "IntEnum",
    "Literal",
    "ModelParseError",
    "None",
    "ObjectUnion",
    "Optional",
    "Pagination",
    "Paging",
    "SecurityScheme",
    "StrEnum",
    "Stream",
    "SyncPage",
    "TaggedUnionModel",
    "Timeout",
    "True",
    "TypeVar",
    "UNSET",
    "Union",
    "UnknownVariant",
    "Unset",
    "Upload",
    "UploadContent",
];

// Go declares everything in one directory, where file names count too.
const GO: &[&str] = &[
    "APIError",
    "ApiError",
    "AutoPager",
    "BasicAuth",
    "Client",
    "Collections",
    "DecodeError",
    "DefaultNumRetries",
    "DefaultServerURL",
    "DefaultTimeout",
    "ErrBadRequest",
    "ErrConflict",
    "ErrForbidden",
    "ErrNotFound",
    "ErrRateLimited",
    "ErrServer",
    "ErrUnauthorized",
    "ErrUnprocessableEntity",
    "ErrWebhookInvalidTimestamp",
    "ErrWebhookMissingHeaders",
    "ErrWebhookNoMatchingSignature",
    "ErrWebhookTimestampTooNew",
    "ErrWebhookTimestampTooOld",
    "ErrorBody",
    "ErrorStatus",
    "Errors",
    "EventStream",
    "ExtraFields",
    "Middleware",
    "Models",
    "NewWebhook",
    "NewWebhookRaw",
    "Null",
    "Nullable",
    "Options",
    "Page",
    "Pager",
    "Ptr",
    "Request",
    "RequestAuth",
    "RequestError",
    "RequestOption",
    "RequestPager",
    "RequestStreaming",
    "RequiredMap",
    "RequiredSlice",
    "RoundTripperFunc",
    "SdkError",
    "Set",
    "SseEvent",
    "Stream",
    "TimeoutError",
    "TransportError",
    "UnionError",
    "UnionRules",
    "Upload",
    "Version",
    "Webhook",
    "WebhookSecretPrefix",
    "WebhookTolerance",
    "Webhooks",
    "WithHeader",
    "WithIdempotencyKey",
    "WithMaxRetries",
    "WithTimeout",
];

const JAVA: &[&str] = &[
    "AbstractMap",
    "ApiException",
    "ArrayDeque",
    "ArrayList",
    "Base64",
    "BigDecimal",
    "BigInteger",
    "Boolean",
    "Builder",
    "Byte",
    "Call",
    "Character",
    "Class",
    "Collection",
    "Collections",
    "Collectors",
    "Comparable",
    "CompletableFuture",
    "DateTimeFormatter",
    "DateTimeParseException",
    "Deprecated",
    "Deque",
    "DeserializationContext",
    "DeserializationFeature",
    "Double",
    "Duration",
    "Enum",
    "Error",
    "EventStream",
    "Exception",
    "File",
    "Float",
    "FunctionalInterface",
    "GeneralSecurityException",
    "HashMap",
    "Headers",
    "HttpHeaders",
    "HttpUrl",
    "IOException",
    "InputStream",
    "Instant",
    "Integer",
    "Interceptor",
    "InterruptedIOException",
    "Iterable",
    "Iterator",
    "JavaTimeModule",
    "JavaType",
    "Jdk8Module",
    "JsonAnyGetter",
    "JsonAnySetter",
    "JsonAutoDetect",
    "JsonCreator",
    "JsonDeserialize",
    "JsonGenerator",
    "JsonIgnoreProperties",
    "JsonInclude",
    "JsonNode",
    "JsonParser",
    "JsonProcessingException",
    "JsonProperty",
    "JsonSubTypes",
    "JsonTypeInfo",
    "JsonTypeName",
    "JsonUnwrapped",
    "JsonValue",
    "LinkedHashMap",
    "LinkedHashSet",
    "List",
    "LocalDate",
    "Long",
    "Mac",
    "Map",
    "MediaType",
    "MessageDigest",
    "Multipart",
    "MultipartBody",
    "NoSuchElementException",
    "Number",
    "Object",
    "ObjectMapper",
    "Objects",
    "OffsetDateTime",
    "OkHttpClient",
    "Optional",
    "Override",
    "Paginator",
    "Request",
    "RequestBody",
    "RequestOptions",
    "Response",
    "Runnable",
    "RuntimeException",
    "SafeVarargs",
    "SecretKeySpec",
    "SerializationFeature",
    "SerializerProvider",
    "Set",
    "Short",
    "SimpleModule",
    "SocketTimeoutException",
    "Source",
    "StandardCharsets",
    "StdDeserializer",
    "StdSerializer",
    "Stream",
    "StreamSupport",
    "String",
    "StringBuilder",
    "Supplier",
    "SuppressWarnings",
    "System",
    "Thread",
    "ThreadLocalRandom",
    "Throwable",
    "ToQueryParam",
    "TreeMap",
    "TypeReference",
    "URI",
    "UUID",
    "UncheckedIOException",
    "Upload",
    "Utils",
    "Visibility",
    "Void",
    "ZonedDateTime",
];

// Ours, and those of the namespaces resources import, which would become ambiguous.
const CSHARP: &[&str] = &[
    "Action",
    "Activator",
    "AggregateException",
    "ApiAuth",
    "ApiException",
    "ApiExceptionExtensions",
    "ApiRequest",
    "ApiTransport",
    "ArgumentException",
    "ArgumentNullException",
    "ArgumentOutOfRangeException",
    "Array",
    "ArrayList",
    "ArraySegment",
    "Assembly",
    "Attribute",
    "Barrier",
    "BasicCredentials",
    "BitConverter",
    "Boolean",
    "Buffer",
    "Byte",
    "CallerMemberName",
    "CancellationToken",
    "CancellationTokenSource",
    "Char",
    "Collection",
    "Comparer",
    "Console",
    "Convert",
    "Credentials",
    "CultureInfo",
    "DateOnly",
    "DateTime",
    "DateTimeOffset",
    "DateTimeStyles",
    "Decimal",
    "Default",
    "Delegate",
    "Dictionary",
    "Directory",
    "Double",
    "Encoding",
    "Enum",
    "Enumerable",
    "Environment",
    "EventArgs",
    "EventHandler",
    "EventStream",
    "Exception",
    "File",
    "FormatException",
    "Forwarder",
    "Func",
    "GC",
    "Guid",
    "HashSet",
    "Hashtable",
    "HMAC",
    "HMACSHA256",
    "HttpClient",
    "HttpContent",
    "HttpMethod",
    "HttpRequestException",
    "HttpRequestMessage",
    "HttpResponseMessage",
    "HttpStatusCode",
    "IAsyncDisposable",
    "IComparable",
    "ICollection",
    "IDictionary",
    "IDisposable",
    "IEnumerable",
    "IEnumerator",
    "IEquatable",
    "IFormattable",
    "IGrouping",
    "IIntegerEnum",
    "IList",
    "Index",
    "Int16",
    "Int32",
    "Int64",
    "IntegerEnumConverter",
    "Interlocked",
    "InvalidOperationException",
    "IReadOnlyDictionary",
    "IReadOnlyList",
    "ISet",
    "IStringEnum",
    "Json",
    "JsonArray",
    "JsonConstructor",
    "JsonConverter",
    "JsonDocument",
    "JsonElement",
    "JsonException",
    "JsonIgnore",
    "JsonNode",
    "JsonObject",
    "JsonPropertyName",
    "JsonSerializer",
    "JsonSerializerContext",
    "JsonSerializerOptions",
    "JsonValue",
    "JsonValueKind",
    "KeyNotFoundException",
    "KeyValuePair",
    "Lazy",
    "LinkedList",
    "List",
    "Lock",
    "Math",
    "MaybeUnset",
    "MaybeUnsetConverter",
    "Memory",
    "MemoryStream",
    "Module",
    "Monitor",
    "MultipartBody",
    "MultipartContent",
    "Mutex",
    "NotFoundException",
    "NotImplementedException",
    "NotSupportedException",
    "Nullable",
    "Object",
    "Obsolete",
    "Options",
    "Pagination",
    "Paginator",
    "Parallel",
    "Path",
    "Predicate",
    "Queue",
    "Random",
    "Range",
    "RateLimitException",
    "ReadOnlyMemory",
    "ReadOnlySpan",
    "RequestOptions",
    "SByte",
    "SchemeKind",
    "SecurityScheme",
    "Semaphore",
    "ServerErrorException",
    "SHA256",
    "Single",
    "SortedDictionary",
    "SortedSet",
    "Span",
    "SseEvent",
    "SseParser",
    "Stack",
    "Stream",
    "StreamContent",
    "String",
    "StringBuilder",
    "StringComparer",
    "StringComparison",
    "StringContent",
    "StringEnumConverter",
    "Task",
    "Thread",
    "TimeOnly",
    "TimeSpan",
    "Timeout",
    "Timer",
    "Tuple",
    "Type",
    "UInt16",
    "UInt32",
    "UInt64",
    "UnauthorizedException",
    "UnionRules",
    "UnprocessableEntityException",
    "Unrecognized",
    "Unsafe",
    "Upload",
    "Uri",
    "UriKind",
    "Utf8JsonReader",
    "Utf8JsonWriter",
    "ValueTask",
    "ValueTuple",
    "Version",
    "Void",
    "Volatile",
    "Webhook",
    "WebhookVerificationException",
];

#[cfg(test)]
mod tests {
    use super::*;

    fn reserved(language: &str) -> BTreeSet<String> {
        type_names(language, "Acme")
    }

    #[test]
    fn digits_empty_names_and_reserved_names_become_safe() {
        let reserved = BTreeSet::from(["Upload".to_owned()]);
        assert_eq!(safe_type_name("3DModel", &reserved), "N3dModel");
        assert_eq!(safe_type_name("1thing", &reserved), "N1thing");
        assert_eq!(safe_type_name("upload", &reserved), "UploadModel");
        assert_eq!(safe_type_name("user_profile", &reserved), "UserProfile");
        assert_eq!(safe_type_name("", &reserved), "Schema");
    }

    #[test]
    fn known_collisions_are_reserved_per_language() {
        let expected: &[(&str, &[&str])] = &[
            (
                "rust",
                &["Self", "Codec", "UnionRules", "Serialize", "Deserialize"],
            ),
            (
                "go",
                &[
                    "ApiError",
                    "Ptr",
                    "Null",
                    "Set",
                    "ErrorBody",
                    "WithHeader",
                    "ErrorStatus",
                    "UnionRules",
                ],
            ),
            (
                "python",
                &["None", "IntEnum", "Unset", "UnknownVariant", "BaseModel"],
            ),
            (
                "java",
                &[
                    "JsonProperty",
                    "JsonInclude",
                    "Short",
                    "Deprecated",
                    "Override",
                    "SuppressWarnings",
                    "FunctionalInterface",
                    "String",
                    "Integer",
                ],
            ),
            (
                "csharp",
                &["UnionRules", "IStringEnum", "SseParser", "String"],
            ),
            ("typescript", &["Response", "Upload", "Set"]),
        ];
        for (language, names) in expected {
            let reserved = reserved(language);
            for name in *names {
                assert!(reserved.contains(*name), "{language} should reserve {name}");
            }
        }
    }

    #[test]
    fn runtime_declarations_are_reserved() {
        for (language, name) in [
            ("go", "UnionRules"),
            ("go", "RoundTripperFunc"),
            ("go", "RequestPager"),
            ("rust", "Paginator"),
            ("java", "AsyncPage"),
            ("csharp", "SseParser"),
            ("python", "TaggedUnionModel"),
            ("typescript", "ErrorParsers"),
        ] {
            assert!(
                runtime_names(language, "Acme").contains(name),
                "{language} runtime declares {name}"
            );
        }
        assert!(!runtime_names("java", "Acme").contains("Builder"));
    }

    #[test]
    fn the_client_name_is_reserved() {
        assert!(reserved("java").contains("AcmeHttpClient"));
        assert!(reserved("csharp").contains("AcmeClient"));
        assert!(reserved("go").contains("Acme"));
        assert!(reserved("python").contains("AcmeError"));
        assert!(reserved("rust").contains("Acme"));
    }
}
