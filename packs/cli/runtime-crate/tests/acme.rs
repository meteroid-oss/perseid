//! The hand-written generated CLI of `examples/acme.rs`, run in process end to end.
#[allow(dead_code)]
#[path = "../examples/acme.rs"]
mod acme;

use std::{
    collections::HashMap,
    fs,
    io::{self, BufRead, BufReader, Read, Write},
    net::{TcpListener, TcpStream},
    path::{Path, PathBuf},
    sync::{mpsc, Arc, Mutex},
    thread,
    time::{Duration, Instant},
};

use acme::{
    cli,
    sdk::{LAST_RETRIES, LAST_TOKEN, PAGES_FETCHED},
};
use perseid_cli_runtime::{
    app::{Captured, Host},
    exit,
    oauth::{self, Browser, DeviceCode},
    run_with, ApiKey, App, Method, Methods,
};
use serde_json::{json, Value};
use url::Url;

struct Run {
    code: u8,
    stdout: String,
    stderr: String,
}

impl Run {
    fn json(&self) -> Value {
        serde_json::from_str(&self.stdout).unwrap_or_else(|e| panic!("{e}: {}", self.stdout))
    }

    /// The last line of stderr, after any login prompt.
    fn error(&self) -> Value {
        let line = self.stderr.lines().last().unwrap_or_default();
        serde_json::from_str(line).unwrap_or_else(|e| panic!("{e}: {}", self.stderr))
    }
}

fn run_in(args: &str, stdin: &str, setup: impl FnOnce(&mut Host)) -> Run {
    run_app(cli::APP, args, stdin, setup)
}

fn run_app(app: App, args: &str, stdin: &str, setup: impl FnOnce(&mut Host)) -> Run {
    #[cfg(feature = "keyring")]
    perseid_cli_runtime::keyring::use_mock_store();
    let (mut host, captured) = Host::captured(stdin);
    host.env
        .insert("ACME_API_KEY".into(), "sk_test_0000".into());
    setup(&mut host);
    let argv = std::iter::once("acme").chain(args.split_whitespace());
    let code = run_with(app, cli::command(), argv, host, cli::dispatch);
    Run {
        code,
        stdout: captured.stdout.text(),
        stderr: captured.stderr.text(),
    }
}

fn run(args: &str) -> Run {
    run_in(args, "", |_| {})
}

#[test]
fn lists_stream_every_page_as_jsonl_off_a_terminal() {
    let out = run("things list");
    assert_eq!(out.code, exit::OK, "{}", out.stderr);
    let ids: Vec<Value> = out
        .stdout
        .lines()
        .map(|line| serde_json::from_str::<Value>(line).unwrap()["id"].clone())
        .collect();
    assert_eq!(
        ids,
        [
            json!("th_1"),
            json!("th_2"),
            json!("th_3"),
            json!("th_4"),
            json!("th_5")
        ]
    );
    assert_eq!(PAGES_FETCHED.with(|pages| pages.get()), 3);
}

#[test]
fn max_items_stops_fetching_pages() {
    let out = run("things list --max-items 3 --jq .id");
    assert_eq!(out.stdout, "th_1\nth_2\nth_3\n");
    assert_eq!(PAGES_FETCHED.with(|pages| pages.get()), 2);
}

#[test]
fn query_flags_reach_the_sdk() {
    let out = run("things list --search Widget --per-page 10 --format json --jq map(.name)");
    assert_eq!(out.json(), json!(["Widget", "Widget XL"]));
    assert_eq!(PAGES_FETCHED.with(|pages| pages.get()), 1);
}

#[test]
fn tables_on_request() {
    let out = run("things list --search Wid --format table");
    assert_eq!(
        out.stdout,
        "id    name       quantity  tags\nth_1  Widget     10        []\nth_5  Widget XL  50        []\n"
    );
}

#[test]
fn a_terminal_gets_a_table_by_default() {
    let out = run_in("things list --max-items 1 --no-color", "", |host| {
        host.stdout_tty = true;
        host.width = Some(80);
    });
    assert_eq!(
        out.stdout,
        "id    name    quantity  tags\nth_1  Widget  10        []\n"
    );
}

#[test]
fn create_merges_data_and_field_flags_into_the_request() {
    let out = run("things create -d {\"quantity\":2,\"name\":\"Old\"} --name Widget --tag a --tag b --address.city Paris");
    assert_eq!(out.code, exit::OK, "{}", out.stderr);
    assert_eq!(
        out.stdout,
        "{\"id\":\"th_6\",\"name\":\"Widget\",\"quantity\":2,\"tags\":[\"a\",\"b\"],\"address\":{\"city\":\"Paris\"}}\n"
    );
}

#[test]
fn create_reads_the_body_from_stdin() {
    let out = run_in(
        "things create --data - --format yaml",
        r#"{"name": "Piped", "tags": ["x"]}"#,
        |_| {},
    );
    assert_eq!(
        out.stdout,
        "id: th_6\nname: Piped\nquantity: 0\ntags:\n- x\n"
    );
}

#[test]
fn body_errors_are_usage_errors_naming_the_flag() {
    let out = run("things create --quantity 2");
    assert_eq!(out.code, exit::USAGE);
    assert_eq!(out.stdout, "");
    assert_eq!(
        out.error(),
        json!({"error": {
            "kind": "usage", "status": null,
            "message": "invalid body: name (--name): required (a string)",
            "body": null, "request_id": null, "retryable": false, "retry_after": null,
            "errors": [{"path": "name", "flag": "--name", "message": "required (a string)"}]
        }})
    );
    let out = run("things create --name x -d {\"nmae\":1,\"address\":{\"town\":\"P\"}}");
    let errors = &out.error()["error"]["errors"];
    assert_eq!(errors[0]["message"], "unknown field; did you mean name?");
    assert_eq!(errors[1]["path"], "address.city");
    assert_eq!(errors[2]["message"], "unknown field; expected one of city");
    let allowed = run("things create --name x -d {\"nmae\":1} --allow-unknown-fields");
    assert_eq!(allowed.code, exit::OK, "{}", allowed.stderr);
    let out = run("things create --name x --quantity many");
    assert_eq!(
        out.error()["error"]["message"],
        "--quantity: expected an integer, got `many`"
    );
}

#[test]
fn api_errors_carry_the_status_and_body() {
    let out = run("things create --name taken");
    assert_eq!(out.code, exit::API_CLIENT);
    assert_eq!(
        out.error(),
        json!({"error": {
            "kind": "api", "status": 409,
            "message": "API error 409: a thing named `taken` exists",
            "body": {"message": "a thing named `taken` exists", "code": "name_taken"},
            "request_id": "req_123", "retryable": false, "retry_after": null, "errors": null
        }})
    );
}

#[test]
fn a_failing_filter_keeps_the_response_of_a_successful_call() {
    let out = run("things create --name Lamp --jq .name.first");
    assert_eq!(out.code, exit::OUTPUT);
    assert_eq!(out.json()["id"], "th_6");
    assert_eq!(out.error()["error"]["kind"], "output");
}

#[test]
fn a_page_flag_fetches_that_page_and_a_terminal_the_first() {
    let out = run("things list --page 1 --jq .id");
    assert_eq!(out.stdout, "th_3\nth_4\n");
    assert_eq!(PAGES_FETCHED.with(|pages| pages.get()), 1);
    let tty = run_in("things list --format jsonl --jq .id", "", |host| {
        host.stdout_tty = true
    });
    assert_eq!(tty.stdout, "th_1\nth_2\n");
    assert!(
        tty.stderr.starts_with("Showing the first 2 items: --all"),
        "{}",
        tty.stderr
    );
    let all = run_in("things list --all --jq .id", "", |host| {
        host.stdout_tty = true
    });
    assert_eq!(all.stdout.lines().count(), 5);
    let none = run("things list --max-items 0");
    assert_eq!((none.code, none.stdout.as_str()), (exit::OK, ""));
}

#[test]
fn the_format_comes_from_the_environment_too() {
    let out = run_in("things list --max-items 1", "", |host| {
        host.stdout_tty = true;
        host.stderr_tty = true;
        host.env.insert("ACME_FORMAT".into(), "json".into());
    });
    assert_eq!(out.stdout, "[\n  {\n    \"id\": \"th_1\",\n    \"name\": \"Widget\",\n    \"quantity\": 10,\n    \"tags\": []\n  }\n]\n");
    let flag = run_in("things list --max-items 1 --format jsonl", "", |host| {
        host.env.insert("ACME_FORMAT".into(), "table".into());
    });
    assert_eq!(flag.stdout.lines().count(), 1);
    let json_errors = run_in("things list --api-key sk_bad", "", |host| {
        host.stderr_tty = true;
        host.env.insert("ACME_FORMAT".into(), "jsonl".into());
    });
    assert_eq!(json_errors.error()["error"]["status"], 401);
    let bad = run_in("things list", "", |host| {
        host.env.insert("ACME_FORMAT".into(), "xml".into());
    });
    assert_eq!(bad.code, exit::USAGE);
}

#[test]
fn schema_prints_the_index_and_the_schema_of_a_command() {
    let index = run("schema");
    assert_eq!(index.json().as_array().unwrap().len(), 2);
    let things = run("schema things create");
    assert_eq!(things.json()["body"]["required"], json!(["name"]));
    assert_eq!(
        things.json()["body"]["properties"]["address"]["required"],
        json!(["city"])
    );
    let response = run("schema things list --response");
    assert_eq!(
        response.json()["response"]["properties"]["id"],
        json!({"type": "string"})
    );
    let unknown = run("schema nothing");
    assert_eq!(unknown.code, exit::USAGE);
    let guide = run("agents");
    assert!(guide.stdout.len() < 2048, "{}", guide.stdout.len());
    assert!(guide.stdout.contains("acme schema"));
}

