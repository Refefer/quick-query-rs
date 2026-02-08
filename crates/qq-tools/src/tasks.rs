//! Task tracking tools for session-scoped work management.
//!
//! Provides in-memory task storage with CRUD operations, designed for
//! the project manager agent to track delegated work items.

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use qq_core::{Error, PropertySchema, Tool, ToolDefinition, ToolOutput, ToolParameters};

// =============================================================================
// Task types
// =============================================================================

/// Status of a tracked task.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskStatus {
    Todo,
    InProgress,
    Done,
    Blocked,
}

impl std::fmt::Display for TaskStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TaskStatus::Todo => write!(f, "todo"),
            TaskStatus::InProgress => write!(f, "in_progress"),
            TaskStatus::Done => write!(f, "done"),
            TaskStatus::Blocked => write!(f, "blocked"),
        }
    }
}

impl TaskStatus {
    fn from_str(s: &str) -> Result<Self, String> {
        match s {
            "todo" => Ok(TaskStatus::Todo),
            "in_progress" => Ok(TaskStatus::InProgress),
            "done" => Ok(TaskStatus::Done),
            "blocked" => Ok(TaskStatus::Blocked),
            other => Err(format!(
                "Invalid status '{}'. Valid values: todo, in_progress, done, blocked",
                other
            )),
        }
    }
}

/// A tracked task.
#[derive(Debug, Clone, Serialize)]
pub struct Task {
    pub id: String,
    pub title: String,
    pub status: TaskStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub assignee: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub blocked_by: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub notes: Vec<String>,
}

// =============================================================================
// TaskStore
// =============================================================================

/// Inner state for the task store.
struct TaskStoreInner {
    tasks: HashMap<String, Task>,
    next_id: u32,
}

/// In-memory task store, session-scoped.
///
/// Thread-safe via `Mutex`. No persistence between sessions.
pub struct TaskStore {
    inner: Mutex<TaskStoreInner>,
}

impl TaskStore {
    pub fn new() -> Self {
        Self {
            inner: Mutex::new(TaskStoreInner {
                tasks: HashMap::new(),
                next_id: 1,
            }),
        }
    }

    /// Format a compact markdown task board summary.
    ///
    /// Returns `None` if no tasks exist. Shows status, id, title, assignee,
    /// blocked_by, blocks (derived), description, and latest note.
    pub fn format_board(&self) -> Option<String> {
        let inner = self.inner.lock().unwrap();

        if inner.tasks.is_empty() {
            return None;
        }

        // Sort tasks by ID numerically
        let mut tasks: Vec<&Task> = inner.tasks.values().collect();
        tasks.sort_by_key(|t| t.id.parse::<u32>().unwrap_or(0));

        // Build reverse blocks map: task_id -> list of tasks it blocks
        let mut blocks_map: HashMap<&str, Vec<&str>> = HashMap::new();
        for task in &tasks {
            for dep_id in &task.blocked_by {
                blocks_map
                    .entry(dep_id.as_str())
                    .or_default()
                    .push(&task.id);
            }
        }

        let mut lines = vec!["## Current Task Board".to_string(), String::new()];

        for task in &tasks {
            let assignee_str = task
                .assignee
                .as_deref()
                .map(|a| format!(" (assignee: {})", a))
                .unwrap_or_default();
            lines.push(format!(
                "- [{}] #{}: {}{}",
                task.status, task.id, task.title, assignee_str
            ));

            if !task.blocked_by.is_empty() {
                let deps: Vec<String> = task.blocked_by.iter().map(|id| format!("#{}", id)).collect();
                lines.push(format!("  blocked by: {}", deps.join(", ")));
            }

            if let Some(blocks) = blocks_map.get(task.id.as_str()) {
                let blocked: Vec<String> = blocks.iter().map(|id| format!("#{}", id)).collect();
                lines.push(format!("  blocks: {}", blocked.join(", ")));
            }

            if let Some(desc) = &task.description {
                lines.push(format!("  description: {}", desc));
            }

            if let Some(note) = task.notes.last() {
                lines.push(format!("  latest note: {}", note));
            }
        }

        Some(lines.join("\n"))
    }
}

