use anyhow::{Context, Result};
use std::path::PathBuf;

const CONFIG_TEMPLATE: &str = r#"# qq configuration
# Documentation: https://github.com/andrewbsmith/quick-query
#
# API keys are read from environment variables by default:
#   ANTHROPIC_API_KEY, OPENAI_API_KEY, GEMINI_API_KEY
# You can also set them directly in this file (not recommended).

default_profile = "default"

# ── Providers ────────────────────────────────────────────────────
# Uncomment and configure the providers you use.
# Provider type is auto-detected from the section name.

[providers.anthropic]
# api_key = "sk-ant-..."          # or set ANTHROPIC_API_KEY env var
default_model = "claude-sonnet-4-20250514"

# [providers.openai]
# api_key = "sk-..."              # or set OPENAI_API_KEY env var
# default_model = "gpt-4o"

# [providers.gemini]
# api_key = "AIza..."             # or set GEMINI_API_KEY env var
# default_model = "gemini-2.5-flash"

# ── Profiles ─────────────────────────────────────────────────────
# A profile bundles a provider, model, and optional system prompt.
# Switch profiles with: qq -P <name>

[profiles.default]
provider = "anthropic"
"#;

const AGENTS_TEMPLATE: &str = r#"# qq agent configuration (optional)
#
# Override built-in agent settings or define custom agents.
# See examples/agents.toml for full documentation.
#
# Built-in agents: pm, chat, explore, researcher, coder,
#                  reviewer, summarizer, planner, writer

# Example: increase coder turn limit
# [builtin.coder]
# max_turns = 100
"#;

pub fn run() -> Result<()> {
    let config_dir = dirs::config_dir()
        .ok_or_else(|| anyhow::anyhow!("Could not determine config directory"))?
        .join("qq");

    let config_path = config_dir.join("config.toml");
    let agents_path = config_dir.join("agents.toml");

    // Create directory if needed
    std::fs::create_dir_all(&config_dir)
        .with_context(|| format!("Failed to create config directory: {}", config_dir.display()))?;

    // Check for existing files
    let config_exists = config_path.exists();
    let agents_exists = agents_path.exists();

    if config_exists || agents_exists {
        println!("Existing config files found:");
        if config_exists {
            println!("  {}", config_path.display());
        }
        if agents_exists {
            println!("  {}", agents_path.display());
        }
        print!("\nOverwrite? (Existing files will be backed up) [y/N] ");

        // Flush stdout so the prompt appears before reading
        use std::io::Write;
        std::io::stdout().flush()?;

        let mut input = String::new();
        std::io::stdin().read_line(&mut input)?;

        if !input.trim().eq_ignore_ascii_case("y") {
            println!("Setup cancelled.");
            return Ok(());
        }

        // Back up existing files
        if config_exists {
            backup_file(&config_path)?;
        }
        if agents_exists {
            backup_file(&agents_path)?;
        }
    }

    // Write config files
    std::fs::write(&config_path, CONFIG_TEMPLATE)
        .with_context(|| format!("Failed to write {}", config_path.display()))?;
    println!("Created {}", config_path.display());

    std::fs::write(&agents_path, AGENTS_TEMPLATE)
        .with_context(|| format!("Failed to write {}", agents_path.display()))?;
    println!("Created {}", agents_path.display());

    println!("\nNext steps:");
    println!("  1. Set your API key:  export ANTHROPIC_API_KEY=\"sk-ant-...\"");
    println!("  2. Start chatting:    qq");
    println!("  3. Or run a prompt:   qq -p \"hello world\"");

    Ok(())
}

/// Back up a file to <name>.bak, appending a timestamp if .bak already exists.
fn backup_file(path: &PathBuf) -> Result<()> {
    let mut backup = path.with_extension("toml.bak");

    if backup.exists() {
        let timestamp = chrono::Local::now().format("%Y%m%d-%H%M%S");
        let name = format!("toml.bak.{}", timestamp);
        backup = path.with_extension(name);
    }

    std::fs::rename(path, &backup)
        .with_context(|| format!("Failed to back up {} to {}", path.display(), backup.display()))?;
    println!("  Backed up to {}", backup.display());

    Ok(())
}