#[test]
fn errors_are_human_on_a_terminal() {
    let out = run_in("things list --api-key sk_bad", "", |host| {
        host.stderr_tty = true;
        host.env.insert("NO_COLOR".into(), "1".into());
    });
    assert_eq!(out.code, exit::API_CLIENT);
    assert_eq!(
        out.stderr,
        "error: API error 401: invalid API key\n  request id: req_123\n"
    );
    let colored = run_in("things list --api-key sk_bad", "", |host| {
        host.stderr_tty = true
    });
    assert!(colored
        .stderr
        .starts_with("\x1b[1;31merror:\x1b[0m API error 401"));
    let no_color = run_in("things list --api-key sk_bad --no-color", "", |host| {
        host.stderr_tty = true
    });
    assert!(no_color.stderr.starts_with("error: API error 401"));
}

#[test]
fn network_and_request_errors() {
    let out = run("things list --base-url http://unreachable.test");
    assert_eq!(out.code, exit::NETWORK);
    assert_eq!(out.error()["error"]["kind"], "network");
    let out = run("things list --base-url ftp.acme.dev");
    assert_eq!(out.code, exit::USAGE);
    assert_eq!(
        out.error()["error"]["message"],
        "invalid request: invalid base URL `ftp.acme.dev`"
    );
}

#[test]
fn a_missing_api_key_says_how_to_set_one() {
    let out = run_in("things list", "", |host| {
        host.env.clear();
    });
    assert_eq!(out.code, exit::CONFIG);
    assert_eq!(
        out.error()["error"]["message"],
        "not logged in: run `acme auth login`, pass --api-key or set ACME_API_KEY"
    );
}

#[test]
fn parse_errors_are_json_off_a_terminal() {
    let out = run("things list --bogus");
    assert_eq!(out.code, exit::USAGE);
    let message = out.error()["error"]["message"].as_str().unwrap().to_owned();
    assert!(
        message.starts_with("unexpected argument '--bogus' found"),
        "{message}"
    );
    let human = run_in("things list --bogus", "", |host| {
        host.stderr_tty = true;
        host.env.insert("NO_COLOR".into(), "1".into());
    });
    assert!(human
        .stderr
        .starts_with("error: unexpected argument '--bogus' found"));
    let jq = run("things list --jq .[");
    assert_eq!(jq.code, exit::USAGE);
    assert_eq!(PAGES_FETCHED.with(|pages| pages.get()), 0);
}

