//! Shell command parsing for pipeline extraction and tokenization.

/// Tools that support subcommand-level permission classification.
/// When a command starts with one of these, the first non-flag word after
/// the tool name is extracted as `tool-subcmd` (e.g., `cargo build` → `cargo-build`).
const SUBCOMMAND_TOOLS: &[&str] = &[
    "cargo", "git", "npm", "npx", "yarn", "pnpm", "pip", "pip3", "poetry",
];

/// Extract command names from a shell command string.
///
/// Splits on `|`, `&&`, `||`, `;` (outside of quotes) and returns
/// the first word (command name) from each segment.
///
/// Tools listed in [`SUBCOMMAND_TOOLS`] get subcommand extraction:
/// `cargo build` → `cargo-build`, `git log` → `git-log`, etc.
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

/// Parse a heredoc delimiter from the characters after `<<` or `<<-`.
/// Returns `Some((delimiter, chars_consumed))` or `None` if no valid delimiter found.
fn parse_heredoc_delimiter(chars: &[char], start: usize) -> Option<(String, usize)> {
    let mut pos = start;

    // Skip whitespace between << and delimiter
    while pos < chars.len() && (chars[pos] == ' ' || chars[pos] == '\t') {
        pos += 1;
    }

    if pos >= chars.len() || chars[pos] == '\n' {
        return None;
    }

    let delimiter;
    match chars[pos] {
        '\'' => {
            // Single-quoted: << 'EOF'
            pos += 1; // skip opening quote
            let word_start = pos;
            while pos < chars.len() && chars[pos] != '\'' {
                pos += 1;
            }
            if pos >= chars.len() {
                return None; // unterminated quote
            }
            delimiter = chars[word_start..pos].iter().collect::<String>();
            pos += 1; // skip closing quote
        }
        '"' => {
            // Double-quoted: << "EOF"
            pos += 1;
            let word_start = pos;
            while pos < chars.len() && chars[pos] != '"' {
                pos += 1;
            }
            if pos >= chars.len() {
                return None;
            }
            delimiter = chars[word_start..pos].iter().collect::<String>();
            pos += 1;
        }
        _ => {
            // Bare word: << EOF
            let word_start = pos;
            while pos < chars.len()
                && !chars[pos].is_whitespace()
                && chars[pos] != ';'
                && chars[pos] != '&'
                && chars[pos] != '|'
            {
                pos += 1;
            }
            if pos == word_start {
                return None;
            }
            delimiter = chars[word_start..pos].iter().collect::<String>();
        }
    }

    if delimiter.is_empty() {
        return None;
    }

    Some((delimiter, pos - start))
}

/// Return the last line of `s` (from after the last `\n` to end).
fn extract_last_line(s: &str) -> &str {
    match s.rfind('\n') {
        Some(pos) => &s[pos + 1..],
        None => s,
    }
}

