//! Preference tools for persistent user preference storage.

use async_trait::async_trait;
use rusqlite::{Connection, params};
use serde::Deserialize;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use qq_core::{Error, PropertySchema, Tool, ToolDefinition, ToolOutput, ToolParameters};

/// Memory storage backed by SQLite
pub struct MemoryStore {
    conn: Arc<Mutex<Connection>>,
}

impl MemoryStore {
    pub fn new(db_path: impl Into<PathBuf>) -> Result<Self, Error> {
        let path = db_path.into();

        // Create parent directories if needed
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| Error::tool("memory", format!("Failed to create directory: {}", e)))?;
        }

        let conn = Connection::open(&path)
            .map_err(|e| Error::tool("memory", format!("Failed to open database: {}", e)))?;

        // Create table if it doesn't exist
        conn.execute(
            "CREATE TABLE IF NOT EXISTS memories (
                name TEXT PRIMARY KEY,
                value TEXT NOT NULL,
                created_at TEXT DEFAULT CURRENT_TIMESTAMP,
                updated_at TEXT DEFAULT CURRENT_TIMESTAMP
            )",
            [],
        )
        .map_err(|e| Error::tool("memory", format!("Failed to create table: {}", e)))?;

        Ok(Self {
            conn: Arc::new(Mutex::new(conn)),
        })
    }

    pub fn in_memory() -> Result<Self, Error> {
        let conn = Connection::open_in_memory()
            .map_err(|e| Error::tool("memory", format!("Failed to create in-memory database: {}", e)))?;

        conn.execute(
            "CREATE TABLE IF NOT EXISTS memories (
                name TEXT PRIMARY KEY,
                value TEXT NOT NULL,
                created_at TEXT DEFAULT CURRENT_TIMESTAMP,
                updated_at TEXT DEFAULT CURRENT_TIMESTAMP
            )",
            [],
        )
        .map_err(|e| Error::tool("memory", format!("Failed to create table: {}", e)))?;

        Ok(Self {
            conn: Arc::new(Mutex::new(conn)),
        })
    }

    fn get(&self, name: &str) -> Result<Option<String>, Error> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn
            .prepare("SELECT value FROM memories WHERE name = ?")
            .map_err(|e| Error::tool("memory", e.to_string()))?;

        let result = stmt
            .query_row(params![name], |row| row.get(0))
            .ok();

        Ok(result)
    }

    fn set(&self, name: &str, value: &str) -> Result<(), Error> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT OR REPLACE INTO memories (name, value, updated_at) VALUES (?, ?, CURRENT_TIMESTAMP)",
            params![name, value],
        )
        .map_err(|e| Error::tool("memory", format!("Failed to save memory: {}", e)))?;

        Ok(())
    }

    fn delete(&self, name: &str) -> Result<bool, Error> {
        let conn = self.conn.lock().unwrap();
        let rows = conn
            .execute("DELETE FROM memories WHERE name = ?", params![name])
            .map_err(|e| Error::tool("memory", format!("Failed to delete memory: {}", e)))?;

        Ok(rows > 0)
    }

    fn list(&self) -> Result<Vec<String>, Error> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn
            .prepare("SELECT name FROM memories ORDER BY name")
            .map_err(|e| Error::tool("memory", e.to_string()))?;

        let names: Vec<String> = stmt
            .query_map([], |row| row.get(0))
            .map_err(|e| Error::tool("memory", e.to_string()))?
            .filter_map(|r| r.ok())
            .collect();

        Ok(names)
    }
}

// =============================================================================
// Read Preference Tool
// =============================================================================

pub struct ReadPreferenceTool {
    store: Arc<MemoryStore>,
}

impl ReadPreferenceTool {
    pub fn new(store: Arc<MemoryStore>) -> Self {
        Self { store }
    }
}

#[derive(Deserialize)]
struct ReadPreferenceArgs {
    name: String,
}

#[async_trait]
impl Tool for ReadPreferenceTool {
    fn name(&self) -> &str {
        "read_preference"
    }

    fn description(&self) -> &str {
        "Read a stored user preference by name. Returns the value if found."
    }

    fn definition(&self) -> ToolDefinition {
        ToolDefinition::new(self.name(), self.description()).with_parameters(
            ToolParameters::new()
                .add_property("name", PropertySchema::string("Name of the user preference to read (e.g., 'coding_style', 'preferred_language')"), true),
        )
    }

    async fn execute(&self, arguments: serde_json::Value) -> Result<ToolOutput, Error> {
        let args: ReadPreferenceArgs = serde_json::from_value(arguments)
            .map_err(|e| Error::tool("read_preference", format!("Invalid arguments: {}", e)))?;

        match self.store.get(&args.name)? {
            Some(value) => Ok(ToolOutput::success(value)),
            None => Ok(ToolOutput::success(format!("Preference '{}' not found", args.name))),
        }
    }
}

// =============================================================================
// Update Preference Tool
// =============================================================================

pub struct UpdatePreferenceTool {
    store: Arc<MemoryStore>,
}

impl UpdatePreferenceTool {
    pub fn new(store: Arc<MemoryStore>) -> Self {
        Self { store }
    }
}

#[derive(Deserialize)]
struct UpdatePreferenceArgs {
    name: String,
    value: String,
}

#[async_trait]
impl Tool for UpdatePreferenceTool {
    fn name(&self) -> &str {
        "update_preference"
    }

