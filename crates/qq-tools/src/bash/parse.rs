//! Shell command parsing for pipeline extraction and tokenization.

/// Extract command names from a shell command string.
///
/// Splits on `|`, `&&`, `||`, `;` (outside of quotes) and returns
/// the first word (command name) from each segment.
///
/// Git subcommands are returned as `git-<subcommand>` for tier classification.
pub fn extract_commands(input: &str) -> Result<Vec<String>, String> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return Err("Empty command".to_string());
    }

    let segments = split_pipeline(trimmed);
    let mut commands = Vec::new();

    for segment in &segments {
        let seg = segment.trim();
        if seg.is_empty() {
            continue;
        }
        let cmd = extract_first_command(seg);
        if cmd.is_empty() {
            continue;
        }
        commands.push(cmd);
    }

    if commands.is_empty() {
        return Err("No commands found".to_string());
    }

    Ok(commands)
}

/// Check if a command string contains shell operators (pipes, redirects, etc.)
/// Used by AppLevel fallback to reject complex commands.
pub fn has_shell_operators(input: &str) -> bool {
    let mut in_single_quote = false;
    let mut in_double_quote = false;
    let mut prev = '\0';
    let chars: Vec<char> = input.chars().collect();

    for (i, &ch) in chars.iter().enumerate() {
        match ch {
            '\'' if !in_double_quote && prev != '\\' => {
                in_single_quote = !in_single_quote;
            }
            '"' if !in_single_quote && prev != '\\' => {
                in_double_quote = !in_double_quote;
            }
            '|' | ';' if !in_single_quote && !in_double_quote => {
                return true;
            }
            '&' if !in_single_quote && !in_double_quote => {
                if i + 1 < chars.len() && chars[i + 1] == '&' {
                    return true;
                }
            }
            '>' | '<' if !in_single_quote && !in_double_quote => {
                return true;
            }
            '$' if !in_single_quote && !in_double_quote => {
                // Subshell: $(...)
                if i + 1 < chars.len() && chars[i + 1] == '(' {
                    return true;
                }
            }
            '`' if !in_single_quote && !in_double_quote => {
                return true;
            }
            _ => {}
        }
        prev = ch;
    }

    false
}

/// Tokenize a simple command string (no pipes/redirects) into arguments.
/// Respects single/double quotes and backslash escapes.
pub fn tokenize(input: &str) -> Result<Vec<String>, String> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return Err("Empty command".to_string());
    }

    let mut tokens = Vec::new();
    let mut current = String::new();
    let mut in_single_quote = false;
    let mut in_double_quote = false;
    let chars: Vec<char> = trimmed.chars().collect();
    let mut i = 0;

    while i < chars.len() {
        let ch = chars[i];

        if in_single_quote {
            if ch == '\'' {
                in_single_quote = false;
            } else {
                current.push(ch);
            }
        } else if in_double_quote {
            if ch == '\\' && i + 1 < chars.len() {
                let next = chars[i + 1];
                match next {
                    '"' | '\\' | '$' | '`' => {
                        current.push(next);
                        i += 1;
                    }
                    _ => {
                        current.push(ch);
                    }
                }
            } else if ch == '"' {
                in_double_quote = false;
            } else {
                current.push(ch);
            }
        } else {
            match ch {
                '\'' => {
                    in_single_quote = true;
                }
                '"' => {
                    in_double_quote = true;
                }
                '\\' if i + 1 < chars.len() => {
                    current.push(chars[i + 1]);
                    i += 1;
                }
                ' ' | '\t' => {
                    if !current.is_empty() {
                        tokens.push(std::mem::take(&mut current));
                    }
                }
                _ => {
                    current.push(ch);
                }
            }
        }

        i += 1;
    }

    if in_single_quote || in_double_quote {
        return Err("Unterminated quote".to_string());
    }

    if !current.is_empty() {
        tokens.push(current);
    }

    if tokens.is_empty() {
        return Err("Empty command".to_string());
    }

    Ok(tokens)
}

