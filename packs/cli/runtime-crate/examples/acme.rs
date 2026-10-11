//! The generated code of a CLI, by hand: the contract the pack's templates follow.
//! `cli` is what perseid renders; `sdk` fakes the Rust SDK (shaped like meteroid-rs) in process.
//! `ACME_API_KEY=sk_test cargo run --example acme -- things list --max-items 3`
use std::process::ExitCode;

use perseid_cli_runtime::app;

fn main() -> ExitCode {
    app::run(cli::APP, cli::command(), cli::dispatch)
}

/// The generated CLI.
pub mod cli {
    use clap::{value_parser, Arg, ArgMatches, Command};
    use perseid_cli_runtime::{
        app::{self, App, Ctx},
        auth::{ApiKey, Auth, BoxFuture, Method, Methods},
        body::{self, Body, Field, Kind},
        oauth::{self, Browser, DeviceCode},
        CliError, Credential,
    };

    use super::sdk;

    pub const APP: App = App {
        name: "acme",
        env_prefix: "ACME",
        auth: &AUTH,
        default_base_url: DEFAULT_BASE_URL,
        default_format: None,
        schema: SCHEMA,
    };

    /// The SDK's default base URL, which relative OAuth URLs resolve against without --base-url.
    pub const DEFAULT_BASE_URL: Option<&str> = Some("https://api.acme.dev");

    /// What perseid generates: `schema.json`, the types and the command words of resources,
    /// and in each resource module its commands.
    pub const SCHEMA: &[&str] = &[SCHEMA_JSON, things::SCHEMA];