impl Default for TaskStore {
    fn default() -> Self {
        Self::new()
    }
}

// =============================================================================
// CreateTaskTool
// =============================================================================

pub struct CreateTaskTool {
    store: Arc<TaskStore>,
}

impl CreateTaskTool {
    pub fn new(store: Arc<TaskStore>) -> Self {
        Self { store }
    }
}

#[derive(Deserialize)]
struct CreateTaskArgs {
    title: String,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    assignee: Option<String>,
    #[serde(default)]
    status: Option<String>,
    #[serde(default)]
    blocked_by: Vec<String>,
}

#[async_trait]
impl Tool for CreateTaskTool {
    fn name(&self) -> &str {
        "create_task"
    }

    fn description(&self) -> &str {
        "Create a new tracked task. Returns the created task as JSON."
    }

    fn tool_description(&self) -> &str {
        "Create a new tracked task for work management."
    }

    fn definition(&self) -> ToolDefinition {
        ToolDefinition::new(self.name(), self.description()).with_parameters(
            ToolParameters::new()
                .add_property("title", PropertySchema::string("Title of the task"), true)
                .add_property(
                    "description",
                    PropertySchema::string("Detailed description of the task"),
                    false,
                )
                .add_property(
                    "assignee",
                    PropertySchema::string("Agent or person assigned to the task"),
                    false,
                )
                .add_property(
                    "status",
                    PropertySchema::string(
                        "Task status: todo, in_progress, done, or blocked (default: todo)",
                    ),
                    false,
                )
                .add_property(
                    "blocked_by",
                    PropertySchema::array(
                        "IDs of prerequisite tasks that must complete before this one",
                        PropertySchema::string("Task ID"),
                    ),
                    false,
                ),
        )
    }

    fn is_blocking(&self) -> bool {
        false
    }

    async fn execute(&self, arguments: serde_json::Value) -> Result<ToolOutput, Error> {
        let args: CreateTaskArgs = serde_json::from_value(arguments)
            .map_err(|e| Error::tool("create_task", format!("Invalid arguments: {}", e)))?;

        let status = match args.status {
            Some(s) => TaskStatus::from_str(&s)
                .map_err(|e| Error::tool("create_task", e))?,
            None => TaskStatus::Todo,
        };

        let mut inner = self.store.inner.lock().unwrap();

        // Validate blocked_by references
        for dep_id in &args.blocked_by {
            if !inner.tasks.contains_key(dep_id) {
                return Ok(ToolOutput::error(format!(
                    "Dependency task '{}' not found",
                    dep_id
                )));
            }
        }

        let id = inner.next_id.to_string();
        inner.next_id += 1;

        let task = Task {
            id: id.clone(),
            title: args.title,
            status,
            assignee: args.assignee,
            description: args.description,
            blocked_by: args.blocked_by,
            notes: Vec::new(),
        };

        let output = serde_json::to_string_pretty(&task)
            .unwrap_or_else(|_| format!("Task '{}' created", id));
        inner.tasks.insert(id, task);

        Ok(ToolOutput::success(output))
    }
}

// =============================================================================
// UpdateTaskTool
// =============================================================================

pub struct UpdateTaskTool {
    store: Arc<TaskStore>,
}

impl UpdateTaskTool {
    pub fn new(store: Arc<TaskStore>) -> Self {
        Self { store }
    }
}

#[derive(Deserialize)]
struct UpdateTaskArgs {
    id: String,
    #[serde(default)]
    title: Option<String>,
    #[serde(default)]
    status: Option<String>,
    #[serde(default)]
    assignee: Option<String>,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    blocked_by: Option<Vec<String>>,
    #[serde(default)]
    add_note: Option<String>,
}

#[async_trait]
impl Tool for UpdateTaskTool {
    fn name(&self) -> &str {
        "update_task"
    }

    fn description(&self) -> &str {
        "Update an existing task's fields. Returns the updated task as JSON."
    }

    fn tool_description(&self) -> &str {
        "Update an existing tracked task."
    }

