//! jg binary entry point: alias expansion, argument parsing, dispatch and output.

mod cli;

use clap::{CommandFactory, Parser};
use cli::{Cli, Cmd, ColorChoice};
use jg::ops::{self, Report, Style, branch, config, remote, stash};
use std::io::{IsTerminal, Write};

fn main() {
    ops::update::cleanup_old();
    let raw: Vec<String> = std::env::args().collect();
    let wants_json = raw.iter().any(|a| a == "--json");

    let builtin: Vec<String> = Cli::command()
        .get_subcommands()
        .flat_map(|c| std::iter::once(c.get_name().to_string()).chain(c.get_all_aliases().map(String::from)))
        .chain(["help".to_string()])
        .collect();
    let args = match config::expand_alias(raw, &builtin) {
        Ok(a) => a,
        Err(e) => fail(&e, wants_json),
    };
    let cli = Cli::parse_from(args);
    if let Some(d) = &cli.dir
        && let Err(e) = std::env::set_current_dir(d)
    {
        fail(&anyhow::anyhow!("cannot change to '{}': {e}", d.display()), cli.json);
    }
    let color = match cli.color {
        ColorChoice::Always => true,
        ColorChoice::Never => false,
        ColorChoice::Auto => !cli.json && std::io::stdout().is_terminal() && std::env::var_os("NO_COLOR").is_none(),
    };
    let ctx = Ctx { json: cli.json, style: Style { color } };
    if let Err(e) = dispatch(cli.cmd, &ctx) {
        fail(&e, ctx.json);
    }
}

struct Ctx {
    json: bool,
    style: Style,
}

/// Print a report as JSON or human text. Conflicts exit with status 1 (like git).
fn emit<R: Report>(r: R, ctx: &Ctx) -> anyhow::Result<()> {
    let value = serde_json::to_value(&r)?;
    if ctx.json {
        println!("{}", serde_json::to_string_pretty(&value)?);
    } else {
        let text = r.human(ctx.style);
        if !text.is_empty() {
            println!("{text}");
        }
    }
    std::io::stdout().flush().ok();
    if value.get("result").and_then(|v| v.as_str()) == Some("conflict") {
        std::process::exit(1);
    }
    Ok(())
}

fn fail(e: &anyhow::Error, json: bool) -> ! {
    if json {
        println!("{}", serde_json::json!({ "error": format!("{e:#}") }));
    } else {
        eprintln!("error: {e:#}");
    }
    std::process::exit(1);
}

fn dispatch(cmd: Cmd, ctx: &Ctx) -> anyhow::Result<()> {
    match cmd {
        Cmd::Init(o) => emit(ops::init::run(o)?, ctx),
        Cmd::Clone(o) => emit(ops::sync::clone(o)?, ctx),
        Cmd::Status(o) => emit(ops::status::run(o)?, ctx),
        Cmd::Add(o) => emit(ops::stage::add(o)?, ctx),
        Cmd::Rm(o) => emit(ops::stage::rm(o)?, ctx),
        Cmd::Mv(o) => emit(ops::stage::mv(o)?, ctx),
        Cmd::Restore(o) => emit(ops::stage::restore(o)?, ctx),
        Cmd::Commit(o) => emit(ops::commit::run(o)?, ctx),
        Cmd::Diff(o) => emit(ops::diff::run(o)?, ctx),
        Cmd::Log(o) => emit(ops::log::run(o)?, ctx),
        Cmd::Show(o) => emit(ops::log::show(o)?, ctx),
        Cmd::Reflog(o) => emit(ops::log::reflog(o)?, ctx),
        Cmd::Branch(o) => match ops::branch::run(o)? {
            branch::BranchResult::List(l) => emit(l, ctx),
            branch::BranchResult::Done(m) => emit(m, ctx),
        },
        Cmd::Checkout(o) => emit(ops::branch::checkout(o)?, ctx),
        Cmd::Merge(o) => emit(ops::merge::merge(o)?, ctx),
        Cmd::CherryPick(o) => emit(ops::merge::cherry_pick(o)?, ctx),
        Cmd::Revert(o) => emit(ops::merge::revert(o)?, ctx),
        Cmd::Reset(o) => emit(ops::reset::run(o)?, ctx),
        Cmd::Tag(o) => match ops::branch::tag(o)? {
            branch::TagResult::List(l) => emit(l, ctx),
            branch::TagResult::Done(m) => emit(m, ctx),
        },
        Cmd::Stash { cmd, push } => {
            let cmd = cmd.or(Some(stash::StashCmd::Push(push)));
            match ops::stash::run(cmd)? {
                stash::StashResult::List(l) => emit(l, ctx),
                stash::StashResult::Show(d) => emit(d, ctx),
                stash::StashResult::Done(m) => emit(m, ctx),
            }
        }
        Cmd::Remote { cmd } => match ops::remote::run(cmd)? {
            remote::RemoteResult::List(l) => emit(l, ctx),
            remote::RemoteResult::Done(m) => emit(m, ctx),
        },
        Cmd::Fetch(o) => emit(ops::sync::fetch(o)?, ctx),
        Cmd::Pull(o) => emit(ops::sync::pull(o)?, ctx),
        Cmd::Push(o) => emit(ops::sync::push(o)?, ctx),
        Cmd::GitExport(o) => emit(ops::gitexport::run(o)?, ctx),
        Cmd::Config(o) => match ops::config::config(o)? {
            config::ConfigResult::List(l) => emit(l, ctx),
            config::ConfigResult::Done(m) => emit(m, ctx),
        },
        Cmd::Alias { cmd } => match ops::config::alias(cmd)? {
            config::AliasResult::List(l) => emit(l, ctx),
            config::AliasResult::Done(m) => emit(m, ctx),
        },
        Cmd::ShellInit(o) => {
            print!("{}", ops::config::shell_init(o)?);
            Ok(())
        }
        Cmd::Completions { shell } => {
            clap_complete::generate(shell, &mut Cli::command(), "jg", &mut std::io::stdout());
            Ok(())
        }
        Cmd::SelfUpdate(o) => emit(ops::update::run(o)?, ctx),
        Cmd::AgentGuide => {
            print!("{}", ops::agent::GUIDE);
            Ok(())
        }
        Cmd::Version => emit(
            ops::Message::new(
                format!("jg {} ({})", ops::update::VERSION, ops::update::platform_key()),
                serde_json::json!({ "version": ops::update::VERSION, "platform": ops::update::platform_key() }),
            ),
            ctx,
        ),
        Cmd::Serve { path } => jg::transport::serve(&path),
    }
}