#[test]
fn help_is_golden() {
    let out = run("things create --help");
    assert_eq!(out.code, exit::OK);
    assert_eq!(
        out.stdout,
        r#"Create a thing

Usage: acme things create [OPTIONS]

Options:
  -h, --help  Print help

Body:
  -d, --data <JSON|@FILE|->    Request body as JSON, @file to read it from a file, - from stdin; field flags override it
      --name <STRING>          The thing's name
      --quantity <INTEGER>     How many are in stock
      --tag <STRING>           A tag (repeatable)
      --address.city <STRING>  Where it is stored

Global options: --format --jq --pretty --max-items --all --dry-run --allow-unknown-fields --profile --api-key --base-url --retries --no-retry --timeout --no-color --debug; see `acme --help`
"#
    );
    let root = run("--help");
    let mcp = match cfg!(feature = "mcp-http") {
        true => "on stdio or over HTTP",
        false => "on stdin and stdout",
    };
    assert_eq!(
        root.stdout,
        format!(
            r#"The Acme API from the command line

Usage: acme <COMMAND>

Commands:
  things      Things in the catalog
  schema      Print the JSON Schema of a command's input, or the index of commands
  agents      How to use this CLI from scripts and agents: output, errors, discovery
  mcp         Serve the API's operations as the tools of an MCP server, {mcp}
  auth        Manage credentials and profiles
  completion  Print a shell completion script
  help        Print this message or the help of the given subcommand(s)

Options:
  -h, --help     Print help
  -V, --version  Print version

Global options:
      --format <FORMAT>       json, jsonl, yaml or table [env: ACME_FORMAT] [default: table on a terminal, else json; jsonl for lists and streams]
      --jq <FILTER>           jq filter (jaq: jq's builtins, @csv, @tsv...), run on each item of a list, on the whole array with --format json; strings print raw
      --pretty                Indent JSON off a terminal too
      --max-items <N>         Stop lists after N items, fetching no further page
      --all                   Fetch every page of a list on a terminal too, which shows the first page
      --dry-run               Print the request as JSON instead of sending it: method, url, query, headers, body
      --allow-unknown-fields  Send body fields and enum values the schema does not know
      --profile <NAME>        Profile of the config file to use [env: ACME_PROFILE]
      --api-key <KEY>         API key [env: ACME_API_KEY]
      --base-url <URL>        API base URL [env: ACME_BASE_URL]
      --retries <N>           Retry a request up to N times on no response, 408, 429 or 5xx, if idempotent or with an idempotency key [env: ACME_RETRIES] [default: 2]
      --no-retry              Never retry, as --retries 0
      --timeout <SECS>        Timeout of each attempt in seconds, 0 for none [env: ACME_TIMEOUT] [default: the SDK's]
      --no-color              Disable colors (also when NO_COLOR is set)
      --debug                 Print diagnostics to stderr
"#
        )
    );
    let auth = run("auth --help");
    assert!(
        auth.stdout.contains("Commands:\n  login     Log in and store the credential in a profile\n  logout    Remove the credential of a profile\n  status    Show the credential commands use: profile, method, expiry, base URL\n  token     Print the bearer token commands send, refreshed if needed\n  profiles  List the profiles of the config file\n"),
        "{}",
        auth.stdout
    );
    let bare = run("");
    assert_eq!(bare.code, exit::USAGE);
    assert!(bare
        .stderr
        .starts_with("The Acme API from the command line\n"));
}

/// The config directory `dir`, secrets in the file.
fn with_config(dir: &Path) -> impl FnOnce(&mut Host) + '_ {
    move |host| {
        host.env.clear();
        host.env.insert("ACME_NO_KEYRING".into(), "1".into());
        host.config_dir = Some(dir.to_owned());
    }
}

#[test]
fn login_profiles_and_logout() {
    let dir = tempfile::tempdir().unwrap();
    let login = run_in(
        "auth login",
        "sk_live_abcdefgh1234\n",
        with_config(dir.path()),
    );
    assert_eq!(login.code, exit::OK, "{}", login.stderr);
    let path = dir.path().join("acme/config.toml");
    assert!(
        login
            .stderr
            .starts_with("Logged in to profile `default` (api key sk_live_…1234), saved to "),
        "{}",
        login.stderr
    );

    let staging = run_in(
        "auth login --profile staging --api-key sk_staging_99999999 --base-url http://staging.acme.dev",
        "",
        with_config(dir.path()),
    );
    assert_eq!(staging.code, exit::OK, "{}", staging.stderr);
    assert_eq!(
        fs::read_to_string(&path).unwrap(),
        "default = \"default\"\n\n[profiles.default]\napi_key = \"sk_live_abcdefgh1234\"\n\n[profiles.staging]\napi_key = \"sk_staging_99999999\"\nbase_url = \"http://staging.acme.dev\"\n"
    );

    let profiles = run_in("auth profiles", "", with_config(dir.path()));
    assert_eq!(
        profiles.json(),
        json!([
            {"name": "default", "default": true, "base_url": null, "auth": "api key sk_live_…1234"},
            {"name": "staging", "default": false, "base_url": "http://staging.acme.dev", "auth": "api key sk_staging_…9999"}
        ])
    );

    let listed = run_in(
        "things list --max-items 1 --profile staging --debug",
        "",
        with_config(dir.path()),
    );
    assert_eq!(listed.code, exit::OK, "{}", listed.stderr);
    assert!(
        listed
            .stderr
            .contains("debug: profile `staging` (--profile)\n"),
        "{}",
        listed.stderr
    );
    assert!(listed
        .stderr
        .contains("debug: auth api key sk_staging_…9999 (profile `staging`)\n"));
    assert!(listed
        .stderr
        .contains("debug: base url http://staging.acme.dev (profile `staging`)\n"));

    let unknown = run_in("things list --profile nope", "", with_config(dir.path()));
    assert_eq!(unknown.code, exit::CONFIG);

    let logout = run_in("auth logout --profile staging", "", with_config(dir.path()));
    assert_eq!(
        logout.stderr,
        "Removed the credential of profile `staging`\n"
    );
    let no_key = run_in("things list --profile staging", "", with_config(dir.path()));
    assert_eq!(no_key.code, exit::CONFIG);
}

#[test]
fn auth_status_and_token_never_leak_more_than_asked() {
    let dir = tempfile::tempdir().unwrap();
    let none = run_in("auth status", "", with_config(dir.path()));
    assert_eq!(none.code, exit::CONFIG);
    assert_eq!(none.json()["logged_in"], false);
    assert_eq!(none.json()["method"], Value::Null);
    assert_eq!(
        run_in("auth token", "", with_config(dir.path())).code,
        exit::CONFIG
    );

    let login = run_in(
        "auth login --api-key-stdin --base-url http://u:p@localhost:1",
        "sk_live_abcdefgh1234\n",
        with_config(dir.path()),
    );
    assert_eq!(login.code, exit::OK, "{}", login.stderr);
    let status = run_in("auth status", "", with_config(dir.path()));
    assert_eq!(status.code, exit::OK, "{}", status.stderr);
    assert_eq!(
        status.json(),
        json!({"profile": "default", "logged_in": true, "method": "api_key",
               "source": "profile `default`", "auth": "api key sk_live_…1234",
               "expires_at": null, "expires_in": null, "refreshable": false,
               "base_url": "http://***@localhost:1", "storage": "file"})
    );
    assert!(!status.stdout.contains("abcdefgh"));
    let token = run_in("auth token", "", with_config(dir.path()));
    assert_eq!(token.stdout, "sk_live_abcdefgh1234\n");

    let env = run_in("auth status", "", |host| {
        with_config(dir.path())(host);
        host.env
            .insert("ACME_API_KEY".into(), "sk_env_123456789".into());
    });
    assert_eq!(env.json()["source"], "$ACME_API_KEY");
    assert_eq!(env.json()["storage"], Value::Null);

    let both = run_in(
        "auth login --api-key-stdin --api-key sk",
        "sk\n",
        with_config(dir.path()),
    );
    assert_eq!(both.code, exit::USAGE);
    let empty = run_in("auth login --api-key-stdin", "\n", with_config(dir.path()));
    assert_eq!(empty.code, exit::USAGE);
}

#[cfg(feature = "keyring")]
#[test]
fn secrets_go_to_the_keyring_and_stay_there_on_refresh() {
    let server = OAuthServer::start(|request, _| match request.path.as_str() {
        "/token" => tokens("at_new", "rt_new"),
        _ => (200, json!({})),
    });
    let app = server.app();
    let dir = tempfile::tempdir().unwrap();
    let keyring = |host: &mut Host| {
        with_config(dir.path())(host);
        host.env.remove("ACME_NO_KEYRING");
    };
    let login = run_app(
        app,
        "auth login --profile kr --api-key sk_kr_123456789",
        "",
        keyring,
    );
    assert_eq!(login.code, exit::OK, "{}", login.stderr);
    assert!(
        login.stderr.ends_with(", its secrets in the OS keyring\n"),
        "{}",
        login.stderr
    );
    let stored = &config(dir.path())["profiles"]["kr"];
    assert_eq!(stored.get("api_key"), None);
    assert_eq!(stored["secrets"].as_str(), Some("keyring"));
    let listed = run_app(app, "things list --max-items 1 --profile kr", "", keyring);
    assert_eq!(listed.code, exit::OK, "{}", listed.stderr);
    assert_eq!(LAST_TOKEN.with(|t| t.borrow().clone()), "sk_kr_123456789");
    let profiles = run_app(app, "auth profiles", "", keyring);
    assert_eq!(
        profiles.json()[0]["auth"],
        "api key sk_kr_…6789, in the OS keyring"
    );

    let sealed = |access: &str| {
        let mut credential = perseid_cli_runtime::Credential {
            access_token: Some(access.into()),
            refresh_token: Some("rt_old".into()),
            expires_at: Some(1),
            method: Some("web".into()),
            ..Default::default()
        };
        perseid_cli_runtime::keyring::store("acme", "kr", &mut credential).unwrap();
        let mut file = perseid_cli_runtime::config::ConfigFile::default();
        file.login("kr", credential, None);
        file.save(&dir.path().join("acme/config.toml")).unwrap();
    };
    sealed("at_old");
    let refreshed = run_app(app, "things list --max-items 1 --profile kr", "", keyring);
    assert_eq!(refreshed.code, exit::OK, "{}", refreshed.stderr);
    assert_eq!(LAST_TOKEN.with(|t| t.borrow().clone()), "at_new");
    let stored = &config(dir.path())["profiles"]["kr"];
    assert_eq!(
        (stored.get("access_token"), stored.get("refresh_token")),
        (None, None)
    );
    assert!(stored["expires_at"].as_integer().unwrap() > 1);
    let token = run_app(app, "auth token --profile kr", "", keyring);
    assert_eq!(token.stdout, "at_new\n");
    let status = run_app(app, "auth status --profile kr", "", keyring);
    assert_eq!(
        (
            status.json()["storage"].clone(),
            status.json()["refreshable"].clone()
        ),
        (json!("keyring"), json!(true))
    );

    let plain = run_app(
        app,
        "auth login --profile kr --no-keyring --api-key sk_file_123456",
        "",
        keyring,
    );
    assert_eq!(plain.code, exit::OK, "{}", plain.stderr);
    let stored = &config(dir.path())["profiles"]["kr"];
    assert_eq!(
        (stored["api_key"].as_str(), stored.get("secrets")),
        (Some("sk_file_123456"), None)
    );
    let mut gone = perseid_cli_runtime::Credential {
        secrets: Some("keyring".into()),
        ..Default::default()
    };
    assert!(perseid_cli_runtime::keyring::load("acme", "kr", &mut gone).is_err());

    run_app(
        app,
        "auth login --profile kr2 --api-key sk_kr2_123456789",
        "",
        keyring,
    );
    let logout = run_app(app, "auth logout --profile kr2", "", keyring);
    assert_eq!(logout.code, exit::OK, "{}", logout.stderr);
    let mut gone = perseid_cli_runtime::Credential {
        secrets: Some("keyring".into()),
        ..Default::default()
    };
    assert!(perseid_cli_runtime::keyring::load("acme", "kr2", &mut gone).is_err());
}

#[test]
fn login_rejects_an_empty_key() {
    let dir = tempfile::tempdir().unwrap();
    let out = run_in("auth login", "\n", with_config(dir.path()));
    assert_eq!(out.code, exit::USAGE);
    assert!(!dir.path().join("acme/config.toml").exists());
}

#[test]
fn completion_and_man_pages() {
    let bash = run("completion bash");
    assert_eq!(bash.code, exit::OK);
    assert!(bash.stdout.contains("_acme()"));
    assert!(bash.stdout.contains("things"));
    let zsh = run("completion zsh");
    assert!(zsh.stdout.starts_with("#compdef acme"));

    let man = run("man");
    assert!(man.stdout.contains(".TH acme 1"), "{}", man.stdout);
    let dir = tempfile::tempdir().unwrap();
    let pages = run(&format!("man --out-dir {}", dir.path().display()));
    assert_eq!(pages.code, exit::OK, "{}", pages.stderr);
    for page in [
        "acme.1",
        "acme-things.1",
        "acme-things-list.1",
        "acme-things-create.1",
        "acme-auth.1",
        "acme-auth-login.1",
    ] {
        assert!(dir.path().join(page).exists(), "{page}");
    }
    assert!(!dir.path().join("acme-man.1").exists());
}

struct ClosedPipe;

impl io::Write for ClosedPipe {
    fn write(&mut self, _: &[u8]) -> io::Result<usize> {
        Err(io::ErrorKind::BrokenPipe.into())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

#[test]
fn a_closed_stdout_ends_the_run_quietly() {
    let out = run_in("things list", "", |host| host.stdout = Box::new(ClosedPipe));
    assert_eq!(out.code, exit::OK);
    assert_eq!(out.stderr, "");
    assert_eq!(PAGES_FETCHED.with(|pages| pages.get()), 1);
    for args in ["completion bash", "man"] {
        let out = run_in(args, "", |host| host.stdout = Box::new(ClosedPipe));
        assert_eq!((out.code, out.stderr.as_str()), (exit::OK, ""), "{args}");
    }
}

#[test]
fn an_explicit_key_works_despite_config_errors() {
    let dir = tempfile::tempdir().unwrap();
    write_config(dir.path(), "profiles = 3");
    let with_key = |host: &mut Host| {
        with_config(dir.path())(host);
        host.env.insert("ACME_API_KEY".into(), "sk_env".into());
        host.env.insert("ACME_PROFILE".into(), "nope".into());
    };
    let out = run_in("things list --max-items 1 --debug", "", with_key);
    assert_eq!(out.code, exit::OK, "{}", out.stderr);
    assert!(
        out.stderr.contains("(ignored: the API key is explicit)"),
        "{}",
        out.stderr
    );
    assert_eq!(LAST_TOKEN.with(|t| t.borrow().clone()), "sk_env");

    let flag = run_in(
        "things list --max-items 1 --api-key sk_flag",
        "",
        with_config(dir.path()),
    );
    assert_eq!((flag.code, flag.stderr.as_str()), (exit::OK, ""));
    let without = run_in("things list", "", with_config(dir.path()));
    assert_eq!(without.code, exit::CONFIG);
}

#[test]
fn debug_hides_the_userinfo_of_the_base_url() {
    let out =
        run("things list --max-items 1 --debug --base-url https://jane:s3cret@api.acme.dev/v1");
    assert_eq!(out.code, exit::OK, "{}", out.stderr);
    assert!(
        out.stderr
            .contains("debug: base url https://***@api.acme.dev/v1 (--base-url)\n"),
        "{}",
        out.stderr
    );
    assert!(!out.stderr.contains("s3cret"));
    let default = run("things list --max-items 1 --debug");
    assert!(default
        .stderr
        .contains("debug: base url https://api.acme.dev (default)\n"));
}

#[test]
fn retries_and_timeouts_reach_the_sdk() {
    let retries = || LAST_RETRIES.with(|last| last.get());
    run("things list --max-items 1");
    assert_eq!(retries(), (2, Some(Duration::from_secs(60))));
    run("things list --max-items 1 --retries 5 --timeout 1.5");
    assert_eq!(retries(), (5, Some(Duration::from_millis(1500))));
    run("things list --max-items 1 --no-retry --timeout 0");
    assert_eq!(retries(), (0, None));
    let env = |host: &mut Host| {
        host.env.insert("ACME_RETRIES".into(), "4".into());
        host.env.insert("ACME_TIMEOUT".into(), "3".into());
    };
    run_in("things list --max-items 1", "", env);
    assert_eq!(retries(), (4, Some(Duration::from_secs(3))));
    run_in("things list --max-items 1 --no-retry", "", env);
    assert_eq!(retries(), (0, Some(Duration::from_secs(3))));

    let invalid = run_in("things list", "", |host| {
        host.env.insert("ACME_TIMEOUT".into(), "soon".into());
    });
    assert_eq!(invalid.code, exit::USAGE);
    assert_eq!(
        invalid.error()["error"]["message"],
        "ACME_TIMEOUT: expected a number of seconds, got `soon`"
    );
    assert_eq!(run("things list --retries 1 --no-retry").code, exit::USAGE);
    assert_eq!(run("things list --timeout -1").code, exit::USAGE);
}

/// Runs `acme mcp <args>` on these messages, one per line, and returns its responses.
fn mcp(args: &str, messages: &[Value]) -> Vec<Value> {
    let stdin: String = messages.iter().map(|m| format!("{m}\n")).collect();
    let out = run_in(&format!("mcp {args}"), &stdin, |_| {});
    assert_eq!(out.code, exit::OK, "{}", out.stderr);
    out.stdout
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect()
}

fn call(id: u64, tool: &str, arguments: Value) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "method": "tools/call",
           "params": {"name": tool, "arguments": arguments}})
}