    fn definition(&self) -> ToolDefinition {
        ToolDefinition::new(self.name(), self.description()).with_parameters(
            ToolParameters::new()
                .add_property("id", PropertySchema::string("ID of the task to update"), true)
                .add_property(
                    "title",
                    PropertySchema::string("New title for the task"),
                    false,
                )
                .add_property(
                    "status",
                    PropertySchema::string(
                        "New status: todo, in_progress, done, or blocked",
                    ),
                    false,
                )
                .add_property(
                    "assignee",
                    PropertySchema::string("New assignee for the task"),
                    false,
                )
                .add_property(
                    "description",
                    PropertySchema::string("New description for the task"),
                    false,
                )
                .add_property(
                    "blocked_by",
                    PropertySchema::array(
                        "Replace dependency list (use empty array to clear)",
                        PropertySchema::string("Task ID"),
                    ),
                    false,
                )
                .add_property(
                    "add_note",
                    PropertySchema::string("Append a progress note to the task"),
                    false,
                ),
        )
    }

    fn is_blocking(&self) -> bool {
        false
    }

    async fn execute(&self, arguments: serde_json::Value) -> Result<ToolOutput, Error> {
        let args: UpdateTaskArgs = serde_json::from_value(arguments)
            .map_err(|e| Error::tool("update_task", format!("Invalid arguments: {}", e)))?;

        let mut inner = self.store.inner.lock().unwrap();

        // Validate blocked_by references before mutating
        if let Some(ref deps) = args.blocked_by {
            for dep_id in deps {
                if dep_id == &args.id {
                    return Ok(ToolOutput::error(
                        "A task cannot depend on itself".to_string(),
                    ));
                }
                if !inner.tasks.contains_key(dep_id) {
                    return Ok(ToolOutput::error(format!(
                        "Dependency task '{}' not found",
                        dep_id
                    )));
                }
            }
        }

        let task = match inner.tasks.get_mut(&args.id) {
            Some(t) => t,
            None => {
                return Ok(ToolOutput::error(format!(
                    "Task with id '{}' not found",
                    args.id
                )));
            }
        };

        if let Some(title) = args.title {
            task.title = title;
        }
        if let Some(status_str) = args.status {
            task.status = TaskStatus::from_str(&status_str)
                .map_err(|e| Error::tool("update_task", e))?;
        }
        if let Some(assignee) = args.assignee {
            task.assignee = Some(assignee);
        }
        if let Some(description) = args.description {
            task.description = Some(description);
        }
        if let Some(deps) = args.blocked_by {
            task.blocked_by = deps;
        }
        if let Some(note) = args.add_note {
            task.notes.push(note);
        }

        let output = serde_json::to_string_pretty(task)
            .unwrap_or_else(|_| format!("Task '{}' updated", args.id));

        Ok(ToolOutput::success(output))
    }
}

// =============================================================================
// ListTasksTool
// =============================================================================

pub struct ListTasksTool {
    store: Arc<TaskStore>,
}

impl ListTasksTool {
    pub fn new(store: Arc<TaskStore>) -> Self {
        Self { store }
    }
}

#[derive(Deserialize)]
struct ListTasksArgs {
    #[serde(default)]
    status: Option<String>,
    #[serde(default)]
    assignee: Option<String>,
}

#[async_trait]
impl Tool for ListTasksTool {
    fn name(&self) -> &str {
        "list_tasks"
    }

    fn description(&self) -> &str {
        "List tracked tasks, optionally filtered by status and/or assignee."
    }

    fn tool_description(&self) -> &str {
        "List tracked tasks with optional filters."
    }

    fn definition(&self) -> ToolDefinition {
        ToolDefinition::new(self.name(), self.description()).with_parameters(
            ToolParameters::new()
                .add_property(
                    "status",
                    PropertySchema::string(
                        "Filter by status: todo, in_progress, done, or blocked",
                    ),
                    false,
                )
                .add_property(
                    "assignee",
                    PropertySchema::string("Filter by assignee name"),
                    false,
                ),
        )
    }

    fn is_blocking(&self) -> bool {
        false
    }

