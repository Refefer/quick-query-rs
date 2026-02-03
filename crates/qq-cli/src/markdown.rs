//! Streaming markdown renderer for terminal output.
//!
//! This module provides markdown rendering that works with streaming LLM output,
//! re-rendering content as it arrives to display proper formatting.

use std::io::{self, Write};

use crossterm::{
    cursor::{MoveToColumn, MoveUp},
    style::Color,
    terminal::{Clear, ClearType},
    ExecutableCommand,
};
use termimad::{MadSkin, terminal_size};

/// A streaming markdown renderer that accumulates content and re-renders.
pub struct MarkdownRenderer {
    /// Accumulated content
    content: String,
    /// Number of lines we've rendered (for clearing)
    rendered_lines: u16,
    /// The markdown skin for styling
    skin: MadSkin,
    /// Terminal width
    term_width: usize,
}

impl MarkdownRenderer {
    /// Create a new markdown renderer.
    pub fn new() -> Self {
        let (width, _) = terminal_size();
        let term_width = (width as usize).saturating_sub(2).max(40);

        Self {
            content: String::new(),
            rendered_lines: 0,
            skin: create_skin(),
            term_width,
        }
    }

    /// Add content and re-render.
    pub fn push(&mut self, text: &str) -> io::Result<()> {
        self.content.push_str(text);
        self.render()
    }

    /// Clear previous render and re-render current content.
    fn render(&mut self) -> io::Result<()> {
        let mut stdout = io::stdout();

        // Clear previously rendered lines
        if self.rendered_lines > 0 {
            // Move up and clear each line
            for _ in 0..self.rendered_lines {
                stdout.execute(MoveUp(1))?;
                stdout.execute(Clear(ClearType::CurrentLine))?;
            }
            stdout.execute(MoveToColumn(0))?;
        }

        // Render markdown
        let rendered = self.skin.text(&self.content, Some(self.term_width));
        let output = format!("{}", rendered);

        // Count lines for next clear
        self.rendered_lines = output.lines().count() as u16;

        // Print rendered content
        print!("{}", output);
        stdout.flush()?;

        Ok(())
    }

    /// Finish rendering and add final newlines.
    pub fn finish(&mut self) -> io::Result<()> {
        // Do a final render to ensure everything is displayed
        if !self.content.is_empty() {
            self.render()?;
        }
        println!("\n");
        Ok(())
    }

    /// Get the accumulated content (for saving to session).
    pub fn content(&self) -> &str {
        &self.content
    }

    /// Check if any content has been added.
    pub fn is_empty(&self) -> bool {
        self.content.is_empty()
    }
}

impl Default for MarkdownRenderer {
    fn default() -> Self {
        Self::new()
    }
}

/// Render markdown content to stdout (one-shot, non-streaming).
pub fn render_markdown(content: &str) {
    let (width, _) = terminal_size();
    let term_width = (width as usize).saturating_sub(2).max(40);
    let skin = create_skin();
    let rendered = skin.text(content, Some(term_width));
    println!("{}", rendered);
}

/// Create a styled markdown skin for terminal output.
pub fn create_skin() -> MadSkin {
    let mut skin = MadSkin::default();

    // Customize colors for better terminal appearance
    skin.bold.set_fg(Color::White);
    skin.italic.set_fg(Color::Magenta);
    skin.inline_code.set_fg(Color::Yellow);
    skin.code_block.set_fg(Color::Yellow);

    // Headers
    skin.headers[0].set_fg(Color::Green);
    skin.headers[1].set_fg(Color::Green);
    skin.headers[2].set_fg(Color::Cyan);

    // Lists and quotes
    skin.bullet.set_fg(Color::Cyan);
    skin.quote_mark.set_fg(Color::DarkGrey);

    skin
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_create_skin() {
        let skin = create_skin();
        // Just verify it creates without panic
        let _ = skin.text("# Hello\n\nThis is **bold**.", Some(80));
    }

    #[test]
    fn test_markdown_renderer() {
        let renderer = MarkdownRenderer::new();
        assert!(renderer.is_empty());
    }
}
