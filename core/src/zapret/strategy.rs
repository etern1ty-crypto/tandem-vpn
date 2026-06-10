//! Parsing of Flowseal strategy `.bat` files into a `winws.exe` argument string.
//!
//! A strategy file launches the bypass with a line like:
//!
//! ```bat
//! start "zapret: %~n0" /min "%BIN%winws.exe" --wf-tcp=80,443,%GameFilterTCP% ^
//! --filter-udp=443 --hostlist="%LISTS%list-general.txt" --dpi-desync=fake --new ^
//! --filter-tcp=443 --dpi-desync=multisplit
//! ```
//!
//! Lines are joined on the trailing `^` continuation character. Everything after
//! `winws.exe"` is the argument string, which still contains the placeholders
//! `%BIN%`, `%LISTS%`, `%GameFilter%`, `%GameFilterTCP%` and `%GameFilterUDP%`.
//! [`render_winws_args`] substitutes those placeholders the same way the upstream
//! `service.bat` does, producing the exact arguments used to create the service.

use std::path::Path;

/// Game-filter configuration. When disabled the placeholders expand to empty
/// strings, mirroring `service.bat` with the game filter turned off.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GameFilter {
    pub enabled: bool,
    /// Value substituted for `%GameFilterTCP%` when enabled.
    pub tcp: String,
    /// Value substituted for `%GameFilterUDP%` when enabled.
    pub udp: String,
    /// Value substituted for `%GameFilter%` when enabled.
    pub combined: String,
}

impl Default for GameFilter {
    fn default() -> Self {
        Self::disabled()
    }
}

impl GameFilter {
    pub fn disabled() -> Self {
        Self {
            enabled: false,
            tcp: "1024-65535".into(),
            udp: "1024-65535".into(),
            combined: "1024-65535".into(),
        }
    }

    pub fn enabled() -> Self {
        Self {
            enabled: true,
            ..Self::disabled()
        }
    }

    fn tcp_value(&self) -> &str {
        if self.enabled {
            &self.tcp
        } else {
            ""
        }
    }
    fn udp_value(&self) -> &str {
        if self.enabled {
            &self.udp
        } else {
            ""
        }
    }
    fn combined_value(&self) -> &str {
        if self.enabled {
            &self.combined
        } else {
            ""
        }
    }
}

/// Join `^`-continued lines into logical lines.
fn join_continuations(contents: &str) -> Vec<String> {
    let mut logical = Vec::new();
    let mut current = String::new();
    for raw in contents.lines() {
        let line = raw.trim_end_matches(['\r']);
        let trimmed = line.trim_end();
        if let Some(stripped) = trimmed.strip_suffix('^') {
            current.push_str(stripped);
            current.push(' ');
        } else {
            current.push_str(line);
            logical.push(std::mem::take(&mut current));
        }
    }
    if !current.is_empty() {
        logical.push(current);
    }
    logical
}

/// Extract the raw (placeholder-laden) argument string that follows
/// `winws.exe` in a strategy file. Returns `None` if no invocation is found.
pub fn extract_winws_args(contents: &str) -> Option<String> {
    const MARKER: &str = "winws.exe";
    for line in join_continuations(contents) {
        if let Some(pos) = line.find(MARKER) {
            let rest = &line[pos + MARKER.len()..];
            let rest = rest.trim_start();
            // Drop the closing quote of `"%BIN%winws.exe"`.
            let rest = rest.strip_prefix('"').unwrap_or(rest);
            let args = rest.trim();
            if !args.is_empty() {
                return Some(args.to_string());
            }
        }
    }
    None
}

fn with_trailing_sep(p: &Path) -> String {
    let mut s = p.to_string_lossy().into_owned();
    let sep = if cfg!(windows) { '\\' } else { '/' };
    if !s.ends_with(sep) && !s.ends_with('/') && !s.ends_with('\\') {
        s.push(sep);
    }
    s
}