#[test]
fn mcp_serves_a_tool_per_operation_after_initialize() {
    let responses = mcp(
        "",
        &[
            json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {
                "protocolVersion": "2025-06-18", "capabilities": {},
                "clientInfo": {"name": "test", "version": "1"}}}),
            json!({"jsonrpc": "2.0", "method": "notifications/initialized"}),
            json!({"jsonrpc": "2.0", "id": 2, "method": "tools/list"}),
            call(
                3,
                "things_list",
                json!({"max_items": 2, "search": "Widget"}),
            ),
            call(
                4,
                "things_create",
                json!({"body": {"name": "Lamp", "quantity": 2}}),
            ),
            call(5, "things_create", json!({"body": {"nmae": "Lamp"}})),
            call(6, "things_create", json!({})),
            call(7, "things_lsit", json!({})),
            json!({"jsonrpc": "2.0", "id": 8, "method": "ping"}),
            json!({"jsonrpc": "2.0", "id": 9, "method": "resources/list"}),
            call(10, "things_list", json!({"max_items": 2})),
            call(11, "things_list", json!({"page": 0, "per_page": 2})),
            call(
                12,
                "things_list",
                json!({"jq": ".[] | .name", "max_items": 3}),
            ),
            call(13, "things_list", json!({"jq": ".[] | .nope("})),
        ],
    );
    let ids: Vec<u64> = responses
        .iter()
        .map(|r| r["id"].as_u64().unwrap())
        .collect();
    assert_eq!(ids, [1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13]);
    let init = &responses[0]["result"];
    assert_eq!(init["protocolVersion"], "2025-06-18");
    assert_eq!(
        init["serverInfo"],
        json!({"name": "acme", "version": "1.0.0"})
    );
    assert_eq!(
        init["capabilities"],
        json!({"tools": {"listChanged": false}})
    );
    assert!(init.get("resultType").is_none());

    let tools = responses[1]["result"]["tools"].as_array().unwrap();
    let names: Vec<&str> = tools.iter().map(|t| t["name"].as_str().unwrap()).collect();
    assert_eq!(names, ["things_list", "things_create"]);
    assert_eq!(tools[0]["annotations"]["readOnlyHint"], true);
    let page = &tools[0]["inputSchema"]["properties"]["page"];
    assert_eq!(
        (&page["type"], page.get("format")),
        (&json!("integer"), None)
    );
    assert!(tools[0]["outputSchema"]["properties"]["items"].is_object());
    let create = &tools[1]["inputSchema"];
    assert_eq!(create["required"], json!(["body"]));
    assert_eq!(create["properties"]["body"]["required"], json!(["name"]));
    let address = &create["properties"]["body"]["properties"]["address"];
    assert_eq!(address["required"], json!(["city"]));
    assert!(!responses[1].to_string().contains("$ref"));

    let listed = &responses[2]["result"];
    assert_eq!(listed["isError"], false);
    let page: Value = serde_json::from_str(listed["content"][0]["text"].as_str().unwrap()).unwrap();
    assert_eq!(
        page,
        json!({"items": [{"id": "th_1", "name": "Widget", "quantity": 10, "tags": []},
                         {"id": "th_5", "name": "Widget XL", "quantity": 50, "tags": []}],
               "has_more": false, "next": null})
    );
    assert_eq!(listed["structuredContent"], page);

    let created = &responses[3]["result"]["structuredContent"];
    assert_eq!(
        (created["name"].clone(), created["quantity"].clone()),
        (json!("Lamp"), json!(2))
    );

    let invalid = &responses[4]["result"];
    assert_eq!(invalid["isError"], true);
    let error: Value =
        serde_json::from_str(invalid["content"][0]["text"].as_str().unwrap()).unwrap();
    assert_eq!(error["error"]["kind"], "usage");
    let paths: Vec<&Value> = error["error"]["errors"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| &e["path"])
        .collect();
    assert_eq!(paths, ["name", "nmae"]);
    let missing = responses[5]["result"]["content"][0]["text"]
        .as_str()
        .unwrap();
    assert!(
        missing.contains("missing required arguments: body"),
        "{missing}"
    );

    assert_eq!(responses[6]["error"]["code"], -32602);
    assert_eq!(
        responses[6]["error"]["message"],
        "Unknown tool: things_lsit: did you mean things_list?"
    );
    assert_eq!(responses[7]["result"], json!({}));
    assert_eq!(responses[8]["error"]["code"], -32601);

    let more = &responses[9]["result"]["structuredContent"];
    assert_eq!(more["items"].as_array().unwrap().len(), 2);
    assert_eq!(
        (&more["has_more"], &more["next"]),
        (&json!(true), &Value::Null)
    );
    assert!(more["hint"].as_str().unwrap().contains("max_items"));
    let first = &responses[10]["result"]["structuredContent"];
    assert_eq!(first["items"].as_array().unwrap().len(), 2);
    assert_eq!(first["next"], json!({"page": 1}));
    let names = &responses[11]["result"]["structuredContent"];
    assert_eq!(
        *names,
        json!({"jq": ["Widget", "Gadget", "Gizmo"], "has_more": true, "next": null,
               "hint": "More items follow: pass a larger `max_items`, or narrow the list with its filters"})
    );
    let bad = &responses[12]["result"];
    assert_eq!(bad["isError"], true);
    let error: Value = serde_json::from_str(bad["content"][0]["text"].as_str().unwrap()).unwrap();
    assert!(
        error["error"]["message"]
            .as_str()
            .unwrap()
            .starts_with("`jq`: invalid filter"),
        "{error}"
    );
    assert!(error["error"].get("status").is_none());
}

#[test]
fn mcp_schemas_and_results_suit_the_client() {
    let init = |client: &str, version: &str| {
        json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {
            "protocolVersion": version, "capabilities": {},
            "clientInfo": {"name": client, "version": "1"}}})
    };
    let list = json!({"jsonrpc": "2.0", "id": 2, "method": "tools/list"});
    let claude = mcp("", &[init("claude-code", "2025-11-25"), list.clone()]);
    assert!(claude[1]["result"]["tools"][1]["outputSchema"].is_object());
    let cursor = mcp(
        "--exclude-tools *_create",
        &[
            init("cursor-vscode", "2025-03-26"),
            list.clone(),
            call(3, "things_list", json!({"max_items": 1})),
        ],
    );
    let tools = cursor[1]["result"]["tools"].as_array().unwrap();
    assert_eq!(tools.len(), 1);
    assert!(tools[0].get("outputSchema").is_none());
    assert!(cursor[2]["result"].get("structuredContent").is_none());
    let forced = mcp(
        "--client openai",
        &[init("claude-code", "2025-11-25"), list],
    );
    assert_eq!(forced[1]["result"]["tools"].as_array().unwrap().len(), 2);
    let out = run_in("mcp --list --tools things_list", "", |_| {});
    let listed: Value = serde_json::from_str(&out.stdout).unwrap();
    assert_eq!(listed[0]["name"], "things_list");
    assert_eq!(listed.as_array().unwrap().len(), 1);
}

