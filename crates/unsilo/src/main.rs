//! Thin entry point. Every decision lives in the library so tests can reach it
//! without spawning a process.

use clap::Parser as _;
use std::io::IsTerminal as _;
use unsilo::cli::{Cli, Command, Format};
use unsilo::{Env, Error, Result, ops, report};

fn main() -> std::process::ExitCode {
    match run() {
        Ok(code) => code,
        Err(e) => {
            eprintln!("unsilo: {e}");
            std::process::ExitCode::from(u8::try_from(e.exit_code()).unwrap_or(1))
        }
    }
}

/// Printed through anstream so a console that needs the escape codes translated
/// gets that, and so anything left over is stripped when stdout is not a
/// terminal. The decision not to colour is already made before this point; this
/// is the backstop.
fn emit<T: serde::Serialize>(
    json: bool,
    value: &T,
    human: impl FnOnce() -> String,
) -> Result<(), Error> {
    if json {
        let rendered = serde_json::to_string_pretty(value).map_err(|e| Error::json("stdout", e))?;
        println!("{rendered}");
    } else {
        anstream::print!("{}", human());
    }
    Ok(())
}

/// A dry run prints its plan and then says, through the exit code, that there is
/// work waiting. Useful as `unsilo apply --dry-run || unsilo apply` in a hook.
fn pending(dry_run: bool, changes: usize) -> Result<std::process::ExitCode, Error> {
    if dry_run && changes > 0 {
        return Err(Error::DryRunPending(changes));
    }
    Ok(std::process::ExitCode::SUCCESS)
}

fn label(
    env: &Env,
    st: unsilo::style::Style,
    json: bool,
    id: Option<String>,
    name: Option<String>,
    learn: bool,
    list: bool,
) -> Result<std::process::ExitCode, Error> {
    if list {
        let listing = ops::label::list(env)?;
        emit(json, &listing, || report::labels(st, &listing))?;
        return Ok(std::process::ExitCode::SUCCESS);
    }
    if learn {
        let learned = ops::label::learn(env)?;
        emit(json, &learned, || report::learned(st, &learned))?;
        return Ok(std::process::ExitCode::SUCCESS);
    }
    let (Some(id), Some(name)) = (id, name) else {
        return Err(Error::Usage("label needs an id and a name, or --learn, or --list".to_owned()));
    };
    let labelled = ops::label::set(env, &id, &name)?;
    emit(json, &labelled, || report::labelled(st, &labelled))?;
    Ok(std::process::ExitCode::SUCCESS)
}

fn run() -> Result<std::process::ExitCode> {
    let cli = Cli::parse();
    let st = cli.style(std::io::stdout().is_terminal())?;
    // One decision, taken above, then handed to anstream so it does not take a
    // second one. Left on auto it would strip the escapes that --color always
    // asked for, because it looks at the terminal too. Its job here is only to
    // translate for a console that needs it.
    anstream::ColorChoice::write_global(if st.is_colored() {
        anstream::ColorChoice::AlwaysAnsi
    } else {
        anstream::ColorChoice::Never
    });
    let env = Env::discover()?;

    match cli.command.unwrap_or(Command::Doctor { strict: false }) {
        Command::Doctor { strict } => {
            let report = ops::doctor::run(&env)?;
            if cli.json {
                let rendered =
                    serde_json::to_string_pretty(&report).map_err(|e| Error::json("stdout", e))?;
                println!("{rendered}");
            } else {
                anstream::print!("{}", report::doctor(st, &report));
            }
            if strict && report.has_warnings() {
                return Ok(std::process::ExitCode::FAILURE);
            }
            Ok(std::process::ExitCode::SUCCESS)
        }
        Command::Find { query, filters, format } => {
            let mut filter = filters.to_filter()?;
            filter.query = query;
            let results = ops::find::run(&env, &filter)?;
            let format = if cli.json { Format::Json } else { format };
            match format {
                Format::Json => {
                    let rendered = serde_json::to_string_pretty(&results)
                        .map_err(|e| Error::json("stdout", e))?;
                    println!("{rendered}");
                }
                Format::Table => anstream::print!("{}", report::find(st, &results, &env.home)),
                Format::Paths => print!("{}", report::paths(&results)),
                Format::Resume => print!("{}", report::resume_commands(&results)),
            }
            if results.rows.is_empty() {
                return Err(Error::NoMatches);
            }
            Ok(std::process::ExitCode::SUCCESS)
        }
        Command::Apply { filters, dry_run, keep_mcp, no_prune, adopt_cli_sessions } => {
            let filter = filters.to_filter()?;
            let options = ops::apply::Options {
                dry_run,
                keep_account_scoped: keep_mcp,
                no_prune,
                adopt_cli_sessions,
            };
            let report = ops::apply::run(&env, &filter, &options)?;
            emit(cli.json, &report, || report::apply(st, &report))?;
            pending(dry_run, report.changes())
        }
        Command::Off { dry_run, purge } => {
            let report = ops::off::run(&env, &ops::off::Options { dry_run, purge })?;
            emit(cli.json, &report, || report::off(st, &report))?;
            pending(dry_run, report.removed.len())
        }
        Command::Label { id, name, learn, list } => {
            label(&env, st, cli.json, id, name, learn, list)
        }
        Command::Restore { name, dry_run, force, skip_conflicts, rewrite_cwd } => {
            let rewrite_cwd = rewrite_cwd
                .iter()
                .map(|pair| {
                    pair.split_once('=').map(|(a, b)| (a.to_owned(), b.to_owned())).ok_or_else(
                        || {
                            Error::Usage(format!(
                                "--rewrite-cwd espera VIEJO=NUEVO, recibi {pair:?}"
                            ))
                        },
                    )
                })
                .collect::<Result<Vec<_>>>()?;
            let options = ops::restore::Options { dry_run, force, skip_conflicts, rewrite_cwd };
            let report = ops::restore::run(&env, &name, &options)?;
            if cli.json {
                let rendered =
                    serde_json::to_string_pretty(&report).map_err(|e| Error::json("stdout", e))?;
                println!("{rendered}");
            } else {
                anstream::print!("{}", report::restore(st, &report));
            }
            pending(dry_run, report.restored)
        }
        Command::Snapshot { scope, name, metadata_only } => {
            let options = unsilo::snapshot::Options { with_bodies: !metadata_only };
            let written = ops::snapshot::run(&env, scope.into(), &name, options)?;
            if cli.json {
                let rendered = serde_json::to_string_pretty(&written.manifest)
                    .map_err(|e| Error::json("stdout", e))?;
                println!("{rendered}");
            } else {
                anstream::print!("{}", report::snapshot(st, &written));
            }
            Ok(std::process::ExitCode::SUCCESS)
        }
    }
}
