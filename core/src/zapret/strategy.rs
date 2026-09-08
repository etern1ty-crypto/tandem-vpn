//! Constrained Flowseal batch parser. Batch files are data, never executed.
use crate::config::GameFilter;
use crate::{Error, Result};
use serde::Serialize;
use std::path::{Path, PathBuf};

const VALUE_OPTIONS: &[&str] = &[
    "wf-tcp",
    "wf-udp",
    "wf-l3",
    "ip-id",
    "filter-tcp",
    "filter-udp",
    "filter-l3",
    "filter-l7",
    "hostlist",
    "hostlist-exclude",
    "hostlist-domains",
    "hostlist-exclude-domains",
    "ipset",
    "ipset-exclude",
    "ipset-ip",
    "ipset-ip-exclude",
    "dpi-desync",
    "dpi-desync-repeats",
    "dpi-desync-ttl",
    "dpi-desync-ttl6",
    "dpi-desync-autottl",
    "dpi-desync-autottl6",
    "dpi-desync-fooling",
    "dpi-desync-split-pos",
    "dpi-desync-split-seqovl",
    "dpi-desync-split-seqovl-pattern",
    "dpi-desync-fake-tls",
    "dpi-desync-fake-http",
    "dpi-desync-fake-quic",
    "dpi-desync-fake-unknown-udp",
    "dpi-desync-fake-unknown-tcp",
    "dpi-desync-fake-discord",
    "dpi-desync-fake-stun",
    "dpi-desync-fake-dht",
    "dpi-desync-fake-wireguard",
    "dpi-desync-fake-syndata",
    "dpi-desync-fake-tcp-mod",
    "dpi-desync-fake-tls-mod",
    "dpi-desync-fake-tls-sni",
    "dpi-desync-cutoff",
    "dpi-desync-start",
    "dpi-desync-any-protocol",
    "dpi-desync-skip-nosni",
    "dpi-desync-badseq-increment",
    "dpi-desync-badack-increment",
    "dpi-desync-udplen-increment",
    "dpi-desync-udplen-pattern",
    "dpi-desync-fake-tls-padencap",
    "dpi-desync-hostfakesplit-mod",
    "dpi-desync-hostfakesplit-midhost",
    "dpi-desync-hostfakesplit-host",
    "dpi-desync-fakedsplit-pattern",
    "dpi-desync-fakeddisorder-pattern",
    "wssize",
    "wsize",
    "mss",
    "hostcase",
    "hostspell",
    "hostnospace",
    "domcase",
    "methodspace",
    "methodeol",
];
const FILE_OPTIONS: &[&str] = &[
    "hostlist",
    "hostlist-exclude",
    "ipset",
    "ipset-exclude",
    "dpi-desync-fake-tls",
    "dpi-desync-fake-http",
    "dpi-desync-fake-quic",
    "dpi-desync-fake-unknown-udp",
    "dpi-desync-fake-unknown-tcp",
    "dpi-desync-fake-discord",
    "dpi-desync-fake-stun",
    "dpi-desync-fake-dht",
    "dpi-desync-fake-wireguard",
    "dpi-desync-fake-syndata",
    "dpi-desync-split-seqovl-pattern",
    "dpi-desync-fakedsplit-pattern",
    "dpi-desync-fakeddisorder-pattern",
    "dpi-desync-udplen-pattern",
];

#[derive(Debug, Clone, Serialize)]
pub struct StrategyPlan {
    pub executable: String,
    pub args: Vec<String>,
    pub binary_path: String,
    pub referenced_files: Vec<String>,
}

fn logical_lines(contents: &str) -> Result<Vec<String>> {
    if contents.len() > 128 * 1024 || contents.contains('\0') {
        return Err(Error::Invalid(
            "Strategy is oversized or contains NUL".into(),
        ));
    }
    let mut lines = Vec::new();
    let mut current = String::new();
    for line in contents.trim_start_matches('\u{feff}').lines() {
        let line = line.trim_end();
        if let Some(part) = line.strip_suffix('^') {
            current.push_str(part);
            current.push(' ');
        } else {
            current.push_str(line);
            lines.push(std::mem::take(&mut current));
        }
    }
    if !current.is_empty() {
        return Err(Error::Invalid("Unfinished batch continuation".into()));
    }
    Ok(lines)
}

/// Restricted Windows-style tokenizer: balanced double quotes, no shell syntax.
/// Quotes group values; literal embedded quotes and backslash-quote escapes are
/// intentionally unsupported, since they are not needed by supported presets.
fn tokens(line: &str) -> Result<Vec<String>> {
    let mut result = Vec::new();
    let mut value = String::new();
    let mut quoted = false;
    for c in line.chars() {
        match c {
            '"' => quoted = !quoted,
            '&' | '|' | '<' | '>' | '^' | '\0' => {
                return Err(Error::Security(
                    "Shell operators in a strategy invocation are forbidden".into(),
                ))
            }
            c if c.is_whitespace() && !quoted => {
                if !value.is_empty() {
                    result.push(std::mem::take(&mut value));
                }
            }
            c if c.is_control() => {
                return Err(Error::Invalid("Control character in strategy".into()))
            }
            c => value.push(c),
        }
    }
    if quoted {
        return Err(Error::Invalid("Unbalanced strategy quotes".into()));
    }
    if !value.is_empty() {
        result.push(value);
    }
    Ok(result)
}