/// Substitute Flowseal placeholders in a raw argument string.
///
/// * `%BIN%`            → `bin_dir` (with trailing separator)
/// * `%LISTS%`          → `lists_dir` (with trailing separator)
/// * `%GameFilterTCP%`  → game filter TCP value (empty when disabled)
/// * `%GameFilterUDP%`  → game filter UDP value (empty when disabled)
/// * `%GameFilter%`     → game filter combined value (empty when disabled)
pub fn render_winws_args(raw: &str, bin_dir: &Path, lists_dir: &Path, game: &GameFilter) -> String {
    // Order matters: replace the more specific GameFilter* before %GameFilter%.
    raw.replace("%BIN%", &with_trailing_sep(bin_dir))
        .replace("%LISTS%", &with_trailing_sep(lists_dir))
        .replace("%GameFilterTCP%", game.tcp_value())
        .replace("%GameFilterUDP%", game.udp_value())
        .replace("%GameFilter%", game.combined_value())
}

/// Convenience: parse a strategy file and render its arguments in one step.
pub fn parse_and_render(
    contents: &str,
    bin_dir: &Path,
    lists_dir: &Path,
    game: &GameFilter,
) -> crate::Result<String> {
    let raw = extract_winws_args(contents)
        .ok_or_else(|| crate::Error::Strategy("no winws.exe invocation found".into()))?;
    Ok(render_winws_args(&raw, bin_dir, lists_dir, game))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    const SAMPLE: &str = r#"@echo off
chcp 65001 > nul
cd /d "%~dp0"
call service.bat status_zapret
set "BIN=%~dp0bin\"
set "LISTS=%~dp0lists\"
start "zapret: %~n0" /min "%BIN%winws.exe" --wf-tcp=80,443,%GameFilterTCP% --wf-udp=443,%GameFilterUDP% ^
--filter-udp=443 --hostlist="%LISTS%list-general.txt" --dpi-desync=fake --dpi-desync-repeats=6 --new ^
--filter-tcp=443 --hostlist="%LISTS%list-google.txt" --dpi-desync=multisplit
"#;

    #[test]
    fn joins_continuation_lines() {
        let logical = join_continuations(SAMPLE);
        let winws_line = logical.iter().find(|l| l.contains("winws.exe")).unwrap();
        // All three continued physical lines must be on one logical line.
        assert!(winws_line.contains("--filter-udp=443"));
        assert!(winws_line.contains("--filter-tcp=443"));
        assert!(!winws_line.contains('^'));
    }

    #[test]
    fn extracts_args_after_winws() {
        let args = extract_winws_args(SAMPLE).unwrap();
        assert!(args.starts_with("--wf-tcp=80,443"));
        assert!(!args.contains("winws.exe"));
        assert!(!args.contains("/min"));
    }

    #[test]
    fn returns_none_without_invocation() {
        assert!(extract_winws_args("@echo off\necho nothing here").is_none());
    }

    #[test]
    fn substitutes_paths_and_game_filter_disabled() {
        let bin = PathBuf::from("/opt/zapret/bin");
        let lists = PathBuf::from("/opt/zapret/lists");
        let out = parse_and_render(SAMPLE, &bin, &lists, &GameFilter::disabled()).unwrap();
        let lists_str = with_trailing_sep(&lists);
        assert!(out.contains(&format!("{}list-general.txt", lists_str)));
        assert!(!out.contains("%LISTS%"));
        assert!(!out.contains("%BIN%"));
        // Disabled game filter -> placeholders gone, leaving a trailing comma like upstream.
        assert!(out.contains("--wf-tcp=80,443,"));
        assert!(!out.contains("%GameFilterTCP%"));
    }

    #[test]
    fn substitutes_game_filter_enabled() {
        let bin = PathBuf::from("/opt/zapret/bin");
        let lists = PathBuf::from("/opt/zapret/lists");
        let out = parse_and_render(SAMPLE, &bin, &lists, &GameFilter::enabled()).unwrap();
        assert!(out.contains("--wf-tcp=80,443,1024-65535"));
        assert!(out.contains("--wf-udp=443,1024-65535"));
    }
}