#[test]
fn mcp_serves_requests_carrying_their_version() {
    let meta = json!({"io.modelcontextprotocol/protocolVersion": "2026-07-28",
                      "io.modelcontextprotocol/clientCapabilities": {}});
    let responses = mcp(
        "--read-only --tools things_*,plans_*",
        &[
            json!({"jsonrpc": "2.0", "id": "d", "method": "server/discover", "params": {"_meta": meta}}),
            json!({"jsonrpc": "2.0", "id": 2, "method": "tools/list", "params": {"_meta": meta}}),
            json!({"jsonrpc": "2.0", "id": 3, "method": "tools/list", "params": {"_meta": {
                "io.modelcontextprotocol/protocolVersion": "1900-01-01"}}}),
            json!({"jsonrpc": "2.0", "id": 4, "method": "tools/list", "params": {"_meta": {
                "io.modelcontextprotocol/protocolVersion": "2026-07-28"}}}),
            json!({"jsonrpc": "2.0", "id": 5, "method": "tools/call", "params": {
                "_meta": meta, "name": "things_create", "arguments": {"body": {"name": "x"}}}}),
        ],
    );
    let discover = &responses[0]["result"];
    assert_eq!(responses[0]["id"], "d");
    assert_eq!(discover["supportedVersions"][0], "2026-07-28");
    assert_eq!(discover["resultType"], "complete");
    assert_eq!(
        discover["_meta"]["io.modelcontextprotocol/serverInfo"]["name"],
        "acme"
    );
    assert_eq!(
        discover["capabilities"],
        json!({"tools": {"listChanged": false}})
    );

    let listed = &responses[1]["result"];
    assert_eq!(
        (listed["resultType"].clone(), listed["cacheScope"].clone()),
        (json!("complete"), json!("public"))
    );
    let names: Vec<&str> = listed["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, ["things_list"]);

    let unsupported = &responses[2]["error"];
    assert_eq!(unsupported["code"], -32022);
    assert_eq!(unsupported["data"]["requested"], "1900-01-01");
    assert_eq!(unsupported["data"]["supported"][1], "2025-11-25");
    assert_eq!(responses[3]["error"]["code"], -32602);
    assert_eq!(
        responses[4]["error"]["message"],
        "Unknown tool: things_create: this server leaves it out (its toolsets, --toolsets, --tools, --exclude-tools, --read-only)"
    );
}

/// `cli::dispatch`, but `things list --search slow` first sleeps 300ms.
async fn slowly(
    ctx: perseid_cli_runtime::Ctx,
    matches: clap::ArgMatches,
) -> Result<(), perseid_cli_runtime::CliError> {
    let search = matches
        .subcommand_matches("things")
        .and_then(|things| things.subcommand_matches("list"))
        .and_then(|list| list.get_one::<String>("search"));
    if search.is_some_and(|search| search == "slow") {
        tokio::time::sleep(Duration::from_millis(300)).await;
    }
    cli::dispatch(ctx, matches).await
}

#[test]
fn mcp_calls_run_side_by_side_and_cancel() {
    let messages = [
        call(1, "things_list", json!({"search": "slow"})),
        json!({"jsonrpc": "2.0", "id": 2, "method": "ping"}),
        call(3, "things_list", json!({"max_items": 1})),
        call(4, "things_list", json!({"search": "slow"})),
        json!({"jsonrpc": "2.0", "method": "notifications/cancelled",
               "params": {"requestId": 4, "reason": "the user pressed Esc"}}),
        json!({"jsonrpc": "2.0", "id": 5, "method": "ping"}),
    ];
    let stdin: String = messages.iter().map(|m| format!("{m}\n")).collect();
    let (mut host, captured) = Host::captured(stdin);
    host.env
        .insert("ACME_API_KEY".into(), "sk_test_0000".into());
    let started = Instant::now();
    let code = run_with(cli::APP, cli::command(), ["acme", "mcp"], host, slowly);
    let elapsed = started.elapsed();
    assert_eq!(code, exit::OK, "{}", captured.stderr.text());
    let responses: Vec<Value> = captured
        .stdout
        .text()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    let ids: Vec<&Value> = responses.iter().map(|r| &r["id"]).collect();
    assert_eq!(
        ids,
        [2, 3, 5, 1],
        "the slow call answers last, the cancelled one never"
    );
    assert_eq!(responses[3]["result"]["isError"], false);
    assert!(
        elapsed < Duration::from_millis(550),
        "{elapsed:?}: the slow calls ran one after the other"
    );
}

#[test]
fn mcp_answers_malformed_lines_and_ends_with_stdin() {
    let out = run_in("mcp", "not json\n[1]\n\n", |_| {});
    assert_eq!(out.code, exit::OK);
    let lines: Vec<Value> = out
        .stdout
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();
    assert_eq!(lines[0]["error"]["code"], -32700);
    assert_eq!(lines[1]["error"]["code"], -32600);
    assert_eq!(lines.len(), 2);
}

/// The API keys of the calls of `recording` that ran to their end.
#[cfg(feature = "mcp-http")]
static KEYS: Mutex<Vec<String>> = Mutex::new(Vec::new());

/// `slowly`, recording the API key of each call once it is done.
#[cfg(feature = "mcp-http")]
async fn recording(
    mut ctx: perseid_cli_runtime::Ctx,
    matches: clap::ArgMatches,
) -> Result<(), perseid_cli_runtime::CliError> {
    let key = ctx.token().await?;
    let result = slowly(ctx, matches).await;
    KEYS.lock().unwrap().push(key);
    result
}

#[cfg(feature = "mcp-http")]
fn recorded(key: &str) -> bool {
    KEYS.lock().unwrap().iter().any(|recorded| recorded == key)
}

/// `acme mcp --port 0 <args>` on a thread of its own, with ACME_API_KEY=sk_env: its address.
#[cfg(feature = "mcp-http")]
fn mcp_http(args: &str) -> String {
    let args: Vec<String> = ["acme", "mcp", "--port", "0"]
        .into_iter()
        .chain(args.split_whitespace())
        .map(str::to_owned)
        .collect();
    let (sender, receiver) = mpsc::channel();
    thread::spawn(move || {
        let (mut host, captured) = Host::captured("");
        host.env.insert("ACME_API_KEY".into(), "sk_env".into());
        sender.send(captured).unwrap();
        run_with(cli::APP, cli::command(), args, host, recording)
    });
    let captured = receiver.recv().unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let stderr = captured.stderr.text();
        let line = stderr.split("serving http://").nth(1);
        if let Some((address, _)) = line.and_then(|line| line.split_once("/mcp\n")) {
            return address.to_owned();
        }
        assert!(
            Instant::now() < deadline,
            "the server did not start: {stderr}"
        );
        thread::sleep(Duration::from_millis(10));
    }
}

/// An HTTP response: its status, lowercase headers and body.
#[cfg(feature = "mcp-http")]
struct HttpReply {
    status: u16,
    headers: HashMap<String, String>,
    body: String,
}

#[cfg(feature = "mcp-http")]
impl HttpReply {
    fn json(&self) -> Value {
        serde_json::from_str(&self.body).unwrap_or_else(|e| panic!("{e}: {}", self.body))
    }
}

/// Sends one request on a connection of its own and reads the response.
#[cfg(feature = "mcp-http")]
fn http(address: &str, request: &str, headers: &[(&str, &str)], body: &str) -> HttpReply {
    let mut stream = TcpStream::connect(address).unwrap();
    let mut head = format!(
        "{request} HTTP/1.1\r\nhost: {address}\r\nconnection: close\r\ncontent-length: {}\r\n",
        body.len()
    );
    for (name, value) in headers {
        head.push_str(&format!("{name}: {value}\r\n"));
    }
    write!(stream, "{head}\r\n{body}").unwrap();
    let mut response = String::new();
    stream.read_to_string(&mut response).unwrap();
    let (head, body) = response.split_once("\r\n\r\n").unwrap();
    let mut lines = head.lines();
    let status = lines
        .next()
        .unwrap()
        .split(' ')
        .nth(1)
        .unwrap()
        .parse()
        .unwrap();
    let headers = lines
        .filter_map(|line| line.split_once(": "))
        .map(|(name, value)| (name.to_lowercase(), value.to_owned()))
        .collect();
    HttpReply {
        status,
        headers,
        body: body.to_owned(),
    }
}

/// `POST /mcp` of a JSON-RPC message, with `Authorization: Bearer <key>` when given.
#[cfg(feature = "mcp-http")]
fn post(address: &str, key: Option<&str>, message: &Value) -> HttpReply {
    let bearer = key.map(|key| format!("Bearer {key}"));
    let mut headers = vec![("content-type", "application/json")];
    headers.extend(bearer.as_deref().map(|bearer| ("authorization", bearer)));
    http(address, "POST /mcp", &headers, &message.to_string())
}

#[cfg(feature = "mcp-http")]
#[test]
fn mcp_over_http_serves_each_request_with_its_own_key() {
    let address = mcp_http("");
    let health = http(&address, "GET /health", &[], "");
    assert_eq!(health.status, 200);
    assert_eq!(health.json(), json!({"status": "ok", "version": "1.0.0"}));

    let init = json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {
        "protocolVersion": "2025-06-18", "capabilities": {},
        "clientInfo": {"name": "claude-code", "version": "1"}}});
    let initialized = post(&address, Some("sk_http_a"), &init);
    assert_eq!(initialized.status, 200);
    assert_eq!(initialized.headers["content-type"], "application/json");
    assert_eq!(
        initialized.json()["result"]["protocolVersion"],
        "2025-06-18"
    );

    let list = json!({"jsonrpc": "2.0", "id": 2, "method": "tools/list"});
    let listed = post(&address, Some("sk_http_a"), &list).json();
    let tools = listed["result"]["tools"].as_array().unwrap();
    let names: Vec<&str> = tools.iter().map(|t| t["name"].as_str().unwrap()).collect();
    assert_eq!(names, ["things_list", "things_create"]);
    assert!(
        !listed.to_string().contains("$ref"),
        "each request is on its own: no client named, schemas inlined"
    );

    let called = post(
        &address,
        Some("sk_http_a"),
        &call(3, "things_list", json!({"max_items": 1})),
    );
    assert_eq!(called.status, 200);
    let result = &called.json()["result"];
    assert_eq!(result["isError"], false);
    assert_eq!(result["structuredContent"]["items"][0]["id"], "th_1");
    assert!(recorded("sk_http_a"));

    let message = call(4, "things_list", json!({"max_items": 1})).to_string();
    let headers = [
        ("content-type", "application/json"),
        ("x-api-key", "sk_http_b"),
    ];
    let named = http(&address, "POST /mcp", &headers, &message);
    assert_eq!(named.status, 200, "{}", named.body);
    assert!(
        recorded("sk_http_b"),
        "the API's own header carries a key too"
    );

    let keyless = post(&address, None, &call(5, "things_list", json!({})));
    assert_eq!(keyless.status, 401);
    assert_eq!(keyless.headers["www-authenticate"], "Bearer realm=\"mcp\"");
    let error = keyless.json();
    assert_eq!(error["error"]["code"], -32600);
    assert!(error["error"]["message"]
        .as_str()
        .unwrap()
        .contains("Authorization: Bearer"));
    assert!(!recorded("sk_env"), "the server's key serves no request");

    let notification = json!({"jsonrpc": "2.0", "method": "notifications/initialized"});
    let accepted = post(&address, Some("sk_http_a"), &notification);
    assert_eq!((accepted.status, accepted.body.as_str()), (202, ""));

    let get = http(&address, "GET /mcp", &[], "");
    assert_eq!((get.status, get.headers["allow"].as_str()), (405, "POST"));
    assert_eq!(http(&address, "GET /nope", &[], "").status, 404);

    let ping = json!({"jsonrpc": "2.0", "id": 6, "method": "ping"}).to_string();
    let from = |origin: &'static str| {
        let headers = [
            ("content-type", "application/json"),
            ("authorization", "Bearer sk_http_a"),
            ("origin", origin),
        ];
        http(&address, "POST /mcp", &headers, &ping).status
    };
    assert_eq!(from("https://evil.example"), 403);
    assert_eq!(from("http://localhost:6274"), 200);

    let versioned = |version: &'static str| {
        let headers = [
            ("content-type", "application/json"),
            ("authorization", "Bearer sk_http_a"),
            ("mcp-protocol-version", version),
        ];
        let message = call(7, "things_list", json!({"max_items": 1})).to_string();
        http(&address, "POST /mcp", &headers, &message)
    };
    assert_eq!(versioned("1999-01-01").status, 400);
    let legacy = versioned("2025-03-26").json();
    assert!(legacy["result"].get("structuredContent").is_none());

    let malformed = http(
        &address,
        "POST /mcp",
        &[("authorization", "Bearer sk_http_a")],
        "not json",
    );
    assert_eq!(malformed.status, 400);
    assert_eq!(malformed.json()["error"]["code"], -32700);
}