    const SCHEMA_JSON: &str = r##"{
      "resources": {"things": "things"},
      "auth": {"api_key_headers": ["X-Api-Key"]},
      "$defs": {
        "Address": {"type": "object", "additionalProperties": false, "required": ["city"],
                    "properties": {"city": {"type": "string"}}},
        "CreateThingRequest": {"type": "object", "additionalProperties": false, "required": ["name"],
          "properties": {"name": {"type": "string"}, "quantity": {"type": "integer", "format": "int64"},
                         "tags": {"type": "array", "items": {"type": "string"}},
                         "address": {"$ref": "#/$defs/Address"}}},
        "Thing": {"type": "object", "properties": {"id": {"type": "string"}, "name": {"type": "string"}}}
      }
    }"##;

    /// From the spec's `oauth2` flows: `tokenUrl`, `authorizationUrl`, `deviceAuthorizationUrl`,
    /// absolute or relative to the base URL.
    const OAUTH: oauth::Client = oauth::Client::new("acme-cli", "https://auth.acme.dev/token")
        .scopes(&["things"])
        .revocation_url("https://auth.acme.dev/revoke");
    const BROWSER: Browser = Browser::new(OAUTH, "https://auth.acme.dev/authorize");

    /// `auth login` asks for an API key; `--web`, `--device` and `--web-key` log in otherwise.
    pub const AUTH: Methods = Methods {
        default: &ApiKey,
        methods: &[
            Method::new("web", "Log in with a browser", &BROWSER),
            Method::new(
                "device",
                "Log in with a code, from any device",
                &DeviceCode::new(OAUTH, "https://auth.acme.dev/device"),
            ),
            Method::new(
                "web-key",
                "Log in with a browser and store a new API key",
                &WebKey(&BROWSER),
            ),
        ],
    };

    /// A custom login: the browser flow, whose token mints an API key, the only thing stored.
    pub struct WebKey(pub &'static Browser);

    impl Auth for WebKey {
        fn login<'a>(
            &'a self,
            ctx: &'a mut Ctx,
            args: &'a ArgMatches,
        ) -> BoxFuture<'a, Result<Credential, CliError>> {
            Box::pin(async move {
                let session = self.0.login(ctx, args).await?;
                let token = session.access_token.unwrap_or_default();
                let key = client(ctx, &token)?.api_keys().create("acme CLI").await?;
                Ok(Credential::api_key(key.secret))
            })
        }

        fn token<'a>(
            &'a self,
            ctx: &'a mut Ctx,
            credential: &'a mut Credential,
        ) -> BoxFuture<'a, Result<String, CliError>> {
            ApiKey.token(ctx, credential)
        }
    }

    pub fn command() -> Command {
        let root = Command::new("acme")
            .about("The Acme API from the command line")
            .version("1.0.0")
            .subcommand(things::command());
        app::command(&APP, root)
    }

    pub async fn dispatch(mut ctx: Ctx, matches: ArgMatches) -> Result<(), CliError> {
        let token = ctx.token().await?;
        let client = client(&ctx, &token)?;
        match matches.subcommand() {
            Some(("things", matches)) => things::dispatch(&mut ctx, &client, matches).await,
            _ => unreachable!("clap requires a subcommand"),
        }
    }

    fn client(ctx: &Ctx, token: &str) -> Result<sdk::Acme, CliError> {
        let builder = sdk::Acme::builder().token(token);
        let builder = match ctx.base_url() {
            Some(url) => builder.base_url(url),
            None => builder,
        };
        // The SDK retries and times out; the real one also takes `ctx.attempts()` as middleware.
        let builder = match ctx.max_retries() {
            Some(retries) => builder.max_retries(retries),
            None => builder,
        };
        let builder = match ctx.timeout() {
            Some(timeout) => builder.timeout(timeout),
            None => builder,
        };
        Ok(builder.build()?)
    }

    impl From<sdk::error::Error> for CliError {
        fn from(error: sdk::error::Error) -> Self {
            use sdk::error::Error;
            // The real SDK's error is `#[non_exhaustive]`: add `_ => CliError::internal(...)`.
            match error {
                Error::Api(api) => {
                    CliError::api(api.status.as_u16(), &api.body).with_request_id(api.request_id())
                }
                Error::Timeout => CliError::timeout(),
                Error::Connection(source) => CliError::network(&source).with_source(source),
                Error::Decode(source) => CliError::decode(&source).with_source(source),
                Error::Request(source) => CliError::invalid_request(&source).with_source(source),
            }
        }
    }

    mod things {
        use perseid_cli_runtime::args;

        use super::*;

        pub const SCHEMA: &str = r##"{"commands": {
          "things.list": {"resource": "things", "command": "list",
            "method": "GET", "path": "/things", "summary": "List things",
            "params": [{"name": "search", "in": "query", "arg": "--search", "required": false,
                        "schema": {"type": "string"}},
                       {"name": "per_page", "in": "query", "arg": "--per-page", "required": false,
                        "schema": {"type": "integer", "format": "int32"}},
                       {"name": "page", "in": "query", "arg": "--page", "required": false,
                        "schema": {"type": "integer", "format": "int32"}}],
            "paginated": true, "pagination": {"style": "page", "arg": "--page"},
            "response": {"$ref": "#/$defs/Thing"}},
          "things.create": {"resource": "things", "command": "create",
            "method": "POST", "path": "/things", "summary": "Create a thing",
            "body": {"content": "json", "required": true, "schema": {"$ref": "#/$defs/CreateThingRequest"}},
            "response": {"$ref": "#/$defs/Thing"}, "example": {"name": "string"}}
        }}"##;

        pub fn command() -> Command {
            Command::new("things")
                .about("Things in the catalog")
                .subcommand_required(true)
                .subcommand(list_command())
                .subcommand(create_command())
        }

        pub async fn dispatch(
            ctx: &mut Ctx,
            client: &sdk::Acme,
            matches: &ArgMatches,
        ) -> Result<(), CliError> {
            match matches.subcommand() {
                Some(("list", matches)) => list(ctx, client, matches).await,
                Some(("create", matches)) => create(ctx, client, matches).await,
                _ => unreachable!("clap requires a subcommand"),
            }
        }

        fn list_command() -> Command {
            Command::new("list")
                .about("List things, every page")
                .arg(
                    Arg::new("search")
                        .long("search")
                        .value_name("STRING")
                        .help("Only things whose name contains this"),
                )
                .arg(
                    Arg::new("per-page")
                        .long("per-page")
                        .value_name("INTEGER")
                        .value_parser(value_parser!(i32))
                        .help("Number of items per page"),
                )
                .arg(
                    Arg::new("page")
                        .long("page")
                        .value_name("INTEGER")
                        .value_parser(value_parser!(i32))
                        .help("This page only"),
                )
        }

        async fn list(
            ctx: &mut Ctx,
            client: &sdk::Acme,
            matches: &ArgMatches,
        ) -> Result<(), CliError> {
            let mut options = sdk::api::ThingsListOptions::new()
                .maybe_search(matches.get_one::<String>("search").cloned())
                .maybe_per_page(matches.get_one::<i32>("per-page").copied());
            options.page = matches.get_one::<i32>("page").copied();
            let call = client.things().list(Some(options));
            // A page flag fetches that page; a terminal the first one, unless --all.
            let single = args::given(matches, "page");
            let mut items = ctx.items();
            if items.is_full() {
            } else if single || items.first_page_only() {
                let page = call.await?;
                items.fetched_page(page.has_next_page(), single);
                for thing in page.into_items() {
                    if !items.push(&thing)? {
                        break;
                    }
                }
            } else {
                let mut stream = call.items();
                while let Some(thing) = stream.next().await {
                    if !items.push(&thing?)? {
                        break;
                    }
                }
            }
            items.finish()
        }

        const CREATE_FIELDS: &[Field] = &[
            Field::new("name", "name", Kind::String).help("The thing's name"),
            Field::new("quantity", "quantity", Kind::Integer).help("How many are in stock"),
            Field::new("tag", "tags", Kind::String).many().help("A tag"),
            Field::new("address.city", "city", Kind::String)
                .under(&["address"])
                .help("Where it is stored"),
        ];

        fn create_command() -> Command {
            Command::new("create")
                .about("Create a thing")
                .args(body::args(CREATE_FIELDS))
        }

        async fn create(
            ctx: &mut Ctx,
            client: &sdk::Acme,
            matches: &ArgMatches,
        ) -> Result<(), CliError> {
            let body = Body::from_matches(matches, CREATE_FIELDS, ctx.stdin())?;
            let request: sdk::models::CreateThingRequest =
                body.validate(ctx, "things.create", false)?.decode()?;
            let thing = client.things().create(request).await?;
            ctx.print(&thing)
        }
    }
}