    async fn execute(&self, arguments: serde_json::Value) -> Result<ToolOutput, Error> {
        let args: ListTasksArgs = serde_json::from_value(arguments)
            .map_err(|e| Error::tool("list_tasks", format!("Invalid arguments: {}", e)))?;

        let status_filter = match args.status {
            Some(s) => Some(
                TaskStatus::from_str(&s)
                    .map_err(|e| Error::tool("list_tasks", e))?,
            ),
            None => None,
        };

        let inner = self.store.inner.lock().unwrap();

        let mut tasks: Vec<&Task> = inner
            .tasks
            .values()
            .filter(|t| {
                if let Some(ref sf) = status_filter {
                    if &t.status != sf {
                        return false;
                    }
                }
                if let Some(ref af) = args.assignee {
                    match &t.assignee {
                        Some(a) => {
                            if a != af {
                                return false;
                            }
                        }
                        None => return false,
                    }
                }
                true
            })
            .collect();

        if tasks.is_empty() {
            return Ok(ToolOutput::success("No tasks found.".to_string()));
        }

        // Sort by ID numerically
        tasks.sort_by_key(|t| t.id.parse::<u32>().unwrap_or(0));

        // Build reverse blocks map
        let all_tasks: Vec<&Task> = inner.tasks.values().collect();
        let mut blocks_map: HashMap<&str, Vec<&str>> = HashMap::new();
        for t in &all_tasks {
            for dep_id in &t.blocked_by {
                blocks_map
                    .entry(dep_id.as_str())
                    .or_default()
                    .push(&t.id);
            }
        }

        // Serialize with derived "blocks" field
        let output_values: Vec<serde_json::Value> = tasks
            .iter()
            .map(|task| {
                let mut val = serde_json::to_value(task).unwrap();
                if let Some(blocks) = blocks_map.get(task.id.as_str()) {
                    val.as_object_mut().unwrap().insert(
                        "blocks".to_string(),
                        serde_json::Value::Array(
                            blocks.iter().map(|id| serde_json::Value::String(id.to_string())).collect(),
                        ),
                    );
                }
                val
            })
            .collect();

        let output = serde_json::to_string_pretty(&output_values)
            .unwrap_or_else(|_| "Error serializing tasks".to_string());

        Ok(ToolOutput::success(output))
    }
}

// =============================================================================
// DeleteTaskTool
// =============================================================================

pub struct DeleteTaskTool {
    store: Arc<TaskStore>,
}

impl DeleteTaskTool {
    pub fn new(store: Arc<TaskStore>) -> Self {
        Self { store }
    }
}

#[derive(Deserialize)]
struct DeleteTaskArgs {
    id: String,
}

#[async_trait]
impl Tool for DeleteTaskTool {
    fn name(&self) -> &str {
        "delete_task"
    }

    fn description(&self) -> &str {
        "Delete a tracked task by ID."
    }

    fn tool_description(&self) -> &str {
        "Delete a tracked task."
    }

    fn definition(&self) -> ToolDefinition {
        ToolDefinition::new(self.name(), self.description()).with_parameters(
            ToolParameters::new()
                .add_property("id", PropertySchema::string("ID of the task to delete"), true),
        )
    }

    fn is_blocking(&self) -> bool {
        false
    }

    async fn execute(&self, arguments: serde_json::Value) -> Result<ToolOutput, Error> {
        let args: DeleteTaskArgs = serde_json::from_value(arguments)
            .map_err(|e| Error::tool("delete_task", format!("Invalid arguments: {}", e)))?;

        let mut inner = self.store.inner.lock().unwrap();
        match inner.tasks.remove(&args.id) {
            Some(task) => Ok(ToolOutput::success(format!(
                "Task '{}' ('{}') deleted",
                args.id, task.title
            ))),
            None => Ok(ToolOutput::error(format!(
                "Task with id '{}' not found",
                args.id
            ))),
        }
    }
}

// =============================================================================
// UpdateMyTaskTool
// =============================================================================

/// Scoped task update tool for sub-agents.
///
/// Deliberately limited: can only update status and add notes.
/// Cannot change title, description, assignee, or dependencies.
pub struct UpdateMyTaskTool {
    store: Arc<TaskStore>,
}

impl UpdateMyTaskTool {
    pub fn new(store: Arc<TaskStore>) -> Self {
        Self { store }
    }
}

#[derive(Deserialize)]
struct UpdateMyTaskArgs {
    id: String,
    #[serde(default)]
    status: Option<String>,
    #[serde(default)]
    add_note: Option<String>,
}