#[cfg(feature = "mcp-http")]
#[test]
fn mcp_over_http_serves_requests_carrying_their_version() {
    let address = mcp_http("");
    let meta = json!({"io.modelcontextprotocol/protocolVersion": "2026-07-28",
                      "io.modelcontextprotocol/clientInfo": {"name": "claude-code", "version": "1"},
                      "io.modelcontextprotocol/clientCapabilities": {}});
    let modern = |method: &str, name: Option<&str>, params: Value| {
        let mut params = params;
        params["_meta"] = meta.clone();
        let message = json!({"jsonrpc": "2.0", "id": 1, "method": method, "params": params});
        let mut headers = vec![
            ("content-type", "application/json"),
            ("authorization", "Bearer sk_modern"),
            ("mcp-protocol-version", "2026-07-28"),
            ("mcp-method", method),
        ];
        headers.extend(name.map(|name| ("mcp-name", name)));
        http(&address, "POST /mcp", &headers, &message.to_string())
    };
    let discover = modern("server/discover", None, json!({}));
    assert_eq!(discover.status, 200, "{}", discover.body);
    let discovered = &discover.json()["result"];
    assert_eq!(discovered["supportedVersions"][0], "2026-07-28");
    assert_eq!(discovered["resultType"], "complete");

    let listed = modern("tools/list", None, json!({})).json();
    assert_eq!(listed["result"]["ttlMs"], 3_600_000);
    assert_eq!(listed["result"]["cacheScope"], "public");
    let again = modern("tools/list", None, json!({})).json();
    assert_eq!(listed, again, "the list is the same every time");

    let arguments = json!({"name": "things_list", "arguments": {"max_items": 1}});
    let called = modern("tools/call", Some("things_list"), arguments.clone());
    assert_eq!(called.status, 200, "{}", called.body);
    let result = &called.json()["result"];
    assert_eq!(result["resultType"], "complete");
    assert_eq!(result["structuredContent"]["items"][0]["id"], "th_1");
    assert!(recorded("sk_modern"));

    let mismatched = modern("tools/call", Some("things_create"), arguments.clone());
    assert_eq!(mismatched.status, 400);
    let error = mismatched.json();
    assert_eq!(
        (&error["id"], &error["error"]["code"]),
        (&json!(1), &json!(-32020))
    );
    let unnamed = modern("tools/call", None, arguments);
    assert_eq!(unnamed.json()["error"]["code"], -32020);

    let unknown = modern("resources/list", None, json!({}));
    assert_eq!(unknown.status, 404);
    assert_eq!(unknown.json()["error"]["code"], -32601);

    let message = json!({"jsonrpc": "2.0", "id": 2, "method": "tools/list", "params": {"_meta": {
        "io.modelcontextprotocol/protocolVersion": "2026-07-28"}}});
    let headers = [
        ("content-type", "application/json"),
        ("authorization", "Bearer sk_modern"),
        ("mcp-protocol-version", "2026-07-28"),
        ("mcp-method", "tools/list"),
    ];
    let incomplete = http(&address, "POST /mcp", &headers, &message.to_string());
    assert_eq!(incomplete.status, 400);
    assert_eq!(incomplete.json()["error"]["code"], -32602);
    let headers = [
        headers[0],
        headers[1],
        ("mcp-protocol-version", "1900-01-01"),
    ];
    let unsupported = http(&address, "POST /mcp", &headers, &message.to_string()).json();
    assert_eq!(unsupported["error"]["code"], -32022);
    assert_eq!(unsupported["error"]["data"]["supported"][0], "2026-07-28");
}

#[cfg(feature = "mcp-http")]
#[test]
fn mcp_over_http_uses_the_servers_key_only_when_told_to() {
    let address = mcp_http("--key-from-env --read-only");
    let listed = post(
        &address,
        None,
        &call(1, "things_list", json!({"max_items": 1})),
    );
    assert_eq!(listed.status, 200);
    assert_eq!(listed.json()["result"]["isError"], false);
    assert!(recorded("sk_env"));

    let own = post(
        &address,
        Some("sk_http_c"),
        &call(2, "things_list", json!({"max_items": 1})),
    );
    assert_eq!(own.json()["result"]["isError"], false);
    assert!(recorded("sk_http_c"), "a request's own key wins");

    let refused = post(
        &address,
        None,
        &call(3, "things_create", json!({"body": {"name": "Lamp"}})),
    );
    assert_eq!(refused.status, 200);
    assert_eq!(
        refused.json()["error"]["message"],
        "Unknown tool: things_create: this server leaves it out (its toolsets, --toolsets, --tools, --exclude-tools, --read-only)"
    );
}

#[cfg(feature = "mcp-http")]
#[test]
fn mcp_over_http_runs_calls_side_by_side_and_drops_those_of_clients_gone() {
    let address = mcp_http("");
    let slow = |key: &'static str| {
        let address = address.clone();
        thread::spawn(move || {
            let message = call(1, "things_list", json!({"search": "slow"}));
            post(&address, Some(key), &message).json()
        })
    };
    let started = Instant::now();
    let (first, second) = (slow("sk_side_1"), slow("sk_side_2"));
    let (first, second) = (first.join().unwrap(), second.join().unwrap());
    let elapsed = started.elapsed();
    assert_eq!(first["result"]["isError"], false);
    assert_eq!(second["result"]["isError"], false);
    assert!(
        elapsed < Duration::from_millis(550),
        "{elapsed:?}: the slow calls ran one after the other"
    );

    let message = call(2, "things_list", json!({"search": "slow"})).to_string();
    let mut stream = TcpStream::connect(&address).unwrap();
    write!(
        stream,
        "POST /mcp HTTP/1.1\r\nhost: {address}\r\ncontent-type: application/json\r\n\
         authorization: Bearer sk_gone\r\ncontent-length: {}\r\n\r\n{message}",
        message.len()
    )
    .unwrap();
    thread::sleep(Duration::from_millis(50));
    drop(stream);
    thread::sleep(Duration::from_millis(500));
    assert!(!recorded("sk_gone"), "the call of a client gone runs on");
}

/// One request to the fake OAuth server: its path and form.
#[derive(Clone, Debug)]
struct Request {
    path: String,
    form: HashMap<String, String>,
}

type Responder = dyn Fn(&Request, usize) -> (u16, Value) + Send;

/// An OAuth server on 127.0.0.1, answering each request with `respond(request, n)`, `n`
/// counting the earlier requests to the same path.
struct OAuthServer {
    url: String,
    requests: Arc<Mutex<Vec<Request>>>,
}

impl OAuthServer {
    fn start(respond: impl Fn(&Request, usize) -> (u16, Value) + Send + 'static) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let requests = Arc::new(Mutex::new(Vec::new()));
        let log = requests.clone();
        let respond: Box<Responder> = Box::new(respond);
        thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(stream) = stream else { continue };
                let request = read_request(&stream);
                let seen = log
                    .lock()
                    .unwrap()
                    .iter()
                    .filter(|r: &&Request| r.path == request.path)
                    .count();
                log.lock().unwrap().push(request.clone());
                let (status, body) = respond(&request, seen);
                let body = body.to_string();
                let _ = write!(
                    &stream,
                    "HTTP/1.1 {status} X\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
            }
        });
        Self { url, requests }
    }

    fn requests(&self, path: &str) -> Vec<HashMap<String, String>> {
        let requests = self.requests.lock().unwrap();
        let matching = requests.iter().filter(|r| r.path == path);
        matching.map(|r| r.form.clone()).collect()
    }

    /// The example's login methods, against this server.
    fn app(&self) -> App {
        Self::app_at(&self.url)
    }

    /// The example's login methods with URLs relative to the base URL.
    fn relative_app() -> App {
        Self::app_at("")
    }

    fn app_at(prefix: &str) -> App {
        let at = |path: &str| -> &'static str { Box::leak(format!("{prefix}{path}").into()) };
        let client = oauth::Client::new("acme-cli", at("/token"))
            .scopes(&["things"])
            .revocation_url(at("/revoke"));
        let browser: &'static Browser = Box::leak(Box::new(Browser::new(client, at("/authorize"))));
        let device = Box::leak(Box::new(DeviceCode::new(client, at("/device"))));
        let methods = vec![
            Method::new("web", "", browser),
            Method::new("device", "", device),
            Method::new("web-key", "", Box::leak(Box::new(cli::WebKey(browser)))),
        ];
        let auth = Box::leak(Box::new(Methods {
            default: &ApiKey,
            methods: methods.leak(),
        }));
        App { auth, ..cli::APP }
    }
}