/// A fake of the SDK perseid generates, with its names and shapes, answering in process.
pub mod sdk {
    use std::cell::{Cell, RefCell};

    thread_local! {
        /// Pages fetched on this thread, so tests can check pagination stops early.
        pub static PAGES_FETCHED: Cell<usize> = const { Cell::new(0) };
        /// The token of the last client built on this thread.
        pub static LAST_TOKEN: RefCell<String> = const { RefCell::new(String::new()) };
        /// The retries and timeout of the last client built on this thread.
        pub static LAST_RETRIES: Cell<(u32, Option<std::time::Duration>)> = const { Cell::new((2, None)) };
    }

    pub mod error {
        pub type BoxError = Box<dyn std::error::Error + Send + Sync>;

        #[derive(Debug)]
        pub enum Error {
            Api(Box<ApiError>),
            Timeout,
            Connection(BoxError),
            Decode(BoxError),
            Request(BoxError),
        }

        /// meteroid-rs has `http::StatusCode` and `http::HeaderMap` here.
        #[derive(Debug)]
        pub struct ApiError {
            pub status: StatusCode,
            pub body: Vec<u8>,
            pub request_id: Option<String>,
        }

        impl ApiError {
            pub fn request_id(&self) -> Option<&str> {
                self.request_id.as_deref()
            }
        }

        #[derive(Clone, Copy, Debug)]
        pub struct StatusCode(pub u16);

        impl StatusCode {
            pub fn as_u16(self) -> u16 {
                self.0
            }
        }

        pub(crate) fn api(status: u16, body: &str) -> Error {
            Error::Api(Box::new(ApiError {
                status: StatusCode(status),
                body: body.as_bytes().to_vec(),
                request_id: Some("req_123".into()),
            }))
        }
    }

