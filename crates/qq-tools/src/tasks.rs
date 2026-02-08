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
        let id = inner.next_id.to_string();
        inner.next_id += 1;

        let task = Task {
            id: id.clone(),
            title: args.title,
            status,
            assignee: args.assignee,
            description: args.description,
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

        let output = serde_json::to_string_pretty(&tasks)
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
// Factory functions
// =============================================================================

/// Create all task tools with a shared store (boxed version).
pub fn create_task_tools(store: Arc<TaskStore>) -> Vec<Box<dyn Tool>> {
    vec![
        Box::new(CreateTaskTool::new(store.clone())),
        Box::new(UpdateTaskTool::new(store.clone())),
        Box::new(ListTasksTool::new(store.clone())),
        Box::new(DeleteTaskTool::new(store)),
    ]
}

/// Create all task tools with a shared store (Arc version).
pub fn create_task_tools_arc(store: Arc<TaskStore>) -> Vec<Arc<dyn Tool>> {
    vec![
        Arc::new(CreateTaskTool::new(store.clone())),
        Arc::new(UpdateTaskTool::new(store.clone())),
        Arc::new(ListTasksTool::new(store.clone())),
        Arc::new(DeleteTaskTool::new(store)),
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
        assert_eq!(boxed.len(), 4);
        assert_eq!(boxed[0].name(), "create_task");
        assert_eq!(boxed[1].name(), "update_task");
        assert_eq!(boxed[2].name(), "list_tasks");
        assert_eq!(boxed[3].name(), "delete_task");

        let arced = create_task_tools_arc(store);
        assert_eq!(arced.len(), 4);
        assert_eq!(arced[0].name(), "create_task");
    }
}