fn read_request(stream: &TcpStream) -> Request {
    let mut reader = BufReader::new(stream);
    let mut line = String::new();
    reader.read_line(&mut line).unwrap();
    let path = line.split_whitespace().nth(1).unwrap_or("/").to_owned();
    let mut length = 0;
    loop {
        let mut header = String::new();
        reader.read_line(&mut header).unwrap();
        if header.trim().is_empty() {
            break;
        }
        if let Some((name, value)) = header.split_once(':') {
            if name.eq_ignore_ascii_case("content-length") {
                length = value.trim().parse().unwrap();
            }
        }
    }
    let mut body = vec![0; length];
    reader.read_exact(&mut body).unwrap();
    let form = url::form_urlencoded::parse(&body).into_owned().collect();
    Request { path, form }
}

fn tokens(access: &str, refresh: &str) -> (u16, Value) {
    let body = json!({
        "access_token": access, "token_type": "Bearer", "expires_in": 3600,
        "refresh_token": refresh, "scope": "things"
    });
    (200, body)
}

fn config(dir: &Path) -> toml::Table {
    toml::from_str(&fs::read_to_string(dir.join("acme/config.toml")).unwrap()).unwrap()
}

fn profile(dir: &Path) -> toml::Table {
    config(dir)["profiles"]["default"]
        .as_table()
        .unwrap()
        .clone()
}

#[test]
fn device_code_login_polls_until_approved_then_logout_revokes() {
    let server = OAuthServer::start(|request, n| match request.path.as_str() {
        "/device" => (
            200,
            json!({
                "device_code": "dc_1", "user_code": "ABCD-EFGH",
                "verification_uri": "https://acme.dev/activate",
                "expires_in": 600, "interval": 0
            }),
        ),
        "/token" if n == 0 => (400, json!({"error": "authorization_pending"})),
        "/token" => tokens("at_device", "rt_device"),
        _ => (200, json!({})),
    });
    let app = server.app();
    let dir = tempfile::tempdir().unwrap();

    let login = run_app(app, "auth login --device", "", with_config(dir.path()));
    assert_eq!(login.code, exit::OK, "{}", login.stderr);
    let next = login.json();
    assert_eq!(
        (
            next["verification_uri"].clone(),
            next["user_code"].clone(),
            next["expires_in"].clone()
        ),
        (
            json!("https://acme.dev/activate"),
            json!("ABCD-EFGH"),
            json!(600)
        )
    );
    assert!(next["next_step"]
        .as_str()
        .unwrap()
        .contains("waits for the approval"));
    assert!(
        login
            .stderr
            .starts_with("Open https://acme.dev/activate and enter the code ABCD-EFGH\n"),
        "{}",
        login.stderr
    );
    assert_eq!(
        server.requests("/device"),
        [HashMap::from([
            ("client_id".into(), "acme-cli".into()),
            ("scope".into(), "things".into())
        ])]
    );
    let polls = server.requests("/token");
    assert_eq!(polls.len(), 2);
    assert_eq!(
        polls[1]["grant_type"],
        "urn:ietf:params:oauth:grant-type:device_code"
    );
    assert_eq!(polls[1]["device_code"], "dc_1");
    let stored = profile(dir.path());
    assert_eq!(stored["access_token"].as_str(), Some("at_device"));
    assert_eq!(stored["refresh_token"].as_str(), Some("rt_device"));
    assert_eq!(stored["method"].as_str(), Some("device"));
    assert_eq!(stored["scopes"].as_array().unwrap().len(), 1);

    let listed = run_app(
        app,
        "things list --max-items 1",
        "",
        with_config(dir.path()),
    );
    assert_eq!(listed.code, exit::OK, "{}", listed.stderr);
    assert_eq!(LAST_TOKEN.with(|t| t.borrow().clone()), "at_device");
    assert_eq!(server.requests("/token").len(), 2);

    let profiles = run_app(app, "auth profiles", "", with_config(dir.path()));
    let status = profiles.json()[0]["auth"].as_str().unwrap().to_owned();
    assert!(status.starts_with("oauth, expires in "), "{status}");
    assert!(status.ends_with(", refreshable (--device)"), "{status}");

    let logout = run_app(app, "auth logout", "", with_config(dir.path()));
    assert_eq!(logout.code, exit::OK, "{}", logout.stderr);
    let revoked = server.requests("/revoke");
    assert_eq!(revoked[0]["token"], "rt_device");
    assert_eq!(revoked[0]["token_type_hint"], "refresh_token");
    assert!(config(dir.path()).get("profiles").is_none());
}

#[test]
fn device_code_login_fails_when_denied() {
    let server = OAuthServer::start(|request, _| match request.path.as_str() {
        "/device" => (
            200,
            json!({"device_code": "dc", "user_code": "X", "verification_uri": "https://a", "interval": 0}),
        ),
        _ => (400, json!({"error": "access_denied"})),
    });
    let dir = tempfile::tempdir().unwrap();
    let out = run_app(
        server.app(),
        "auth login --device",
        "",
        with_config(dir.path()),
    );
    assert_eq!(out.code, exit::CONFIG);
    assert_eq!(
        out.error()["error"]["message"],
        "the login was denied: run `acme auth login` again"
    );
    assert!(!dir.path().join("acme/config.toml").exists());
}

fn write_config(dir: &Path, profile: &str) {
    fs::create_dir_all(dir.join("acme")).unwrap();
    fs::write(dir.join("acme/config.toml"), profile).unwrap();
}

#[test]
fn expired_tokens_are_refreshed_and_saved() {
    let server = OAuthServer::start(|request, _| match request.form["refresh_token"].as_str() {
        "rt_old" => tokens("at_new", "rt_new"),
        _ => (400, json!({"error": "invalid_grant"})),
    });
    let app = server.app();
    let dir = tempfile::tempdir().unwrap();
    write_config(
        dir.path(),
        "[profiles.default]\naccess_token = \"at_old\"\nrefresh_token = \"rt_old\"\nexpires_at = 1\nmethod = \"web\"\norg = \"acme\"\n",
    );

    let out = run_app(
        app,
        "things list --max-items 1",
        "",
        with_config(dir.path()),
    );
    assert_eq!(out.code, exit::OK, "{}", out.stderr);
    assert_eq!(LAST_TOKEN.with(|t| t.borrow().clone()), "at_new");
    let refresh = &server.requests("/token")[0];
    assert_eq!(refresh["grant_type"], "refresh_token");
    assert_eq!(refresh["client_id"], "acme-cli");
    let stored = profile(dir.path());
    assert_eq!(stored["access_token"].as_str(), Some("at_new"));
    assert_eq!(stored["refresh_token"].as_str(), Some("rt_new"));
    assert!(stored["expires_at"].as_integer().unwrap() > 1);
    assert_eq!(stored["method"].as_str(), Some("web"));
    assert_eq!(stored["org"].as_str(), Some("acme"));

    let again = run_app(
        app,
        "things list --max-items 1",
        "",
        with_config(dir.path()),
    );
    assert_eq!(again.code, exit::OK, "{}", again.stderr);
    assert_eq!(server.requests("/token").len(), 1);

    let flag = run_app(
        app,
        "things list --max-items 1 --api-key sk_flag",
        "",
        with_config(dir.path()),
    );
    assert_eq!(flag.code, exit::OK, "{}", flag.stderr);
    assert_eq!(LAST_TOKEN.with(|t| t.borrow().clone()), "sk_flag");

    write_config(
        dir.path(),
        "[profiles.default]\naccess_token = \"at\"\nrefresh_token = \"rt_revoked\"\nexpires_at = 1\nmethod = \"web\"\n",
    );
    let out = run_app(app, "things list", "", with_config(dir.path()));
    assert_eq!(out.code, exit::CONFIG);
    assert_eq!(
        out.error()["error"]["message"],
        "the login of profile `default` expired and could not be refreshed: run `acme auth login`"
    );
}

#[test]
fn concurrent_runs_refresh_an_expired_token_once() {
    let server = OAuthServer::start(|request, _| {
        thread::sleep(Duration::from_millis(200));
        match request.form["refresh_token"].as_str() {
            "rt_old" => tokens("at_new", "rt_new"),
            _ => (400, json!({"error": "invalid_grant"})),
        }
    });
    let app = server.app();
    let dir = tempfile::tempdir().unwrap();
    write_config(
        dir.path(),
        "[profiles.default]\naccess_token = \"at_old\"\nrefresh_token = \"rt_old\"\nexpires_at = 1\nmethod = \"web\"\n",
    );
    let runs: Vec<_> = (0..4)
        .map(|_| {
            let dir = dir.path().to_owned();
            thread::spawn(move || {
                let out = run_app(app, "things list --max-items 1", "", with_config(&dir));
                (
                    out.code,
                    out.stderr,
                    LAST_TOKEN.with(|t| t.borrow().clone()),
                )
            })
        })
        .collect();
    for run in runs {
        let (code, stderr, token) = run.join().unwrap();
        assert_eq!(code, exit::OK, "{stderr}");
        assert_eq!(token, "at_new");
    }
    assert_eq!(server.requests("/token").len(), 1);
    assert_eq!(
        profile(dir.path())["refresh_token"].as_str(),
        Some("rt_new")
    );
}