/// Split a command string on pipeline operators (`|`, `&&`, `||`, `;`)
/// while respecting quotes and heredoc syntax.
fn split_pipeline(input: &str) -> Vec<String> {
    let mut segments = Vec::new();
    let mut current = String::new();
    let mut in_single_quote = false;
    let mut in_double_quote = false;
    let mut heredoc_delimiter: Option<String> = None;
    let mut heredoc_strip_tabs = false;
    let chars: Vec<char> = input.chars().collect();
    let mut i = 0;

    while i < chars.len() {
        let ch = chars[i];

        // Heredoc body mode: consume everything until we see the delimiter on its own line
        if let Some(ref delim) = heredoc_delimiter {
            current.push(ch);
            if ch == '\n' || i + 1 == chars.len() {
                let last_line = extract_last_line(&current);
                let mut check_line = last_line;
                // Strip trailing \r for \r\n line endings
                if let Some(stripped) = check_line.strip_suffix('\r') {
                    check_line = stripped;
                }
                // For <<-, strip leading tabs before comparison
                if heredoc_strip_tabs {
                    check_line = check_line.trim_start_matches('\t');
                }
                if check_line == delim.as_str() {
                    heredoc_delimiter = None;
                    heredoc_strip_tabs = false;
                }
            }
            i += 1;
            continue;
        }

        match ch {
            '\'' if !in_double_quote => {
                in_single_quote = !in_single_quote;
                current.push(ch);
            }
            '"' if !in_single_quote => {
                in_double_quote = !in_double_quote;
                current.push(ch);
            }
            '<' if !in_single_quote && !in_double_quote => {
                if i + 1 < chars.len() && chars[i + 1] == '<' {
                    if i + 2 < chars.len() && chars[i + 2] == '<' {
                        // <<< here-string: push all three chars, no body to skip
                        current.push('<');
                        current.push('<');
                        current.push('<');
                        i += 2;
                    } else {
                        // << or <<-
                        current.push('<');
                        current.push('<');
                        let mut after = i + 2;
                        let strip_tabs = after < chars.len() && chars[after] == '-';
                        if strip_tabs {
                            current.push('-');
                            after += 1;
                        }
                        if let Some((delim, consumed)) =
                            parse_heredoc_delimiter(&chars, after)
                        {
                            // Push the delimiter text into current segment
                            for &c in &chars[after..after + consumed] {
                                current.push(c);
                            }
                            heredoc_delimiter = Some(delim);
                            heredoc_strip_tabs = strip_tabs;
                            i = after + consumed - 1; // -1 because loop does i += 1
                        } else {
                            // Malformed heredoc, just treat as regular chars
                            i += 1; // skip second <, loop will advance past
                        }
                    }
                } else {
                    // Single < (redirect), push as-is
                    current.push(ch);
                }
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
/// Returns `tool-<subcommand>` for tools in [`SUBCOMMAND_TOOLS`].
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

    // Handle subcommand extraction for supported tools
    if SUBCOMMAND_TOOLS.contains(&base_cmd) {
        let after_cmd = remaining[first_word_end..].trim_start();
        if !after_cmd.is_empty() {
            let subcmd_end = after_cmd
                .find(|c: char| c.is_whitespace())
                .unwrap_or(after_cmd.len());
            let subcmd = &after_cmd[..subcmd_end];
            // Only treat as subcommand if it doesn't start with -
            if !subcmd.starts_with('-') {
                return format!("{}-{}", base_cmd, subcmd);
            }
        }
        return base_cmd.to_string();
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
            vec!["cargo-build", "cargo-test"]
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
            vec!["cargo-build"]
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
            vec!["mkdir", "cargo-build", "head"]
        );
    }

    #[test]
    fn test_cargo_subcommands() {
        assert_eq!(
            extract_commands("cargo build --release").unwrap(),
            vec!["cargo-build"]
        );
        assert_eq!(
            extract_commands("cargo test -- --nocapture").unwrap(),
            vec!["cargo-test"]
        );
        assert_eq!(
            extract_commands("cargo clippy -- -D warnings").unwrap(),
            vec!["cargo-clippy"]
        );
        assert_eq!(
            extract_commands("cargo run --bin myapp").unwrap(),
            vec!["cargo-run"]
        );
        // Flag-only: no subcommand extracted
        assert_eq!(
            extract_commands("cargo --version").unwrap(),
            vec!["cargo"]
        );
    }

    #[test]
    fn test_npm_subcommands() {
        assert_eq!(
            extract_commands("npm install express").unwrap(),
            vec!["npm-install"]
        );
        assert_eq!(
            extract_commands("npm test").unwrap(),
            vec!["npm-test"]
        );
        assert_eq!(
            extract_commands("npm run build").unwrap(),
            vec!["npm-run"]
        );
    }

    #[test]
    fn test_pip_subcommands() {
        assert_eq!(
            extract_commands("pip install requests").unwrap(),
            vec!["pip-install"]
        );
        assert_eq!(
            extract_commands("pip3 list --outdated").unwrap(),
            vec!["pip3-list"]
        );
        assert_eq!(
            extract_commands("pip freeze").unwrap(),
            vec!["pip-freeze"]
        );
    }

    #[test]
    fn test_yarn_pnpm_poetry_subcommands() {
        assert_eq!(
            extract_commands("yarn add lodash").unwrap(),
            vec!["yarn-add"]
        );
        assert_eq!(
            extract_commands("pnpm install").unwrap(),
            vec!["pnpm-install"]
        );
        assert_eq!(
            extract_commands("poetry show --tree").unwrap(),
            vec!["poetry-show"]
        );
    }

    #[test]
    fn test_non_subcommand_tools_unchanged() {
        // Tools NOT in SUBCOMMAND_TOOLS should stay as-is
        assert_eq!(
            extract_commands("make clean").unwrap(),
            vec!["make"]
        );
        assert_eq!(
            extract_commands("python script.py").unwrap(),
            vec!["python"]
        );
        assert_eq!(
            extract_commands("cmake --build .").unwrap(),
            vec!["cmake"]
        );
    }

    #[test]
    fn test_npx_subcommand() {
        assert_eq!(
            extract_commands("npx prettier --write .").unwrap(),
            vec!["npx-prettier"]
        );
    }

    // --- Heredoc tests ---

    #[test]
    fn test_heredoc_basic() {
        assert_eq!(
            extract_commands("cat << EOF\nhello\nEOF").unwrap(),
            vec!["cat"]
        );
    }

    #[test]
    fn test_heredoc_single_quoted_delimiter() {
        assert_eq!(
            extract_commands("cat > file.md << 'EOF'\ncontent | pipes\nEOF").unwrap(),
            vec!["cat"]
        );
    }

    #[test]
    fn test_heredoc_double_quoted_delimiter() {
        assert_eq!(
            extract_commands("cat << \"END\"\nsome | content\nEND").unwrap(),
            vec!["cat"]
        );
    }

    #[test]
    fn test_heredoc_strip_tabs() {
        assert_eq!(
            extract_commands("cat <<- EOF\n\thello\n\tEOF").unwrap(),
            vec!["cat"]
        );
    }

    #[test]
    fn test_heredoc_with_redirect() {
        assert_eq!(
            extract_commands("cat > out.txt << 'EOF'\nline with | pipe\nEOF").unwrap(),
            vec!["cat"]
        );
    }

    #[test]
    fn test_heredoc_pipe_before() {
        assert_eq!(
            extract_commands("generate | cat << 'EOF'\nstuff\nEOF").unwrap(),
            vec!["generate", "cat"]
        );
    }

    #[test]
    fn test_here_string() {
        assert_eq!(
            extract_commands("cat <<< 'hello world'").unwrap(),
            vec!["cat"]
        );
    }

    #[test]
    fn test_heredoc_empty_body() {
        assert_eq!(
            extract_commands("cat << EOF\nEOF").unwrap(),
            vec!["cat"]
        );
    }

    #[test]
    fn test_heredoc_real_world_bug() {
        // This is the scenario that triggered the bug: document content with
        // shell operators being parsed as pipeline separators
        assert_eq!(
            extract_commands("cat > prd.md << 'EOF'\n# Doc with | && ; ||\nEOF").unwrap(),
            vec!["cat"]
        );
    }

    #[test]
    fn test_heredoc_single_segment() {
        let segments = split_pipeline("cat > file << 'EOF'\nline | pipe && and\nEOF");
        assert_eq!(segments.len(), 1);
    }
}
