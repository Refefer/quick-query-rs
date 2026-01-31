//! Memory tools for persistent key-value storage.

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
// Read Memory Tool
// =============================================================================

pub struct ReadMemoryTool {
    store: Arc<MemoryStore>,
}

impl ReadMemoryTool {
    pub fn new(store: Arc<MemoryStore>) -> Self {
        Self { store }
    }
}

#[derive(Deserialize)]
struct ReadMemoryArgs {
    name: String,
}

#[async_trait]
impl Tool for ReadMemoryTool {
    fn name(&self) -> &str {
        "read_memory"
    }

    fn description(&self) -> &str {
        "Read a stored memory by name. Returns the value if found."
    }

    fn definition(&self) -> ToolDefinition {
        ToolDefinition::new(self.name(), self.description()).with_parameters(
            ToolParameters::new()
                .add_property("name", PropertySchema::string("Name of the memory to read"), true),
        )
    }

    async fn execute(&self, arguments: serde_json::Value) -> Result<ToolOutput, Error> {
        let args: ReadMemoryArgs = serde_json::from_value(arguments)
            .map_err(|e| Error::tool("read_memory", format!("Invalid arguments: {}", e)))?;

        match self.store.get(&args.name)? {
            Some(value) => Ok(ToolOutput::success(value)),
            None => Ok(ToolOutput::success(format!("Memory '{}' not found", args.name))),
        }
    }
}

// =============================================================================
// Add Memory Tool
// =============================================================================

pub struct AddMemoryTool {
    store: Arc<MemoryStore>,
}

impl AddMemoryTool {
    pub fn new(store: Arc<MemoryStore>) -> Self {
        Self { store }
    }
}

#[derive(Deserialize)]
struct AddMemoryArgs {
    name: String,
    value: String,
}

#[async_trait]
impl Tool for AddMemoryTool {
    fn name(&self) -> &str {
        "add_memory"
    }

    fn description(&self) -> &str {
        "Store a memory with a name and value. Overwrites if the name already exists."
    }

    fn definition(&self) -> ToolDefinition {
        ToolDefinition::new(self.name(), self.description()).with_parameters(
            ToolParameters::new()
                .add_property("name", PropertySchema::string("Name/key for the memory"), true)
                .add_property("value", PropertySchema::string("Value to store"), true),
        )
    }

    async fn execute(&self, arguments: serde_json::Value) -> Result<ToolOutput, Error> {
        let args: AddMemoryArgs = serde_json::from_value(arguments)
            .map_err(|e| Error::tool("add_memory", format!("Invalid arguments: {}", e)))?;

        self.store.set(&args.name, &args.value)?;
        Ok(ToolOutput::success(format!("Memory '{}' saved", args.name)))
    }
}

// =============================================================================
// Delete Memory Tool
// =============================================================================

pub struct DeleteMemoryTool {
    store: Arc<MemoryStore>,
}

impl DeleteMemoryTool {
    pub fn new(store: Arc<MemoryStore>) -> Self {
        Self { store }
    }
}

#[derive(Deserialize)]
struct DeleteMemoryArgs {
    name: String,
}

#[async_trait]
impl Tool for DeleteMemoryTool {
    fn name(&self) -> &str {
        "delete_memory"
    }

    fn description(&self) -> &str {
        "Delete a stored memory by name."
    }

    fn definition(&self) -> ToolDefinition {
        ToolDefinition::new(self.name(), self.description()).with_parameters(
            ToolParameters::new()
                .add_property("name", PropertySchema::string("Name of the memory to delete"), true),
        )
    }

    async fn execute(&self, arguments: serde_json::Value) -> Result<ToolOutput, Error> {
        let args: DeleteMemoryArgs = serde_json::from_value(arguments)
            .map_err(|e| Error::tool("delete_memory", format!("Invalid arguments: {}", e)))?;

        if self.store.delete(&args.name)? {
            Ok(ToolOutput::success(format!("Memory '{}' deleted", args.name)))
        } else {
            Ok(ToolOutput::success(format!("Memory '{}' not found", args.name)))
        }
    }
}

// =============================================================================
// List Memories Tool
// =============================================================================

pub struct ListMemoriesTool {
    store: Arc<MemoryStore>,
}

impl ListMemoriesTool {
    pub fn new(store: Arc<MemoryStore>) -> Self {
        Self { store }
    }
}

#[async_trait]
impl Tool for ListMemoriesTool {
    fn name(&self) -> &str {
        "list_memories"
    }

    fn description(&self) -> &str {
        "List all stored memory names."
    }

    fn definition(&self) -> ToolDefinition {
        ToolDefinition::new(self.name(), self.description())
            .with_parameters(ToolParameters::new())
    }

    async fn execute(&self, _arguments: serde_json::Value) -> Result<ToolOutput, Error> {
        let names = self.store.list()?;

        if names.is_empty() {
            Ok(ToolOutput::success("No memories stored"))
        } else {
            Ok(ToolOutput::success(names.join("\n")))
        }
    }
}

// =============================================================================
// Factory function
// =============================================================================

/// Create all memory tools with a shared store
pub fn create_memory_tools(store: Arc<MemoryStore>) -> Vec<Box<dyn Tool>> {
    vec![
        Box::new(ReadMemoryTool::new(store.clone())),
        Box::new(AddMemoryTool::new(store.clone())),
        Box::new(DeleteMemoryTool::new(store.clone())),
        Box::new(ListMemoriesTool::new(store)),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_memory_operations() {
        let store = Arc::new(MemoryStore::in_memory().unwrap());

        // Add memory
        let add_tool = AddMemoryTool::new(store.clone());
        let result = add_tool
            .execute(serde_json::json!({"name": "test", "value": "hello"}))
            .await
            .unwrap();
        assert!(!result.is_error);

        // Read memory
        let read_tool = ReadMemoryTool::new(store.clone());
        let result = read_tool
            .execute(serde_json::json!({"name": "test"}))
            .await
            .unwrap();
        assert_eq!(result.content, "hello");

        // List memories
        let list_tool = ListMemoriesTool::new(store.clone());
        let result = list_tool.execute(serde_json::json!({})).await.unwrap();
        assert!(result.content.contains("test"));

        // Delete memory
        let delete_tool = DeleteMemoryTool::new(store.clone());
        let result = delete_tool
            .execute(serde_json::json!({"name": "test"}))
            .await
            .unwrap();
        assert!(result.content.contains("deleted"));
    }
}