#[test]
fn unexpired_tokens_serve_when_they_cannot_be_refreshed() {
    let closed = TcpListener::bind("127.0.0.1:0").unwrap();
    let base_url = format!("http://{}", closed.local_addr().unwrap());
    drop(closed);
    let dir = tempfile::tempdir().unwrap();
    let soon = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs()
        + 30;
    let list = |profile: &str| {
        write_config(dir.path(), profile);
        let args = format!("things list --max-items 1 --debug --base-url {base_url}");
        let out = run_app(
            OAuthServer::relative_app(),
            &args,
            "",
            with_config(dir.path()),
        );
        (out, LAST_TOKEN.with(|t| t.borrow().clone()))
    };

    let (out, token) = list(&format!(
        "[profiles.default]\naccess_token = \"at_soon\"\nexpires_at = {soon}\nmethod = \"web\"\n"
    ));
    assert_eq!(out.code, exit::OK, "{}", out.stderr);
    assert_eq!(token, "at_soon");

    let (out, token) = list(&format!(
        "[profiles.default]\naccess_token = \"at_soon\"\nrefresh_token = \"rt\"\nexpires_at = {soon}\nmethod = \"web\"\n"
    ));
    assert_eq!(out.code, exit::OK, "{}", out.stderr);
    assert_eq!(token, "at_soon");
    assert!(
        out.stderr
            .contains("debug: refresh failed, using the unexpired token: "),
        "{}",
        out.stderr
    );

    let (out, _) =
        list("[profiles.default]\naccess_token = \"at\"\nexpires_at = 1\nmethod = \"web\"\n");
    assert_eq!(out.code, exit::CONFIG);
}

#[test]
fn relative_oauth_urls_follow_the_base_url() {
    let server = OAuthServer::start(|request, _| match request.path.as_str() {
        "/device" => (
            200,
            json!({"device_code": "dc", "user_code": "X", "verification_uri": "https://a"}),
        ),
        "/token" => tokens("at_rel", "rt_rel"),
        _ => (200, json!({})),
    });
    let app = OAuthServer::relative_app();
    let dir = tempfile::tempdir().unwrap();
    let args = format!("auth login --device --base-url {}/api/v1", server.url);
    let login = run_app(app, &args, "", with_config(dir.path()));
    assert_eq!(login.code, exit::OK, "{}", login.stderr);
    assert_eq!(server.requests("/device").len(), 1);
    assert_eq!(profile(dir.path())["access_token"].as_str(), Some("at_rel"));

    let logout = run_app(app, "auth logout", "", with_config(dir.path()));
    assert_eq!(logout.code, exit::OK, "{}", logout.stderr);
    assert_eq!(server.requests("/revoke")[0]["token"], "rt_rel");
    assert_eq!(server.requests("/token").len(), 1);
}

/// Runs `auth login <flag>` on another thread and plays the browser: waits for the
/// authorization URL on stderr, then requests the redirect `redirect(url)` returns.
fn browser_login(
    app: App,
    flag: &str,
    dir: &Path,
    redirect: impl FnOnce(&Url) -> String,
) -> (Run, Url, String) {
    browser_login_after(false, app, flag, dir, redirect)
}

/// [`browser_login`], with an `idle` connection to the loopback server opened first.
fn browser_login_after(
    idle: bool,
    app: App,
    flag: &str,
    dir: &Path,
    redirect: impl FnOnce(&Url) -> String,
) -> (Run, Url, String) {
    let (sender, receiver) = mpsc::channel();
    let args = format!("auth login {flag}");
    let config_dir = PathBuf::from(dir);
    let cli = thread::spawn(move || {
        let (mut host, captured) = Host::captured("");
        host.config_dir = Some(config_dir);
        host.env.insert("ACME_NO_KEYRING".into(), "1".into());
        sender.send(captured.clone()).unwrap();
        let argv = std::iter::once("acme").chain(args.split_whitespace());
        let code = run_with(app, cli::command(), argv, host, cli::dispatch);
        (code, LAST_TOKEN.with(|t| t.borrow().clone()))
    });
    let captured: Captured = receiver.recv().unwrap();
    let started = Instant::now();
    let url = loop {
        let stderr = captured.stderr.text();
        if let Some(line) = stderr.lines().find(|l| l.contains("/authorize?")) {
            break Url::parse(line.trim()).unwrap();
        }
        assert!(started.elapsed() < Duration::from_secs(10), "{stderr}");
        thread::sleep(Duration::from_millis(10));
    };
    let redirect_uri = Url::parse(&param(&url, "redirect_uri")).unwrap();
    let _idle =
        idle.then(|| TcpStream::connect(redirect_uri.socket_addrs(|| None).unwrap()[0]).unwrap());
    let target = redirect(&url);
    let mut browser = TcpStream::connect(redirect_uri.socket_addrs(|| None).unwrap()[0]).unwrap();
    write!(browser, "GET {target} HTTP/1.1\r\nHost: 127.0.0.1\r\n\r\n").unwrap();
    let mut page = String::new();
    browser.read_to_string(&mut page).unwrap();
    let (code, token) = cli.join().unwrap();
    LAST_TOKEN.with(|t| t.replace(token));
    let run = Run {
        code,
        stdout: captured.stdout.text(),
        stderr: captured.stderr.text(),
    };
    (run, url, page)
}

fn param(url: &Url, name: &str) -> String {
    let mut pairs = url.query_pairs();
    pairs.find(|(key, _)| key == name).unwrap().1.into_owned()
}

fn web_server() -> OAuthServer {
    OAuthServer::start(
        |request, _| match request.form.get("code").map(String::as_str) {
            Some("code_1") => tokens("at_web", "rt_web"),
            _ => (400, json!({"error": "invalid_grant"})),
        },
    )
}

#[test]
fn browser_login_uses_pkce_and_a_loopback_redirect() {
    let server = web_server();
    let dir = tempfile::tempdir().unwrap();
    let (out, url, page) = browser_login(server.app(), "--web", dir.path(), |url| {
        format!("/callback?code=code_1&state={}", param(url, "state"))
    });
    assert_eq!(out.code, exit::OK, "{}", out.stderr);
    assert!(page.starts_with("HTTP/1.1 200 OK"), "{page}");
    assert_eq!(url.path(), "/authorize");
    assert_eq!(param(&url, "response_type"), "code");
    assert_eq!(param(&url, "client_id"), "acme-cli");
    assert_eq!(param(&url, "code_challenge_method"), "S256");
    assert_eq!(param(&url, "scope"), "things");
    let redirect_uri = param(&url, "redirect_uri");
    assert!(
        redirect_uri.starts_with("http://127.0.0.1:"),
        "{redirect_uri}"
    );
    assert!(redirect_uri.ends_with("/callback"), "{redirect_uri}");

    let exchange = &server.requests("/token")[0];
    assert_eq!(exchange["grant_type"], "authorization_code");
    assert_eq!(exchange["redirect_uri"], redirect_uri);
    assert_eq!(
        oauth::pkce_challenge(&exchange["code_verifier"]),
        param(&url, "code_challenge")
    );
    let stored = profile(dir.path());
    assert_eq!(stored["access_token"].as_str(), Some("at_web"));
    assert_eq!(stored["method"].as_str(), Some("web"));
    assert!(out
        .stderr
        .contains("Logged in to profile `default` (oauth, expires in "));
}

#[test]
fn an_idle_connection_does_not_hold_up_the_redirect() {
    let server = web_server();
    let dir = tempfile::tempdir().unwrap();
    let started = Instant::now();
    let (out, _, page) = browser_login_after(true, server.app(), "--web", dir.path(), |url| {
        format!("/callback?code=code_1&state={}", param(url, "state"))
    });
    assert_eq!(out.code, exit::OK, "{}", out.stderr);
    assert!(page.starts_with("HTTP/1.1 200 OK"), "{page}");
    assert!(started.elapsed() < Duration::from_secs(5));
}

#[test]
fn browser_login_rejects_a_forged_state() {
    let server = web_server();
    let dir = tempfile::tempdir().unwrap();
    let (out, _, page) = browser_login(server.app(), "--web", dir.path(), |_| {
        "/callback?code=code_1&state=forged".into()
    });
    assert_eq!(out.code, exit::CONFIG);
    assert!(page.starts_with("HTTP/1.1 400"), "{page}");
    assert_eq!(
        out.error()["error"]["message"],
        "login failed: the redirect's state does not match"
    );
    assert!(server.requests("/token").is_empty());
    assert!(!dir.path().join("acme/config.toml").exists());
}

#[test]
fn a_custom_login_can_mint_an_api_key() {
    let server = web_server();
    let dir = tempfile::tempdir().unwrap();
    let (out, _, _) = browser_login(server.app(), "--web-key", dir.path(), |url| {
        format!("/callback?state={}&code=code_1", param(url, "state"))
    });
    assert_eq!(out.code, exit::OK, "{}", out.stderr);
    assert_eq!(
        fs::read_to_string(dir.path().join("acme/config.toml")).unwrap(),
        "default = \"default\"\n\n[profiles.default]\napi_key = \"sk_minted_at_web\"\nmethod = \"web-key\"\n"
    );
    let listed = run_app(
        server.app(),
        "things list --max-items 1",
        "",
        with_config(dir.path()),
    );
    assert_eq!(listed.code, exit::OK, "{}", listed.stderr);
    assert_eq!(LAST_TOKEN.with(|t| t.borrow().clone()), "sk_minted_at_web");
}

#[test]
fn login_flags_join_auth_login() {
    let help = run("auth login --help");
    for flag in ["--web", "--device", "--web-key"] {
        assert!(help.stdout.contains(flag), "{}", help.stdout);
    }
    let both = run("auth login --web --device");
    assert_eq!(both.code, exit::USAGE);
}