#[async_trait]
impl Tool for UpdateMyTaskTool {
    fn name(&self) -> &str {
        "update_my_task"
    }

    fn description(&self) -> &str {
        "Update your assigned task's status or add a progress note. Use this to report progress, mark tasks done, or flag blockers."
    }

    fn tool_description(&self) -> &str {
        "Update your assigned task's status or add a progress note."
    }

    fn definition(&self) -> ToolDefinition {
        ToolDefinition::new(self.name(), self.description()).with_parameters(
            ToolParameters::new()
                .add_property("id", PropertySchema::string("ID of the task to update"), true)
                .add_property(
                    "status",
                    PropertySchema::string(
                        "New status: todo, in_progress, done, or blocked",
                    ),
                    false,
                )
                .add_property(
                    "add_note",
                    PropertySchema::string("Append a progress note (e.g., findings, blockers, completion summary)"),
                    false,
                ),
        )
    }

    fn is_blocking(&self) -> bool {
        false
    }

    async fn execute(&self, arguments: serde_json::Value) -> Result<ToolOutput, Error> {
        let args: UpdateMyTaskArgs = serde_json::from_value(arguments)
            .map_err(|e| Error::tool("update_my_task", format!("Invalid arguments: {}", e)))?;

        if args.status.is_none() && args.add_note.is_none() {
            return Ok(ToolOutput::error(
                "At least one of 'status' or 'add_note' must be provided".to_string(),
            ));
        }

        let mut inner = self.store.inner.lock().unwrap();
        let task = match inner.tasks.get_mut(&args.id) {
            Some(t) => t,
            None => {
                return Ok(ToolOutput::error(format!(
                    "Task with id '{}' not found",
                    args.id
                )));
            }
        };

        if let Some(status_str) = args.status {
            task.status = TaskStatus::from_str(&status_str)
                .map_err(|e| Error::tool("update_my_task", e))?;
        }
        if let Some(note) = args.add_note {
            task.notes.push(note);
        }

        let output = serde_json::to_string_pretty(task)
            .unwrap_or_else(|_| format!("Task '{}' updated", args.id));

        Ok(ToolOutput::success(output))
    }
}

// =============================================================================
// Factory functions
// =============================================================================

/// Create all task tools with a shared store (boxed version).
pub fn create_task_tools(store: Arc<TaskStore>) -> Vec<Box<dyn Tool>> {
    vec![
        Box::new(CreateTaskTool::new(store.clone())),
        Box::new(UpdateTaskTool::new(store.clone())),
        Box::new(ListTasksTool::new(store.clone())),
        Box::new(DeleteTaskTool::new(store.clone())),
        Box::new(UpdateMyTaskTool::new(store)),
    ]
}