    pub mod models {
        use serde::{Deserialize, Serialize};

        #[derive(Clone, Debug, Serialize, Deserialize)]
        pub struct Address {
            pub city: String,
        }

        #[derive(Clone, Debug, Serialize, Deserialize)]
        pub struct Thing {
            pub id: String,
            pub name: String,
            pub quantity: i64,
            pub tags: Vec<String>,
            #[serde(skip_serializing_if = "Option::is_none")]
            pub address: Option<Address>,
        }

        #[derive(Clone, Debug, Serialize, Deserialize)]
        pub struct ApiKey {
            pub name: String,
            pub secret: String,
        }

        #[derive(Clone, Debug, Serialize, Deserialize)]
        pub struct CreateThingRequest {
            pub name: String,
            #[serde(default, skip_serializing_if = "Option::is_none")]
            pub quantity: Option<i64>,
            #[serde(default, skip_serializing_if = "Option::is_none")]
            pub tags: Option<Vec<String>>,
            #[serde(default, skip_serializing_if = "Option::is_none")]
            pub address: Option<Address>,
        }
    }

    pub mod api {
        use std::collections::VecDeque;

        use super::{
            error::{api, Error},
            models::{ApiKey, CreateThingRequest, Thing},
            LAST_RETRIES, LAST_TOKEN, PAGES_FETCHED,
        };

        #[derive(Clone, Debug, Default)]
        pub struct ThingsListOptions {
            pub search: Option<String>,
            pub page: Option<i32>,
            pub per_page: Option<i32>,
        }

        impl ThingsListOptions {
            pub fn new() -> Self {
                Self::default()
            }

            pub fn maybe_search(mut self, search: Option<String>) -> Self {
                self.search = search;
                self
            }

            pub fn maybe_per_page(mut self, per_page: Option<i32>) -> Self {
                self.per_page = per_page;
                self
            }
        }

        #[derive(Clone, Debug)]
        pub struct Acme {
            pub(crate) token: String,
            pub(crate) base_url: String,
        }

        pub struct AcmeBuilder {
            token: Option<String>,
            base_url: String,
            max_retries: u32,
            timeout: Option<std::time::Duration>,
        }

        impl Acme {
            pub fn builder() -> AcmeBuilder {
                AcmeBuilder {
                    token: None,
                    base_url: "https://api.acme.dev".into(),
                    max_retries: 2,
                    timeout: Some(std::time::Duration::from_secs(60)),
                }
            }

            pub fn things(&self) -> Things<'_> {
                Things { client: self }
            }

            pub fn api_keys(&self) -> ApiKeys<'_> {
                ApiKeys { client: self }
            }

            fn check(&self) -> Result<(), Error> {
                if self.base_url.contains("unreachable") {
                    return Err(Error::Connection("dns error: no such host".into()));
                }
                if self.token == "sk_bad" {
                    return Err(api(401, r#"{"message": "invalid API key"}"#));
                }
                Ok(())
            }
        }

        impl AcmeBuilder {
            pub fn token(mut self, token: impl Into<String>) -> Self {
                self.token = Some(token.into());
                self
            }

            pub fn base_url(mut self, base_url: impl Into<String>) -> Self {
                self.base_url = base_url.into();
                self
            }

            pub fn max_retries(mut self, max_retries: u32) -> Self {
                self.max_retries = max_retries;
                self
            }

            pub fn timeout(mut self, timeout: impl Into<Option<std::time::Duration>>) -> Self {
                self.timeout = timeout.into();
                self
            }

            pub fn build(self) -> Result<Acme, Error> {
                if !self.base_url.starts_with("http") {
                    return Err(Error::Request(
                        format!("invalid base URL `{}`", self.base_url).into(),
                    ));
                }
                let token = self.token.unwrap_or_default();
                LAST_TOKEN.with(|last| last.replace(token.clone()));
                LAST_RETRIES.with(|last| last.set((self.max_retries, self.timeout)));
                Ok(Acme {
                    token,
                    base_url: self.base_url,
                })
            }
        }

        pub struct ApiKeys<'c> {
            client: &'c Acme,
        }

