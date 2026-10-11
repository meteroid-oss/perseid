//! The @@CLIENT_NAME@@ CLI: the commands perseid generates in `generated/`, and yours.
use std::process::ExitCode;

use generated::{
    clap::{ArgMatches, Command},
    runtime::{
        app::{self, App, Ctx},
        CliError,
    },
};

/// How `auth login` logs in and commands get a token: `generated::AUTH` follows the spec's
/// security schemes; `&generated::runtime::auth::ApiKey` or your own `Auth` replace it.
const APP: App = App {
    auth: &generated::AUTH,
    ..generated::APP
};

fn main() -> ExitCode {
    app::run(APP, command(), dispatch)
}

fn command() -> Command {
    // Mount your commands here, `.subcommand(listen::command())`: see README.md.
    let root = generated::command().version(env!("CARGO_PKG_VERSION"));
    app::command(&APP, root)
}

async fn dispatch(mut ctx: Ctx, matches: ArgMatches) -> Result<(), CliError> {
    // Run yours before the generated ones: `if let Some(("listen", m)) = matches.subcommand()`.
    generated::dispatch(&mut ctx, &matches).await
}

#[cfg(test)]
mod tests {
    #[test]
    fn the_command_tree_is_valid() {
        super::command().debug_assert();
    }
}
