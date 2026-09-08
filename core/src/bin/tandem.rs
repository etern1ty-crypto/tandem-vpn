//! Dependency-light CLI. stdout is JSON; operational logs go to stderr.
use tandem_core::{Action, Error, Result, Workbench};

const HELP: &str = "Tandem Workbench — Windows connectivity operations\n\nUsage:\n  tandem --help | --version\n  tandem config-check FILE\n  tandem inspect STRATEGY_FILE\n  tandem bundle-check ZIP SHA256 TAG\n  tandem init | status | doctor | support | check-updates | test-targets\n  tandem config-apply FILE\n  tandem preview STRATEGY_NAME\n  tandem service install STRATEGY_NAME\n  tandem service start | stop | remove\n  tandem bundle import ZIP SHA256 TAG\n  tandem bundle download SHA256 TAG\n  tandem bundle rollback\n  tandem ipset import FILE\n  tandem hosts apply FILE\n  tandem hosts restore\n  tandem recover\n\nUse absolute local paths for file operations. Privileged changes require an\nadministrator terminal. Read-only offline commands work on Linux/macOS too.\nJSON stdout, errors and audit events on stderr. Exit: 0 success, 2 invalid input,\n3 permission/security, 4 busy, 5 operation failure. TANDEM_QUIET=1 mutes audit\nevents on stderr, not the on-disk audit log.\n";

fn parse(args: &[String]) -> Result<Action> {
    let words: Vec<_> = args.iter().map(String::as_str).collect();
    match words.as_slice() {
        ["init"] => Ok(Action::Initialize {}),
        ["status"] => Ok(Action::GetDashboard {}),
        ["doctor"] => Ok(Action::Diagnostics {}),
        ["support"] => Ok(Action::ExportSupport {}),
        ["check-updates"] => Ok(Action::CheckUpdates {}),
        ["test-targets"] => Ok(Action::TestTargets {}),
        ["config-apply", path] => {
            let path = tandem_core::workbench::input_path(path)?;
            let config: tandem_core::config::Config =
                serde_json::from_slice(&tandem_core::files::read_bounded(&path, 64 * 1024)?)?;
            config.validate()?;
            Ok(Action::SaveConfig { config })
        }
        ["preview", name] => Ok(Action::PreviewStrategy {
            strategy: (*name).into(),
        }),
        ["service", "install", name] => Ok(Action::InstallService {
            strategy: (*name).into(),
        }),
        ["service", "start"] => Ok(Action::StartService {}),
        ["service", "stop"] => Ok(Action::StopService {}),
        ["service", "remove"] => Ok(Action::RemoveService {}),
        ["bundle", "import", path, sha256, tag] => Ok(Action::ImportBundle {
            path: (*path).into(),
            sha256: (*sha256).into(),
            tag: (*tag).into(),
        }),
        ["bundle", "download", sha256, tag] => Ok(Action::DownloadBundle {
            sha256: (*sha256).into(),
            tag: (*tag).into(),
        }),
        ["bundle", "rollback"] => Ok(Action::RollbackBundle {}),
        ["ipset", "import", path] => Ok(Action::ImportIpset {
            path: (*path).into(),
        }),
        ["hosts", "apply", path] => Ok(Action::ApplyHosts {
            path: (*path).into(),
        }),
        ["hosts", "restore"] => Ok(Action::RestoreHosts {}),
        ["recover"] => Ok(Action::Recover {}),
        _ => Err(Error::Invalid(
            "Unknown command or argument count. Run tandem --help".into(),
        )),
    }
}

fn clean_path(path: impl AsRef<std::path::Path>) -> std::io::Result<std::path::PathBuf> {
    let p = std::fs::canonicalize(path)?;
    #[cfg(windows)]
    {
        let s = p.to_string_lossy();
        if let Some(stripped) = s.strip_prefix(r"\\?\") {
            return Ok(std::path::PathBuf::from(stripped));
        }
    }
    Ok(p)
}

fn run(args: &[String]) -> Result<serde_json::Value> {
    let words: Vec<_> = args.iter().map(String::as_str).collect();
    match words.as_slice() {
        ["config-check", path] => {
            let contents = tandem_core::files::read_bounded(&clean_path(path)?, 64 * 1024)?;
            let config: tandem_core::config::Config = serde_json::from_slice(&contents)?;
            config.validate()?;
            Ok(serde_json::json!({"valid":true,"config":config}))
        }
        ["inspect", path] => {
            let path = clean_path(path)?;
            let parent = path
                .parent()
                .ok_or_else(|| Error::Invalid("Strategy has no parent".into()))?;
            let source = tandem_core::files::read_text(&path, 128 * 1024)?;
            Ok(serde_json::to_value(
                tandem_core::zapret::strategy::parse_and_render(
                    &source,
                    parent,
                    tandem_core::config::GameFilter::Disabled,
                    false,
                )?,
            )?)
        }
        ["bundle-check", path, hash, tag] => {
            let path = clean_path(path)?;
            let bytes = tandem_core::files::read_bounded(&path, tandem_core::network::MAX_ARCHIVE)?;
            Ok(serde_json::to_value(tandem_core::bundle::inspect(
                &bytes, hash, tag,
            )?)?)
        }
        _ => {
            let action = parse(args)?;
            Workbench::new()?.execute(action)
        }
    }
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.is_empty() || args.as_slice() == ["--help"] || args.as_slice() == ["-h"] {
        print!("{HELP}");
        return;
    }
    if args.as_slice() == ["--version"] {
        println!("tandem {}", env!("CARGO_PKG_VERSION"));
        return;
    }
    match run(&args) {
        Ok(value) => match serde_json::to_string_pretty(&value) {
            Ok(text) => println!("{text}"),
            Err(error) => {
                eprintln!("{error}");
                std::process::exit(5);
            }
        },
        Err(error) => {
            let code = match &error {
                Error::Invalid(_) | Error::Json(_) => 2,
                Error::AdministratorRequired | Error::Security(_) => 3,
                Error::Busy => 4,
                _ => 5,
            };
            eprintln!(
                "{}",
                serde_json::json!({"error":error.to_string(),"exit_code":code})
            );
            std::process::exit(code);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn unknown_commands_fail() {
        assert!(parse(&["delete-everything".into()]).is_err());
    }
    #[test]
    fn surplus_args_fail() {
        assert!(parse(&["service".into(), "remove".into(), "WinDivert".into()]).is_err());
    }
    #[test]
    fn typed_cli_action() {
        assert!(matches!(
            parse(&["service".into(), "stop".into()]).unwrap(),
            Action::StopService {}
        ));
    }
}