    fn description(&self) -> &str {
        "Store or update a user preference that persists across sessions. Use ONLY for long-lived facts about the user \u{2014} coding style, preferred frameworks, name, communication preferences. Do NOT use for task-specific data, intermediate results, or working notes \u{2014} write those to /tmp files instead."
    }

    fn definition(&self) -> ToolDefinition {
        ToolDefinition::new(self.name(), self.description()).with_parameters(
            ToolParameters::new()
                .add_property("name", PropertySchema::string("Descriptive key for the user preference (e.g., 'indent_style', 'preferred_test_framework', 'user_name')"), true)
                .add_property("value", PropertySchema::string("The preference value to store (e.g., 'tabs', 'pytest', 'Andrew')"), true),
        )
    }

    async fn execute(&self, arguments: serde_json::Value) -> Result<ToolOutput, Error> {
        let args: UpdatePreferenceArgs = serde_json::from_value(arguments)
            .map_err(|e| Error::tool("update_preference", format!("Invalid arguments: {}", e)))?;

        self.store.set(&args.name, &args.value)?;
        Ok(ToolOutput::success(format!("Preference '{}' saved", args.name)))
    }
}

// =============================================================================
// Delete Preference Tool
// =============================================================================

pub struct DeletePreferenceTool {
    store: Arc<MemoryStore>,
}

impl DeletePreferenceTool {
    pub fn new(store: Arc<MemoryStore>) -> Self {
        Self { store }
    }
}

#[derive(Deserialize)]
struct DeletePreferenceArgs {
    name: String,
}

#[async_trait]
impl Tool for DeletePreferenceTool {
    fn name(&self) -> &str {
        "delete_preference"
    }

    fn description(&self) -> &str {
        "Delete a stored user preference by name."
    }

    fn definition(&self) -> ToolDefinition {
        ToolDefinition::new(self.name(), self.description()).with_parameters(
            ToolParameters::new()
                .add_property("name", PropertySchema::string("Name of the user preference to delete"), true),
        )
    }

    async fn execute(&self, arguments: serde_json::Value) -> Result<ToolOutput, Error> {
        let args: DeletePreferenceArgs = serde_json::from_value(arguments)
            .map_err(|e| Error::tool("delete_preference", format!("Invalid arguments: {}", e)))?;

        if self.store.delete(&args.name)? {
            Ok(ToolOutput::success(format!("Preference '{}' deleted", args.name)))
        } else {
            Ok(ToolOutput::success(format!("Preference '{}' not found", args.name)))
        }
    }
}

// =============================================================================
// List Preferences Tool
// =============================================================================

pub struct ListPreferencesTool {
    store: Arc<MemoryStore>,
}

impl ListPreferencesTool {
    pub fn new(store: Arc<MemoryStore>) -> Self {
        Self { store }
    }
}

#[async_trait]
impl Tool for ListPreferencesTool {
    fn name(&self) -> &str {
        "list_preferences"
    }

    fn description(&self) -> &str {
        "List all stored user preference names."
    }

    fn definition(&self) -> ToolDefinition {
        ToolDefinition::new(self.name(), self.description())
            .with_parameters(ToolParameters::new())
    }

    async fn execute(&self, _arguments: serde_json::Value) -> Result<ToolOutput, Error> {
        let names = self.store.list()?;

        if names.is_empty() {
            Ok(ToolOutput::success("No preferences stored"))
        } else {
            Ok(ToolOutput::success(names.join("\n")))
        }
    }
}

// =============================================================================
// Factory functions
// =============================================================================

/// Create all preference tools with a shared store (boxed version)
pub fn create_preference_tools(store: Arc<MemoryStore>) -> Vec<Box<dyn Tool>> {
    vec![
        Box::new(ReadPreferenceTool::new(store.clone())),
        Box::new(UpdatePreferenceTool::new(store.clone())),
        Box::new(DeletePreferenceTool::new(store.clone())),
        Box::new(ListPreferencesTool::new(store)),
    ]
}

/// Create all preference tools with a shared store (Arc version)
pub fn create_preference_tools_arc(store: Arc<MemoryStore>) -> Vec<Arc<dyn Tool>> {
    vec![
        Arc::new(ReadPreferenceTool::new(store.clone())),
        Arc::new(UpdatePreferenceTool::new(store.clone())),
        Arc::new(DeletePreferenceTool::new(store.clone())),
        Arc::new(ListPreferencesTool::new(store)),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_preference_operations() {
        let store = Arc::new(MemoryStore::in_memory().unwrap());

        // Update preference
        let update_tool = UpdatePreferenceTool::new(store.clone());
        let result = update_tool
            .execute(serde_json::json!({"name": "test", "value": "hello"}))
            .await
            .unwrap();
        assert!(!result.is_error);

        // Read preference
        let read_tool = ReadPreferenceTool::new(store.clone());
        let result = read_tool
            .execute(serde_json::json!({"name": "test"}))
            .await
            .unwrap();
        assert_eq!(result.content, "hello");

        // List preferences
        let list_tool = ListPreferencesTool::new(store.clone());
        let result = list_tool.execute(serde_json::json!({})).await.unwrap();
        assert!(result.content.contains("test"));

        // Delete preference
        let delete_tool = DeletePreferenceTool::new(store.clone());
        let result = delete_tool
            .execute(serde_json::json!({"name": "test"}))
            .await
            .unwrap();
        assert!(result.content.contains("deleted"));
    }
}