fn invocation(contents: &str) -> Result<Vec<String>> {
    let mut found = None;
    for line in logical_lines(contents)? {
        let line = line.trim().trim_start_matches('@');
        let lower = line.to_ascii_lowercase();
        if lower.starts_with("rem ") || lower.starts_with("::") || lower.starts_with("echo ") {
            continue;
        }
        if !(lower.starts_with("start ")
            || lower.starts_with("\"%bin%winws.exe\"")
            || lower.starts_with("%bin%winws.exe "))
        {
            continue;
        }
        let values = tokens(line)?;
        let positions: Vec<_> = values
            .iter()
            .enumerate()
            .filter(|(_, value)| {
                matches!(
                    value.to_ascii_lowercase().as_str(),
                    "%bin%winws.exe" | "%~dp0bin\\winws.exe" | "winws.exe"
                )
            })
            .map(|(index, _)| index)
            .collect();
        if positions.is_empty() {
            continue;
        }
        if positions.len() != 1 || found.is_some() {
            return Err(Error::Invalid(
                "Strategy must have exactly one winws invocation".into(),
            ));
        }
        let args = values[positions[0] + 1..].to_vec();
        if args.is_empty() {
            return Err(Error::Invalid("Empty winws argument list".into()));
        }
        found = Some(args);
    }
    found.ok_or_else(|| Error::Invalid("No supported start ... winws.exe invocation found".into()))
}

fn replace_ci(mut value: String, marker: &str, replacement: &str) -> String {
    let mut offset = 0;
    loop {
        let lower = value[offset..].to_ascii_lowercase();
        let Some(relative) = lower.find(marker) else {
            break;
        };
        let index = offset + relative;
        value.replace_range(index..index + marker.len(), replacement);
        offset = index + replacement.len();
    }
    value
}

/// Quote one argument according to CommandLineToArgvW / MS C runtime rules.
pub fn quote_arg(value: &str) -> String {
    let mut out = String::from("\"");
    let mut slashes = 0;
    for c in value.chars() {
        match c {
            '\\' => slashes += 1,
            '"' => {
                out.push_str(&"\\".repeat(slashes * 2 + 1));
                out.push('"');
                slashes = 0;
            }
            c => {
                out.push_str(&"\\".repeat(slashes));
                out.push(c);
                slashes = 0;
            }
        }
    }
    out.push_str(&"\\".repeat(slashes * 2));
    out.push('"');
    out
}

pub fn parse_and_render(
    contents: &str,
    engine: &Path,
    game: GameFilter,
    require_files: bool,
) -> Result<StrategyPlan> {
    if !engine.is_absolute() {
        return Err(Error::Invalid("Engine path must be absolute".into()));
    }
    let bin = engine.join("bin");
    let lists = engine.join("lists");
    let sep = std::path::MAIN_SEPARATOR;
    let mut args = Vec::new();
    let mut referenced = Vec::new();
    for raw in invocation(contents)? {
        if !raw.starts_with("--") {
            return Err(Error::Invalid(format!("Expected --option, got {raw}")));
        }
        if [
            "--new",
            "--hostcase",
            "--hostnospace",
            "--domcase",
            "--methodspace",
            "--methodeol",
        ]
        .contains(&raw.as_str())
        {
            args.push(raw);
            continue;
        }
        let (key, raw_value) = raw[2..]
            .split_once('=')
            .ok_or_else(|| Error::Invalid(format!("Option must use --key=value: {raw}")))?;
        if !VALUE_OPTIONS.contains(&key) {
            return Err(Error::Security(format!(
                "Unsupported winws option: --{key}; preset was not executed"
            )));
        }
        let mut value = raw_value.to_owned();
        for (marker, replacement) in [
            ("%gamefiltertcp%", game.tcp().to_owned()),
            ("%gamefilterudp%", game.udp().to_owned()),
            ("%gamefilter%", game.combined().to_owned()),
            ("%bin%", format!("{}{sep}", bin.display())),
            ("%lists%", format!("{}{sep}", lists.display())),
        ] {
            value = replace_ci(value, marker, &replacement);
        }
        if value.is_empty() || value.contains(['%', '!', '\r', '\n', '\0']) {
            return Err(Error::Invalid(format!(
                "Empty or unresolved value for --{key}"
            )));
        }
        if matches!(key, "wf-tcp" | "wf-udp" | "filter-tcp" | "filter-udp") {
            validate_ports(&value)?;
        }
        if FILE_OPTIONS.contains(&key) {
            if let Some(hex) = value.strip_prefix("0x") {
                if !key.starts_with("dpi-desync-")
                    || hex.is_empty()
                    || hex.len() > 65536
                    || hex.len() % 2 != 0
                    || !hex.bytes().all(|c| c.is_ascii_hexdigit())
                {
                    return Err(Error::Invalid("Invalid inline packet bytes".into()));
                }
            } else {
                let path = PathBuf::from(&value);
                let allowed = [&bin, &lists].iter().any(|base| {
                    path.strip_prefix(base).ok().is_some_and(|relative| {
                        relative.components().count() == 1
                            && crate::files::safe_component(&relative.to_string_lossy()).is_ok()
                    })
                });
                if !allowed {
                    return Err(Error::Security(format!(
                        "File option --{key} must reference bin/ or lists/ directly"
                    )));
                }
                crate::files::reject_links(&path)?;
                if require_files && !path.is_file() {
                    return Err(Error::Invalid(format!(
                        "Referenced file is missing: {}",
                        path.display()
                    )));
                }
                referenced.push(value.clone());
            }
        }
        args.push(format!("--{key}={value}"));
    }
    if !args
        .iter()
        .any(|arg| arg.starts_with("--wf-tcp=") || arg.starts_with("--wf-udp="))
    {
        return Err(Error::Invalid(
            "Preset needs an explicit packet capture filter".into(),
        ));
    }
    if args.len() > 512 {
        return Err(Error::Invalid("Too many strategy arguments".into()));
    }
    let executable = bin.join("winws.exe").to_string_lossy().into_owned();
    let binary_path = std::iter::once(executable.as_str())
        .chain(args.iter().map(String::as_str))
        .map(quote_arg)
        .collect::<Vec<_>>()
        .join(" ");
    if binary_path.encode_utf16().count() > 30000 {
        return Err(Error::Invalid(
            "Windows service command line is too long".into(),
        ));
    }
    Ok(StrategyPlan {
        executable,
        args,
        binary_path,
        referenced_files: referenced,
    })
}