/// Split a command string on pipeline operators (`|`, `&&`, `||`, `;`)
/// while respecting quotes.
fn split_pipeline(input: &str) -> Vec<String> {
    let mut segments = Vec::new();
    let mut current = String::new();
    let mut in_single_quote = false;
    let mut in_double_quote = false;
    let chars: Vec<char> = input.chars().collect();
    let mut i = 0;

    while i < chars.len() {
        let ch = chars[i];

        match ch {
            '\'' if !in_double_quote => {
                in_single_quote = !in_single_quote;
                current.push(ch);
            }
            '"' if !in_single_quote => {
                in_double_quote = !in_double_quote;
                current.push(ch);
            }
            '|' if !in_single_quote && !in_double_quote => {
                if i + 1 < chars.len() && chars[i + 1] == '|' {
                    // || operator
                    segments.push(std::mem::take(&mut current));
                    i += 1; // skip second |
                } else {
                    // pipe |
                    segments.push(std::mem::take(&mut current));
                }
            }
            '&' if !in_single_quote && !in_double_quote => {
                if i + 1 < chars.len() && chars[i + 1] == '&' {
                    // && operator
                    segments.push(std::mem::take(&mut current));
                    i += 1; // skip second &
                } else {
                    // single & (background) — treat as part of command
                    current.push(ch);
                }
            }
            ';' if !in_single_quote && !in_double_quote => {
                segments.push(std::mem::take(&mut current));
            }
            _ => {
                current.push(ch);
            }
        }

        i += 1;
    }

    if !current.is_empty() {
        segments.push(current);
    }

    segments
}

