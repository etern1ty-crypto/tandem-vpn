use std::fs;
use std::path::Path;

const HOSTS_PATH: &str = r#"C:\Windows\System32\drivers\etc\hosts"#;
const MARKER_START: &str = "# --- tandem-vpn zapret start ---";
const MARKER_END: &str = "# --- tandem-vpn zapret end ---";

/// Идемпотентное слияние загруженного файла hosts с системным
pub fn merge_hosts(remote_content: &str) -> Result<(), String> {
    let mut current_content = String::new();
    let hosts_file = Path::new(HOSTS_PATH);

    if hosts_file.exists() {
        current_content =
            fs::read_to_string(hosts_file).map_err(|e| format!("Failed to read hosts: {}", e))?;
    }

    // Удаляем старый блок tandem-vpn, если он есть
    let mut new_content = String::new();
    let mut in_tandem_block = false;

    for line in current_content.lines() {
        if line.trim() == MARKER_START {
            in_tandem_block = true;
            continue;
        }
        if line.trim() == MARKER_END {
            in_tandem_block = false;
            continue;
        }
        if !in_tandem_block {
            new_content.push_str(line);
            new_content.push('\n');
        }
    }

    // Если файл не заканчивался переносом, добавляем его
    if !new_content.is_empty() && !new_content.ends_with('\n') {
        new_content.push('\n');
    }

    // Добавляем новый блок
    new_content.push_str(MARKER_START);
    new_content.push('\n');

    for line in remote_content.lines() {
        // Пропускаем пустые строки и комментарии из удаленного файла (опционально)
        // Но лучше просто добавить все как есть
        new_content.push_str(line);
        new_content.push('\n');
    }

    new_content.push_str(MARKER_END);
    new_content.push('\n');

    fs::write(hosts_file, new_content)
        .map_err(|e| format!("Failed to write hosts (need admin rights?): {}", e))?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    // Для тестов мы не можем использовать C:\Windows, поэтому протестируем саму логику слияния.
    // Вынесем логику в отдельную функцию для тестирования.
    fn merge_logic(current: &str, remote: &str) -> String {
        let mut new_content = String::new();
        let mut in_tandem_block = false;

        for line in current.lines() {
            if line.trim() == MARKER_START {
                in_tandem_block = true;
                continue;
            }
            if line.trim() == MARKER_END {
                in_tandem_block = false;
                continue;
            }
            if !in_tandem_block {
                new_content.push_str(line);
                new_content.push('\n');
            }
        }

        if !new_content.is_empty() && !new_content.ends_with('\n') {
            new_content.push('\n');
        }

        new_content.push_str(MARKER_START);
        new_content.push('\n');
        for line in remote.lines() {
            new_content.push_str(line);
            new_content.push('\n');
        }
        new_content.push_str(MARKER_END);
        new_content.push('\n');

        new_content
    }

    #[test]
    fn test_merge_hosts_empty() {
        let current = "";
        let remote = "127.0.0.1 discord.com";
        let expected = format!("{}\n127.0.0.1 discord.com\n{}\n", MARKER_START, MARKER_END);
        assert_eq!(merge_logic(current, remote), expected);
    }

    #[test]
    fn test_merge_hosts_existing() {
        let current = "127.0.0.1 localhost\n";
        let remote = "127.0.0.1 discord.com";
        let expected = format!(
            "127.0.0.1 localhost\n{}\n127.0.0.1 discord.com\n{}\n",
            MARKER_START, MARKER_END
        );
        assert_eq!(merge_logic(current, remote), expected);
    }

    #[test]
    fn test_merge_hosts_replace() {
        let current = format!(
            "127.0.0.1 localhost\n{}\n127.0.0.1 old.com\n{}\n",
            MARKER_START, MARKER_END
        );
        let remote = "127.0.0.1 new.com";
        let expected = format!(
            "127.0.0.1 localhost\n{}\n127.0.0.1 new.com\n{}\n",
            MARKER_START, MARKER_END
        );
        assert_eq!(merge_logic(&current, remote), expected);
    }
}