/// Create all task tools with a shared store (Arc version).
pub fn create_task_tools_arc(store: Arc<TaskStore>) -> Vec<Arc<dyn Tool>> {
    vec![
        Arc::new(CreateTaskTool::new(store.clone())),
        Arc::new(UpdateTaskTool::new(store.clone())),
        Arc::new(ListTasksTool::new(store.clone())),
        Arc::new(DeleteTaskTool::new(store.clone())),
        Arc::new(UpdateMyTaskTool::new(store)),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn new_store() -> Arc<TaskStore> {
        Arc::new(TaskStore::new())
    }

    #[tokio::test]
    async fn test_create_task_defaults() {
        let store = new_store();
        let tool = CreateTaskTool::new(store.clone());

        let result = tool
            .execute(serde_json::json!({"title": "Write tests"}))
            .await
            .unwrap();
        assert!(!result.is_error);
        assert!(result.content.contains("\"id\": \"1\""));
        assert!(result.content.contains("Write tests"));
        assert!(result.content.contains("todo"));
    }

    #[tokio::test]
    async fn test_create_task_with_all_fields() {
        let store = new_store();
        let tool = CreateTaskTool::new(store.clone());

        let result = tool
            .execute(serde_json::json!({
                "title": "Implement feature",
                "description": "Add the new feature",
                "assignee": "coder",
                "status": "in_progress"
            }))
            .await
            .unwrap();
        assert!(!result.is_error);
        assert!(result.content.contains("in_progress"));
        assert!(result.content.contains("coder"));
    }

    #[tokio::test]
    async fn test_id_incrementing() {
        let store = new_store();
        let tool = CreateTaskTool::new(store.clone());

        tool.execute(serde_json::json!({"title": "Task 1"}))
            .await
            .unwrap();
        let result = tool
            .execute(serde_json::json!({"title": "Task 2"}))
            .await
            .unwrap();
        assert!(result.content.contains("\"id\": \"2\""));

        let result = tool
            .execute(serde_json::json!({"title": "Task 3"}))
            .await
            .unwrap();
        assert!(result.content.contains("\"id\": \"3\""));
    }

    #[tokio::test]
    async fn test_update_task() {
        let store = new_store();
        let create = CreateTaskTool::new(store.clone());
        let update = UpdateTaskTool::new(store.clone());

        create
            .execute(serde_json::json!({"title": "Original"}))
            .await
            .unwrap();

        let result = update
            .execute(serde_json::json!({
                "id": "1",
                "title": "Updated",
                "status": "done",
                "assignee": "reviewer"
            }))
            .await
            .unwrap();
        assert!(!result.is_error);
        assert!(result.content.contains("Updated"));
        assert!(result.content.contains("done"));
        assert!(result.content.contains("reviewer"));
    }

    #[tokio::test]
    async fn test_update_task_not_found() {
        let store = new_store();
        let tool = UpdateTaskTool::new(store.clone());

        let result = tool
            .execute(serde_json::json!({"id": "999", "title": "nope"}))
            .await
            .unwrap();
        assert!(result.is_error);
        assert!(result.content.contains("not found"));
    }

    #[tokio::test]
    async fn test_list_tasks_empty() {
        let store = new_store();
        let tool = ListTasksTool::new(store.clone());

        let result = tool.execute(serde_json::json!({})).await.unwrap();
        assert_eq!(result.content, "No tasks found.");
    }

    #[tokio::test]
    async fn test_list_tasks_with_filter() {
        let store = new_store();
        let create = CreateTaskTool::new(store.clone());
        let list = ListTasksTool::new(store.clone());

        create
            .execute(serde_json::json!({"title": "A", "status": "todo"}))
            .await
            .unwrap();
        create
            .execute(serde_json::json!({"title": "B", "status": "done"}))
            .await
            .unwrap();
        create
            .execute(serde_json::json!({"title": "C", "status": "todo", "assignee": "coder"}))
            .await
            .unwrap();

        // Filter by status
        let result = list
            .execute(serde_json::json!({"status": "todo"}))
            .await
            .unwrap();
        assert!(result.content.contains("\"A\""));
        assert!(result.content.contains("\"C\""));
        assert!(!result.content.contains("\"B\""));

        // Filter by assignee
        let result = list
            .execute(serde_json::json!({"assignee": "coder"}))
            .await
            .unwrap();
        assert!(result.content.contains("\"C\""));
        assert!(!result.content.contains("\"A\""));
    }

    #[tokio::test]
    async fn test_delete_task() {
        let store = new_store();
        let create = CreateTaskTool::new(store.clone());
        let delete = DeleteTaskTool::new(store.clone());
        let list = ListTasksTool::new(store.clone());

        create
            .execute(serde_json::json!({"title": "To delete"}))
            .await
            .unwrap();

        let result = delete
            .execute(serde_json::json!({"id": "1"}))
            .await
            .unwrap();
        assert!(!result.is_error);
        assert!(result.content.contains("deleted"));

        let result = list.execute(serde_json::json!({})).await.unwrap();
        assert_eq!(result.content, "No tasks found.");
    }

    #[tokio::test]
    async fn test_delete_task_not_found() {
        let store = new_store();
        let tool = DeleteTaskTool::new(store.clone());

        let result = tool
            .execute(serde_json::json!({"id": "999"}))
            .await
            .unwrap();
        assert!(result.is_error);
        assert!(result.content.contains("not found"));
    }

    #[tokio::test]
    async fn test_invalid_status() {
        let store = new_store();
        let tool = CreateTaskTool::new(store.clone());

        let result = tool
            .execute(serde_json::json!({"title": "Bad", "status": "invalid"}))
            .await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_factory_functions() {
        let store = new_store();

        let boxed = create_task_tools(store.clone());
        assert_eq!(boxed.len(), 5);
        assert_eq!(boxed[0].name(), "create_task");
        assert_eq!(boxed[1].name(), "update_task");
        assert_eq!(boxed[2].name(), "list_tasks");
        assert_eq!(boxed[3].name(), "delete_task");
        assert_eq!(boxed[4].name(), "update_my_task");

        let arced = create_task_tools_arc(store);
        assert_eq!(arced.len(), 5);
        assert_eq!(arced[0].name(), "create_task");
        assert_eq!(arced[4].name(), "update_my_task");
    }

    // --- Dependency tests ---

    #[tokio::test]
    async fn test_create_task_with_dependencies() {
        let store = new_store();
        let tool = CreateTaskTool::new(store.clone());

        // Create task 1
        tool.execute(serde_json::json!({"title": "Task 1"}))
            .await
            .unwrap();

        // Create task 2 blocked by task 1
        let result = tool
            .execute(serde_json::json!({"title": "Task 2", "blocked_by": ["1"]}))
            .await
            .unwrap();
        assert!(!result.is_error);
        assert!(result.content.contains("blocked_by"));
        assert!(result.content.contains("\"1\""));
    }

    #[tokio::test]
    async fn test_create_task_invalid_dependency() {
        let store = new_store();
        let tool = CreateTaskTool::new(store.clone());

        let result = tool
            .execute(serde_json::json!({"title": "Task 1", "blocked_by": ["999"]}))
            .await
            .unwrap();
        assert!(result.is_error);
        assert!(result.content.contains("not found"));
    }

    #[tokio::test]
    async fn test_update_task_self_dependency() {
        let store = new_store();
        let create = CreateTaskTool::new(store.clone());
        let update = UpdateTaskTool::new(store.clone());

        create
            .execute(serde_json::json!({"title": "Task 1"}))
            .await
            .unwrap();

        let result = update
            .execute(serde_json::json!({"id": "1", "blocked_by": ["1"]}))
            .await
            .unwrap();
        assert!(result.is_error);
        assert!(result.content.contains("cannot depend on itself"));
    }

    #[tokio::test]
    async fn test_update_task_invalid_dependency() {
        let store = new_store();
        let create = CreateTaskTool::new(store.clone());
        let update = UpdateTaskTool::new(store.clone());

        create
            .execute(serde_json::json!({"title": "Task 1"}))
            .await
            .unwrap();

        let result = update
            .execute(serde_json::json!({"id": "1", "blocked_by": ["999"]}))
            .await
            .unwrap();
        assert!(result.is_error);
        assert!(result.content.contains("not found"));
    }

    #[tokio::test]
    async fn test_update_task_set_and_clear_dependencies() {
        let store = new_store();
        let create = CreateTaskTool::new(store.clone());
        let update = UpdateTaskTool::new(store.clone());

        create
            .execute(serde_json::json!({"title": "Task 1"}))
            .await
            .unwrap();
        create
            .execute(serde_json::json!({"title": "Task 2"}))
            .await
            .unwrap();

        // Set dependency
        let result = update
            .execute(serde_json::json!({"id": "2", "blocked_by": ["1"]}))
            .await
            .unwrap();
        assert!(!result.is_error);
        assert!(result.content.contains("blocked_by"));

        // Clear dependency
        let result = update
            .execute(serde_json::json!({"id": "2", "blocked_by": []}))
            .await
            .unwrap();
        assert!(!result.is_error);
        assert!(!result.content.contains("blocked_by"));
    }

    // --- Notes tests ---

    #[tokio::test]
    async fn test_update_task_add_note() {
        let store = new_store();
        let create = CreateTaskTool::new(store.clone());
        let update = UpdateTaskTool::new(store.clone());

        create
            .execute(serde_json::json!({"title": "Task 1"}))
            .await
            .unwrap();

        let result = update
            .execute(serde_json::json!({"id": "1", "add_note": "Found 3 files to modify"}))
            .await
            .unwrap();
        assert!(!result.is_error);
        assert!(result.content.contains("Found 3 files to modify"));
    }

    #[tokio::test]
    async fn test_update_task_multiple_notes() {
        let store = new_store();
        let create = CreateTaskTool::new(store.clone());
        let update = UpdateTaskTool::new(store.clone());

        create
            .execute(serde_json::json!({"title": "Task 1"}))
            .await
            .unwrap();

        update
            .execute(serde_json::json!({"id": "1", "add_note": "Note 1"}))
            .await
            .unwrap();
        let result = update
            .execute(serde_json::json!({"id": "1", "add_note": "Note 2"}))
            .await
            .unwrap();
        assert!(result.content.contains("Note 1"));
        assert!(result.content.contains("Note 2"));
    }

    // --- List tasks with blocks ---

    #[tokio::test]
    async fn test_list_tasks_shows_blocks() {
        let store = new_store();
        let create = CreateTaskTool::new(store.clone());
        let list = ListTasksTool::new(store.clone());

        create
            .execute(serde_json::json!({"title": "Task 1"}))
            .await
            .unwrap();
        create
            .execute(serde_json::json!({"title": "Task 2", "blocked_by": ["1"]}))
            .await
            .unwrap();

        let result = list.execute(serde_json::json!({})).await.unwrap();
        assert!(result.content.contains("blocks"));
    }

    // --- format_board tests ---

    #[test]
    fn test_format_board_empty() {
        let store = TaskStore::new();
        assert!(store.format_board().is_none());
    }

    #[tokio::test]
    async fn test_format_board_populated() {
        let store = new_store();
        let create = CreateTaskTool::new(store.clone());

        create
            .execute(serde_json::json!({"title": "Explore auth module", "assignee": "explore"}))
            .await
            .unwrap();
        create
            .execute(serde_json::json!({
                "title": "Refactor auth handler",
                "assignee": "coder",
                "status": "in_progress",
                "blocked_by": ["1"]
            }))
            .await
            .unwrap();

        let board = store.format_board().unwrap();
        assert!(board.contains("## Current Task Board"));
        assert!(board.contains("[todo] #1: Explore auth module (assignee: explore)"));
        assert!(board.contains("[in_progress] #2: Refactor auth handler (assignee: coder)"));
        assert!(board.contains("blocked by: #1"));
        assert!(board.contains("blocks: #2"));
    }

    // --- UpdateMyTaskTool tests ---

    #[tokio::test]
    async fn test_update_my_task_status() {
        let store = new_store();
        let create = CreateTaskTool::new(store.clone());
        let update_my = UpdateMyTaskTool::new(store.clone());

        create
            .execute(serde_json::json!({"title": "My task"}))
            .await
            .unwrap();

        let result = update_my
            .execute(serde_json::json!({"id": "1", "status": "done"}))
            .await
            .unwrap();
        assert!(!result.is_error);
        assert!(result.content.contains("done"));
    }

    #[tokio::test]
    async fn test_update_my_task_add_note() {
        let store = new_store();
        let create = CreateTaskTool::new(store.clone());
        let update_my = UpdateMyTaskTool::new(store.clone());

        create
            .execute(serde_json::json!({"title": "My task"}))
            .await
            .unwrap();

        let result = update_my
            .execute(serde_json::json!({"id": "1", "add_note": "Found the issue"}))
            .await
            .unwrap();
        assert!(!result.is_error);
        assert!(result.content.contains("Found the issue"));
    }

    #[tokio::test]
    async fn test_update_my_task_requires_field() {
        let store = new_store();
        let create = CreateTaskTool::new(store.clone());
        let update_my = UpdateMyTaskTool::new(store.clone());

        create
            .execute(serde_json::json!({"title": "My task"}))
            .await
            .unwrap();

        let result = update_my
            .execute(serde_json::json!({"id": "1"}))
            .await
            .unwrap();
        assert!(result.is_error);
        assert!(result.content.contains("At least one"));
    }

    #[tokio::test]
    async fn test_update_my_task_not_found() {
        let store = new_store();
        let update_my = UpdateMyTaskTool::new(store.clone());

        let result = update_my
            .execute(serde_json::json!({"id": "999", "status": "done"}))
            .await
            .unwrap();
        assert!(result.is_error);
        assert!(result.content.contains("not found"));
    }
}
