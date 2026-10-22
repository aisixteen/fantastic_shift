//! The team binary: fantastic's terminal-shell library (`fantastic_tui`) with this
//! repo's bundles added. Surfaces are opt-in: no flag runs fully headless (no
//! port, no terminal UI); `--port N` serves the web UI; `--tui` opens the
//! terminal UI.

use std::io::IsTerminal;
use std::process::ExitCode;

use fantastic_team::{is_passthrough, parse_args, register_team, Surface, USAGE};

/// The kernel runs in the directory it is started in, so `--dir` is a `chdir`
/// before the shell boots.
fn main() -> ExitCode {
    let all: Vec<String> = std::env::args().skip(1).collect();
    // `setup github`: handles its own arguments, then exits.
    if all.first().map(String::as_str) == Some("setup") {
        return match fantastic_team::setup_github::run_cli(&all[1..]) {
            Ok(()) => ExitCode::SUCCESS,
            Err(e) => {
                eprintln!("fantastic_team: {e}");
                ExitCode::from(2)
            }
        };
    }
    // `--resend-instructions`: send the first agent its instructions again, even
    // when it already exists. Taken out before the other flags are parsed.
    let resend = all.iter().any(|a| a == "--resend-instructions");
    let raw: Vec<String> = all
        .into_iter()
        .filter(|a| a != "--resend-instructions")
        .collect();
    let (flags, argv) = match fantastic_team::instructions::parse_flags(&raw) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("fantastic_team: {e}\n{USAGE}");
            return ExitCode::from(2);
        }
    };
    // No instruction flags at all: the team starts without a first message.
    let instructions = if flags == fantastic_team::instructions::Flags::default() {
        None
    } else {
        match fantastic_team::instructions::render(&flags) {
            Ok(text) => Some(text),
            Err(e) => {
                eprintln!("fantastic_team: {e}\n{USAGE}");
                return ExitCode::from(2);
            }
        }
    };
    let extras: &[fantastic_tui::Extra] = &[register_team];

    // `mcp`, `send`, `readme`: fantastic's own commands, run in this directory.
    if is_passthrough(&argv) {
        return finish(fantastic_tui::main_with(argv, extras));
    }

    let args = match parse_args(&argv) {
        Ok(a) => a,
        Err(e) => {
            eprintln!("{e}");
            return ExitCode::from(2);
        }
    };
    // The terminal UI draws on a terminal. Say so here: with no arguments the
    // shell would print its readme (stdout is not a terminal) and run nothing.
    if args.surface == Surface::Tui && !std::io::stdout().is_terminal() {
        eprintln!("fantastic_team: --tui needs a terminal — without one, run with no flags (headless) or --port N\n{USAGE}");
        return ExitCode::from(2);
    }
    if let Err(e) =
        std::fs::create_dir_all(&args.dir).and_then(|()| std::env::set_current_dir(&args.dir))
    {
        eprintln!(
            "fantastic_team: cannot use --dir {}: {e}",
            args.dir.display()
        );
        return ExitCode::from(2);
    }
    // Plan and apply the team's tree from team.yaml, then deliver the rendered
    // instructions to the first agent when it was just created (or when asked).
    if let Err(e) = fantastic_team::tree::run_at_start(
        std::path::Path::new("."),
        instructions.as_deref(),
        resend,
    ) {
        eprintln!("fantastic_team: {e}");
        return ExitCode::from(1);
    }
    finish(fantastic_tui::main_with(args.fantastic_args(), extras))
}

fn finish<E: std::fmt::Display>(result: Result<(), E>) -> ExitCode {
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("fantastic_team: {e}");
            ExitCode::from(1)
        }
    }
}