        impl ApiKeys<'_> {
            pub async fn create(&self, name: &str) -> Result<ApiKey, Error> {
                self.client.check()?;
                Ok(ApiKey {
                    name: name.into(),
                    secret: format!("sk_minted_{}", self.client.token),
                })
            }
        }

        pub struct Things<'c> {
            client: &'c Acme,
        }

        fn catalog() -> Vec<Thing> {
            ["Widget", "Gadget", "Gizmo", "Doohickey", "Widget XL"]
                .iter()
                .enumerate()
                .map(|(i, name)| Thing {
                    id: format!("th_{}", i + 1),
                    name: (*name).into(),
                    quantity: (i as i64 + 1) * 10,
                    tags: vec![],
                    address: None,
                })
                .collect()
        }

        impl Things<'_> {
            pub fn list(&self, options: Option<ThingsListOptions>) -> PageCall {
                PageCall {
                    client: self.client.clone(),
                    options: options.unwrap_or_default(),
                }
            }

            pub async fn create(&self, request: CreateThingRequest) -> Result<Thing, Error> {
                self.client.check()?;
                if request.name == "taken" {
                    return Err(api(
                        409,
                        r#"{"message": "a thing named `taken` exists", "code": "name_taken"}"#,
                    ));
                }
                Ok(Thing {
                    id: "th_6".into(),
                    name: request.name,
                    quantity: request.quantity.unwrap_or(0),
                    tags: request.tags.unwrap_or_default(),
                    address: request.address,
                })
            }
        }

        pub struct PageCall {
            client: Acme,
            options: ThingsListOptions,
        }

        impl PageCall {
            pub fn items(self) -> Paginator {
                let page = self.call_page();
                Paginator {
                    call: self,
                    buffer: VecDeque::new(),
                    page,
                    done: false,
                }
            }

            fn call_page(&self) -> i32 {
                self.options.page.unwrap_or(0)
            }
        }

        /// One page: its items, and whether another follows.
        pub struct Page {
            items: Vec<Thing>,
            next: bool,
        }

        impl Page {
            pub fn has_next_page(&self) -> bool {
                self.next
            }

            pub fn into_items(self) -> Vec<Thing> {
                self.items
            }
        }

        impl std::future::IntoFuture for PageCall {
            type Output = Result<Page, Error>;
            type IntoFuture = std::future::Ready<Self::Output>;

            fn into_future(self) -> Self::IntoFuture {
                let mut paginator = self.items();
                std::future::ready(paginator.fetch().map(|()| Page {
                    items: paginator.buffer.into(),
                    next: !paginator.done,
                }))
            }
        }

        /// Every item across pages, fetching each page when needed.
        pub struct Paginator {
            call: PageCall,
            buffer: VecDeque<Thing>,
            page: i32,
            done: bool,
        }

        impl Paginator {
            pub async fn next(&mut self) -> Option<Result<Thing, Error>> {
                if self.buffer.is_empty() && !self.done {
                    if let Err(error) = self.fetch() {
                        self.done = true;
                        return Some(Err(error));
                    }
                }
                self.buffer.pop_front().map(Ok)
            }

            fn fetch(&mut self) -> Result<(), Error> {
                self.call.client.check()?;
                PAGES_FETCHED.with(|pages| pages.set(pages.get() + 1));
                let per_page = self.call.options.per_page.unwrap_or(2).max(1) as usize;
                let search = self.call.options.search.as_deref().unwrap_or("");
                let matching: Vec<Thing> = catalog()
                    .into_iter()
                    .filter(|t| t.name.contains(search))
                    .collect();
                let start = self.page as usize * per_page;
                let page: Vec<Thing> = matching
                    .iter()
                    .skip(start)
                    .take(per_page)
                    .cloned()
                    .collect();
                self.done = start + page.len() >= matching.len();
                self.page += 1;
                self.buffer.extend(page);
                Ok(())
            }
        }
    }

    pub use api::Acme;
}
