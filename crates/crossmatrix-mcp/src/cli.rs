//! Command-line front end. No external arg parser: `init`, `check`,
//! `--version`, `--help`, or (with no args) serve MCP on stdio.

use std::io::Write;
use std::path::PathBuf;

use crate::repo;

/// What `main` should do after `run` returns.
pub enum CliAction {
    Serve,
    Exit(i32),
}

/// Parse and execute one CLI invocation. `args` excludes `argv[0]`.
pub fn run(args: &[String], out: &mut dyn Write, err: &mut dyn Write) -> CliAction {
    match args.first().map(String::as_str) {
        None => CliAction::Serve,
        Some("--version") | Some("-V") => {
            let _ = writeln!(out, "crossmatrix-mcp {}", env!("CARGO_PKG_VERSION"));
            CliAction::Exit(0)
        }
        Some("--help") | Some("-h") => {
            let _ = write!(out, "{}", usage());
            CliAction::Exit(0)
        }
        Some("init") => command_init(&args[1..], out, err),
        Some("check") => command_check(&args[1..], out, err),
        Some(_) => {
            let _ = write!(err, "{}", usage());
            CliAction::Exit(2)
        }
    }
}

fn command_init(args: &[String], out: &mut dyn Write, err: &mut dyn Write) -> CliAction {
    let dir = match parse_dir(args) {
        Ok(Some(dir)) => dir,
        Ok(None) => std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
        Err(message) => return usage_error(err, "init", &message),
    };
    match repo::init(&dir) {
        Ok(report) => {
            for path in &report.created {
                let _ = writeln!(out, "created {path}");
            }
            for path in &report.preserved {
                let _ = writeln!(out, "preserved {path}");
            }
            let _ = writeln!(out, "hint: commit .crossmatrix/ to version the model store");
            CliAction::Exit(0)
        }
        Err(message) => {
            let _ = writeln!(err, "crossmatrix init: {message}");
            CliAction::Exit(1)
        }
    }
}

fn command_check(args: &[String], out: &mut dyn Write, err: &mut dyn Write) -> CliAction {
    let root = match parse_dir(args) {
        Ok(Some(dir)) => dir.join(".crossmatrix"),
        Ok(None) => repo::resolve_root_from_process().0,
        Err(message) => return usage_error(err, "check", &message),
    };
    let report = repo::check(&root);
    for diagnostic in &report.diagnostics {
        let _ = writeln!(out, "{}: {}", diagnostic.path, diagnostic.message);
    }
    let _ = writeln!(
        out,
        "crossmatrix check: {} document(s), {} problem(s)",
        report.documents,
        report.diagnostics.len()
    );
    if report.diagnostics.is_empty() {
        CliAction::Exit(0)
    } else {
        CliAction::Exit(1)
    }
}

fn usage_error(err: &mut dyn Write, command: &str, message: &str) -> CliAction {
    let _ = writeln!(err, "crossmatrix {command}: {message}");
    let _ = write!(err, "{}", usage());
    CliAction::Exit(2)
}

fn parse_dir(args: &[String]) -> Result<Option<PathBuf>, String> {
    let mut dir = None;
    let mut index = 0;
    while index < args.len() {
        match args[index].as_str() {
            "--dir" => {
                let value = args
                    .get(index + 1)
                    .ok_or_else(|| "--dir requires a path".to_string())?;
                dir = Some(PathBuf::from(value));
                index += 2;
            }
            other => return Err(format!("unknown option '{other}'")),
        }
    }
    Ok(dir)
}

fn usage() -> String {
    format!(
        "crossmatrix-mcp {version}\n\
\n\
USAGE:\n\
  crossmatrix-mcp                 Serve the MCP server on stdio\n\
  crossmatrix-mcp init [--dir D]  Create a .crossmatrix/ store in D (default: cwd)\n\
  crossmatrix-mcp check [--dir D] Validate the store at D/.crossmatrix (default: resolved root)\n\
  crossmatrix-mcp --version       Print version and exit\n\
  crossmatrix-mcp --help          Print this help and exit\n\
\n\
STATE-DIR RESOLUTION ORDER:\n\
  1. $CROSSMATRIX_DIR (relative values are joined onto the cwd)\n\
  2. nearest ancestor of the cwd containing a .crossmatrix/ directory\n\
  3. $XDG_DATA_HOME/crossmatrix\n\
  4. $HOME/.local/share/crossmatrix\n\
  5. <cwd>/crossmatrix-state\n",
        version = env!("CARGO_PKG_VERSION")
    )
}