/// Extract the first command name from a command segment.
/// Handles environment variable assignments (e.g., `FOO=bar cmd`).
/// Returns `git-<subcommand>` for git commands.
fn extract_first_command(segment: &str) -> String {
    let trimmed = segment.trim();

    // Skip leading environment variable assignments (VAR=value)
    let mut remaining = trimmed;
    loop {
        let word_end = remaining
            .find(|c: char| c.is_whitespace())
            .unwrap_or(remaining.len());
        let word = &remaining[..word_end];

        // Check if it's a VAR=value assignment
        if word.contains('=') && !word.starts_with('=') {
            let eq_pos = word.find('=').unwrap();
            let var_name = &word[..eq_pos];
            if var_name.chars().all(|c| c.is_alphanumeric() || c == '_') {
                // Skip this env var assignment
                remaining = remaining[word_end..].trim_start();
                if remaining.is_empty() {
                    // Just an assignment, no command
                    return String::new();
                }
                continue;
            }
        }
        break;
    }

    // Get the first word (command name)
    let first_word_end = remaining
        .find(|c: char| c.is_whitespace())
        .unwrap_or(remaining.len());
    let cmd = &remaining[..first_word_end];

    // Strip path prefix (e.g., /usr/bin/git -> git)
    let base_cmd = cmd.rsplit('/').next().unwrap_or(cmd);

    // Handle git subcommands: "git log" -> "git-log"
    if base_cmd == "git" {
        let after_git = remaining[first_word_end..].trim_start();
        if !after_git.is_empty() {
            let subcmd_end = after_git
                .find(|c: char| c.is_whitespace())
                .unwrap_or(after_git.len());
            let subcmd = &after_git[..subcmd_end];
            // Only treat as subcommand if it doesn't start with -
            if !subcmd.starts_with('-') {
                return format!("git-{}", subcmd);
            }
        }
        return "git".to_string();
    }

    base_cmd.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_simple_command() {
        assert_eq!(extract_commands("ls -la").unwrap(), vec!["ls"]);
    }

    #[test]
    fn test_pipeline() {
        assert_eq!(
            extract_commands("grep TODO src/*.rs | wc -l").unwrap(),
            vec!["grep", "wc"]
        );
    }

    #[test]
    fn test_and_chain() {
        assert_eq!(
            extract_commands("cargo build && cargo test").unwrap(),
            vec!["cargo", "cargo"]
        );
    }

    #[test]
    fn test_or_chain() {
        assert_eq!(
            extract_commands("test -f foo || echo missing").unwrap(),
            vec!["test", "echo"]
        );
    }

    #[test]
    fn test_semicolons() {
        assert_eq!(
            extract_commands("ls; pwd; date").unwrap(),
            vec!["ls", "pwd", "date"]
        );
    }

    #[test]
    fn test_pipe_inside_quotes() {
        assert_eq!(
            extract_commands("grep 'a|b' file").unwrap(),
            vec!["grep"]
        );
    }

    #[test]
    fn test_double_quotes() {
        assert_eq!(
            extract_commands(r#"grep "foo && bar" file"#).unwrap(),
            vec!["grep"]
        );
    }

    #[test]
    fn test_empty_command() {
        assert!(extract_commands("").is_err());
        assert!(extract_commands("   ").is_err());
    }

    #[test]
    fn test_git_subcommands() {
        assert_eq!(
            extract_commands("git log --oneline -10").unwrap(),
            vec!["git-log"]
        );
        assert_eq!(
            extract_commands("git commit -m 'test'").unwrap(),
            vec!["git-commit"]
        );
        assert_eq!(
            extract_commands("git diff HEAD~1 | grep TODO").unwrap(),
            vec!["git-diff", "grep"]
        );
    }

    #[test]
    fn test_env_var_prefix() {
        assert_eq!(
            extract_commands("FOO=bar cargo build").unwrap(),
            vec!["cargo"]
        );
    }

    #[test]
    fn test_absolute_path_command() {
        assert_eq!(
            extract_commands("/usr/bin/git log").unwrap(),
            vec!["git-log"]
        );
    }

    #[test]
    fn test_has_shell_operators() {
        assert!(has_shell_operators("ls | wc"));
        assert!(has_shell_operators("echo foo > file"));
        assert!(has_shell_operators("cat < file"));
        assert!(has_shell_operators("a && b"));
        assert!(has_shell_operators("a; b"));
        assert!(has_shell_operators("echo $(date)"));
        assert!(has_shell_operators("echo `date`"));

        assert!(!has_shell_operators("ls -la"));
        assert!(!has_shell_operators("grep 'a|b' file"));
        assert!(!has_shell_operators(r#"echo "a && b""#));
    }

    #[test]
    fn test_tokenize_simple() {
        assert_eq!(
            tokenize("ls -la /tmp").unwrap(),
            vec!["ls", "-la", "/tmp"]
        );
    }

    #[test]
    fn test_tokenize_quotes() {
        assert_eq!(
            tokenize("grep 'hello world' file.txt").unwrap(),
            vec!["grep", "hello world", "file.txt"]
        );
        assert_eq!(
            tokenize(r#"echo "hello world""#).unwrap(),
            vec!["echo", "hello world"]
        );
    }

    #[test]
    fn test_tokenize_escapes() {
        assert_eq!(
            tokenize(r"echo hello\ world").unwrap(),
            vec!["echo", "hello world"]
        );
    }

    #[test]
    fn test_tokenize_empty() {
        assert!(tokenize("").is_err());
        assert!(tokenize("   ").is_err());
    }

    #[test]
    fn test_tokenize_unterminated_quote() {
        assert!(tokenize("echo 'hello").is_err());
        assert!(tokenize(r#"echo "hello"#).is_err());
    }

    #[test]
    fn test_complex_pipeline() {
        assert_eq!(
            extract_commands("find . -name '*.rs' | xargs grep TODO | sort | uniq -c").unwrap(),
            vec!["find", "xargs", "sort", "uniq"]
        );
    }

    #[test]
    fn test_mixed_operators() {
        assert_eq!(
            extract_commands("mkdir -p out && cargo build 2>&1 | head -50").unwrap(),
            vec!["mkdir", "cargo", "head"]
        );
    }
}