fn validate_ports(value: &str) -> Result<()> {
    for part in value.split(',') {
        let mut pair = part.split('-');
        let start = pair
            .next()
            .and_then(|v| v.parse::<u16>().ok())
            .ok_or_else(|| Error::Invalid(format!("Invalid port set: {value}")))?;
        if let Some(end) = pair.next() {
            let end = end
                .parse::<u16>()
                .map_err(|_| Error::Invalid("Invalid port range".into()))?;
            if start > end || pair.next().is_some() {
                return Err(Error::Invalid("Descending or malformed port range".into()));
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn root() -> PathBuf {
        std::env::temp_dir().join("Tandem Test").join("engine")
    }
    fn sample() -> &'static str {
        "@echo off\nrem winws.exe --evil\nstart \"title\" /min \"%BIN%winws.exe\" --wf-tcp=80,443,%GameFilterTCP% ^\n--filter-tcp=443 --dpi-desync=fake\n"
    }
    #[test]
    fn parse_supported_invocation() {
        let plan = parse_and_render(sample(), &root(), GameFilter::Disabled, false).unwrap();
        assert!(plan.args.contains(&"--wf-tcp=80,443,12".to_owned()));
        assert!(!plan.binary_path.contains('%'));
    }
    #[test]
    fn case_insensitive_placeholders() {
        assert!(parse_and_render(
            &sample()
                .replace("%BIN%", "%bin%")
                .replace("winws.exe", "WINWS.EXE"),
            &root(),
            GameFilter::Tcp,
            false
        )
        .is_ok());
    }
    #[test]
    fn rejects_shell_and_multiple_invocations() {
        for input in [
            format!("{} & calc.exe", sample().trim()),
            sample().repeat(2),
            "rem winws.exe --wf-tcp=443".into(),
            "start x \"%BIN%winws.exe\" --wf-tcp=443 --debug=C:\\evil".into(),
        ] {
            assert!(parse_and_render(&input, &root(), GameFilter::Disabled, false).is_err());
        }
    }
    #[test]
    fn rejects_unknown_expansion() {
        assert!(parse_and_render(
            &sample().replace("%GameFilterTCP%", "%SECRET%"),
            &root(),
            GameFilter::Disabled,
            false
        )
        .is_err());
    }
    #[test]
    fn rejects_traversing_file_reference() {
        let input = format!("{} --hostlist=\"%LISTS%../secret\"", sample().trim());
        assert!(parse_and_render(&input, &root(), GameFilter::Disabled, false).is_err());
    }
    #[test]
    fn quotes_windows_trailing_backslash() {
        assert_eq!(quote_arg("C:\\folder name\\"), "\"C:\\folder name\\\\\"");
    }
    #[test]
    fn rejects_bad_ports() {
        for value in ["80,", "65536", "3-1", "1-2-3"] {
            assert!(validate_ports(value).is_err());
        }
    }
    #[test]
    fn unfinished_continuation_fails() {
        assert!(logical_lines("start x ^").is_err());
    }
}
