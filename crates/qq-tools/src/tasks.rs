//! Task tracking tools for session-scoped work management.
//!
//! Provides in-memory task storage with CRUD operations, designed for
//! the project manager agent to track delegated work items.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use tokio::sync::Notify;

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
    /// Result output from a background agent execution.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<String>,
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
/// Includes a `Notify` for signaling background agent completions.
pub struct TaskStore {
    inner: Mutex<TaskStoreInner>,
    /// Notified when any task's result/status is set (for `wait_for_tasks`).
    completion_notify: Notify,
}

impl TaskStore {
    pub fn new() -> Self {
        Self {
            inner: Mutex::new(TaskStoreInner {
                tasks: HashMap::new(),
                next_id: 1,
            }),
            completion_notify: Notify::new(),
        }
    }

    /// Check if a task exists.
    pub fn has_task(&self, id: &str) -> bool {
        self.inner.lock().unwrap().tasks.contains_key(id)
    }

    /// Set a task's status.
    pub fn set_status(&self, id: &str, status: TaskStatus) {
        let mut inner = self.inner.lock().unwrap();
        if let Some(task) = inner.tasks.get_mut(id) {
            task.status = status;
        }
    }

    /// Set a task's result and status atomically.
    ///
    /// Used by background agent completions. Notifies any waiters
    /// (i.e., `wait_for_tasks` calls) after the update.
    pub fn set_result(&self, id: &str, result: String, status: TaskStatus) {
        {
            let mut inner = self.inner.lock().unwrap();
            if let Some(task) = inner.tasks.get_mut(id) {
                task.result = Some(result);
                task.status = status;
            }
        }
        // Notify after releasing the lock
        self.completion_notify.notify_waiters();
    }

    /// Wait until at least one task in `watched_ids` has status Done or Blocked.
    ///
    /// Returns the IDs of completed/blocked tasks. If any are already
    /// complete, returns immediately. Returns an empty vec on timeout.
    pub async fn wait_for_completion(
        &self,
        watched_ids: &[String],
        timeout: Duration,
    ) -> Vec<String> {
        let deadline = tokio::time::Instant::now() + timeout;
        loop {
            // Register the waiter BEFORE checking state. `Notify::notify_waiters`
            // does not store permits — it only wakes waiters already registered
            // on a `Notified` future. By calling `enable()` first, any
            // `set_result` that runs between our state check and the `.await`
            // is still captured, eliminating the missed-wakeup race.
            let notified = self.completion_notify.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();

            // Check current state.
            let completed: Vec<String> = {
                let inner = self.inner.lock().unwrap();
                watched_ids
                    .iter()
                    .filter(|id| {
                        inner
                            .tasks
                            .get(id.as_str())
                            .map(|t| matches!(t.status, TaskStatus::Done | TaskStatus::Blocked))
                            .unwrap_or(false)
                    })
                    .cloned()
                    .collect()
            };
            if !completed.is_empty() {
                return completed;
            }

            // Wait for notification or timeout.
            match tokio::time::timeout_at(deadline, notified).await {
                Ok(_) => continue,
                Err(_) => return vec![],
            }
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
                let deps: Vec<String> = task
                    .blocked_by
                    .iter()
                    .map(|id| format!("#{}", id))
                    .collect();
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

            if task.result.is_some() {
                lines.push("  result: [available — use get_task_result to view]".to_string());
            }
        }

        Some(lines.join("\n"))
    }

    /// Clear all tasks and reset the ID counter.
    pub fn clear(&self) {
        let mut inner = self.inner.lock().unwrap();
        inner.tasks.clear();
        inner.next_id = 1;
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
            Some(s) => TaskStatus::from_str(&s).map_err(|e| Error::tool("create_task", e))?,
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
            result: None,
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
                .add_property(
                    "id",
                    PropertySchema::string("ID of the task to update"),
                    true,
                )
                .add_property(
                    "title",
                    PropertySchema::string("New title for the task"),
                    false,
                )
                .add_property(
                    "status",
                    PropertySchema::string("New status: todo, in_progress, done, or blocked"),
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
            task.status =
                TaskStatus::from_str(&status_str).map_err(|e| Error::tool("update_task", e))?;
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
                    PropertySchema::string("Filter by status: todo, in_progress, done, or blocked"),
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
            Some(s) => Some(TaskStatus::from_str(&s).map_err(|e| Error::tool("list_tasks", e))?),
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
                blocks_map.entry(dep_id.as_str()).or_default().push(&t.id);
            }
        }

        // Serialize with derived "blocks" field; suppress full result text
        let output_values: Vec<serde_json::Value> = tasks
            .iter()
            .map(|task| {
                let mut val = serde_json::to_value(task).unwrap();
                if let Some(blocks) = blocks_map.get(task.id.as_str()) {
                    val.as_object_mut().unwrap().insert(
                        "blocks".to_string(),
                        serde_json::Value::Array(
                            blocks
                                .iter()
                                .map(|id| serde_json::Value::String(id.to_string()))
                                .collect(),
                        ),
                    );
                }
                // Replace full result with availability hint
                if task.result.is_some() {
                    val.as_object_mut().unwrap().insert(
                        "result".to_string(),
                        serde_json::Value::String(
                            "[available — use get_task_result to view]".to_string(),
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
            ToolParameters::new().add_property(
                "id",
                PropertySchema::string("ID of the task to delete"),
                true,
            ),
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
                .add_property(
                    "id",
                    PropertySchema::string("ID of the task to update"),
                    true,
                )
                .add_property(
                    "status",
                    PropertySchema::string("New status: todo, in_progress, done, or blocked"),
                    false,
                )
                .add_property(
                    "add_note",
                    PropertySchema::string(
                        "Append a progress note (e.g., findings, blockers, completion summary)",
                    ),
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
            task.status =
                TaskStatus::from_str(&status_str).map_err(|e| Error::tool("update_my_task", e))?;
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
// GetTaskResultTool
// =============================================================================

/// Tool to retrieve the full result output from a completed background agent task.
pub struct GetTaskResultTool {
    store: Arc<TaskStore>,
}

impl GetTaskResultTool {
    pub fn new(store: Arc<TaskStore>) -> Self {
        Self { store }
    }
}

#[derive(Deserialize)]
struct GetTaskResultArgs {
    id: String,
}

#[async_trait]
impl Tool for GetTaskResultTool {
    fn name(&self) -> &str {
        "get_task_result"
    }

    fn description(&self) -> &str {
        "Get the full result output from a completed background agent task."
    }

    fn tool_description(&self) -> &str {
        "Retrieve the result of a completed background agent task."
    }

    fn definition(&self) -> ToolDefinition {
        ToolDefinition::new(self.name(), self.description()).with_parameters(
            ToolParameters::new().add_property(
                "id",
                PropertySchema::string("ID of the task to get the result for"),
                true,
            ),
        )
    }

    fn is_blocking(&self) -> bool {
        false
    }

    async fn execute(&self, arguments: serde_json::Value) -> Result<ToolOutput, Error> {
        let args: GetTaskResultArgs = serde_json::from_value(arguments)
            .map_err(|e| Error::tool("get_task_result", format!("Invalid arguments: {}", e)))?;

        let inner = self.store.inner.lock().unwrap();
        match inner.tasks.get(&args.id) {
            Some(task) => match &task.result {
                Some(result) => Ok(ToolOutput::success(result.clone())),
                None => Ok(ToolOutput::error(format!(
                    "Task #{} has no result yet (status: {})",
                    args.id, task.status
                ))),
            },
            None => Ok(ToolOutput::error(format!(
                "Task with id '{}' not found",
                args.id
            ))),
        }
    }
}

// =============================================================================
// WaitForTasksTool
// =============================================================================

/// Tool that blocks until specified background tasks complete.
pub struct WaitForTasksTool {
    store: Arc<TaskStore>,
}

impl WaitForTasksTool {
    pub fn new(store: Arc<TaskStore>) -> Self {
        Self { store }
    }
}

#[derive(Deserialize)]
struct WaitForTasksArgs {
    task_ids: Vec<String>,
    #[serde(default = "default_timeout_secs")]
    timeout_secs: u64,
}

fn default_timeout_secs() -> u64 {
    600
}

#[async_trait]
impl Tool for WaitForTasksTool {
    fn name(&self) -> &str {
        "wait_for_tasks"
    }

    fn description(&self) -> &str {
        "Wait until one or more background agent tasks complete. Blocks until at least one of the specified tasks reaches 'done' or 'blocked' status, then returns the completed task IDs."
    }

    fn tool_description(&self) -> &str {
        "Block until background agent tasks complete."
    }

    fn definition(&self) -> ToolDefinition {
        ToolDefinition::new(self.name(), self.description()).with_parameters(
            ToolParameters::new()
                .add_property(
                    "task_ids",
                    PropertySchema::array(
                        "IDs of tasks to wait for",
                        PropertySchema::string("Task ID"),
                    ),
                    true,
                )
                .add_property(
                    "timeout_secs",
                    PropertySchema::number(
                        "Maximum seconds to wait (default: 600). Returns empty on timeout.",
                    ),
                    false,
                ),
        )
    }

    fn is_blocking(&self) -> bool {
        false
    }

    async fn execute(&self, arguments: serde_json::Value) -> Result<ToolOutput, Error> {
        let args: WaitForTasksArgs = serde_json::from_value(arguments)
            .map_err(|e| Error::tool("wait_for_tasks", format!("Invalid arguments: {}", e)))?;

        if args.task_ids.is_empty() {
            return Ok(ToolOutput::error(
                "At least one task_id must be provided".to_string(),
            ));
        }

        let timeout = Duration::from_secs(args.timeout_secs);
        let completed = self
            .store
            .wait_for_completion(&args.task_ids, timeout)
            .await;

        if completed.is_empty() {
            return Ok(ToolOutput::success(format!(
                "Timeout after {}s: no tasks completed. Watched task IDs: {}",
                args.timeout_secs,
                args.task_ids.join(", ")
            )));
        }

        // Build summary of completed tasks
        let inner = self.store.inner.lock().unwrap();
        let mut lines = vec![format!("{} task(s) completed:", completed.len())];
        for id in &completed {
            if let Some(task) = inner.tasks.get(id.as_str()) {
                lines.push(format!("  #{} [{}]: {}", id, task.status, task.title));
            }
        }
        lines.push(String::new());
        lines.push("Use get_task_result to retrieve each task's full output.".to_string());

        Ok(ToolOutput::success(lines.join("\n")))
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
        Box::new(UpdateMyTaskTool::new(store.clone())),
        Box::new(GetTaskResultTool::new(store.clone())),
        Box::new(WaitForTasksTool::new(store)),
    ]
}

/// Create all task tools with a shared store (Arc version).
pub fn create_task_tools_arc(store: Arc<TaskStore>) -> Vec<Arc<dyn Tool>> {
    vec![
        Arc::new(CreateTaskTool::new(store.clone())),
        Arc::new(UpdateTaskTool::new(store.clone())),
        Arc::new(ListTasksTool::new(store.clone())),
        Arc::new(DeleteTaskTool::new(store.clone())),
        Arc::new(UpdateMyTaskTool::new(store.clone())),
        Arc::new(GetTaskResultTool::new(store.clone())),
        Arc::new(WaitForTasksTool::new(store)),
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
        assert!(result.text_content().contains("\"id\": \"1\""));
        assert!(result.text_content().contains("Write tests"));
        assert!(result.text_content().contains("todo"));
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
        assert!(result.text_content().contains("in_progress"));
        assert!(result.text_content().contains("coder"));
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
        assert!(result.text_content().contains("\"id\": \"2\""));

        let result = tool
            .execute(serde_json::json!({"title": "Task 3"}))
            .await
            .unwrap();
        assert!(result.text_content().contains("\"id\": \"3\""));
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
        assert!(result.text_content().contains("Updated"));
        assert!(result.text_content().contains("done"));
        assert!(result.text_content().contains("reviewer"));
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
        assert!(result.text_content().contains("not found"));
    }

    #[tokio::test]
    async fn test_list_tasks_empty() {
        let store = new_store();
        let tool = ListTasksTool::new(store.clone());

        let result = tool.execute(serde_json::json!({})).await.unwrap();
        assert_eq!(result.text_content(), "No tasks found.");
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
        assert!(result.text_content().contains("\"A\""));
        assert!(result.text_content().contains("\"C\""));
        assert!(!result.text_content().contains("\"B\""));

        // Filter by assignee
        let result = list
            .execute(serde_json::json!({"assignee": "coder"}))
            .await
            .unwrap();
        assert!(result.text_content().contains("\"C\""));
        assert!(!result.text_content().contains("\"A\""));
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
        assert!(result.text_content().contains("deleted"));

        let result = list.execute(serde_json::json!({})).await.unwrap();
        assert_eq!(result.text_content(), "No tasks found.");
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
        assert!(result.text_content().contains("not found"));
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
        assert_eq!(boxed.len(), 7);
        assert_eq!(boxed[0].name(), "create_task");
        assert_eq!(boxed[1].name(), "update_task");
        assert_eq!(boxed[2].name(), "list_tasks");
        assert_eq!(boxed[3].name(), "delete_task");
        assert_eq!(boxed[4].name(), "update_my_task");
        assert_eq!(boxed[5].name(), "get_task_result");
        assert_eq!(boxed[6].name(), "wait_for_tasks");

        let arced = create_task_tools_arc(store);
        assert_eq!(arced.len(), 7);
        assert_eq!(arced[0].name(), "create_task");
        assert_eq!(arced[6].name(), "wait_for_tasks");
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
        assert!(result.text_content().contains("blocked_by"));
        assert!(result.text_content().contains("\"1\""));
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
        assert!(result.text_content().contains("not found"));
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
        assert!(result.text_content().contains("cannot depend on itself"));
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
        assert!(result.text_content().contains("not found"));
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
        assert!(result.text_content().contains("blocked_by"));

        // Clear dependency
        let result = update
            .execute(serde_json::json!({"id": "2", "blocked_by": []}))
            .await
            .unwrap();
        assert!(!result.is_error);
        assert!(!result.text_content().contains("blocked_by"));
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
        assert!(result.text_content().contains("Found 3 files to modify"));
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
        assert!(result.text_content().contains("Note 1"));
        assert!(result.text_content().contains("Note 2"));
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
        assert!(result.text_content().contains("blocks"));
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
        assert!(result.text_content().contains("done"));
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
        assert!(result.text_content().contains("Found the issue"));
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
        assert!(result.text_content().contains("At least one"));
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
        assert!(result.text_content().contains("not found"));
    }

    #[tokio::test]
    async fn test_clear_store() {
        let store = new_store();
        let create = CreateTaskTool::new(store.clone());

        create
            .execute(serde_json::json!({"title": "Task 1"}))
            .await
            .unwrap();
        create
            .execute(serde_json::json!({"title": "Task 2"}))
            .await
            .unwrap();

        assert!(store.format_board().is_some());

        store.clear();

        assert!(store.format_board().is_none());

        // Next task should get id "1" again
        let result = create
            .execute(serde_json::json!({"title": "After clear"}))
            .await
            .unwrap();
        assert!(result.text_content().contains("\"id\": \"1\""));
    }

    // --- Background task result tests ---

    #[test]
    fn test_has_task() {
        let store = TaskStore::new();
        assert!(!store.has_task("1"));

        {
            let mut inner = store.inner.lock().unwrap();
            inner.tasks.insert(
                "1".to_string(),
                Task {
                    id: "1".to_string(),
                    title: "Test".to_string(),
                    status: TaskStatus::Todo,
                    assignee: None,
                    description: None,
                    blocked_by: vec![],
                    notes: vec![],
                    result: None,
                },
            );
        }
        assert!(store.has_task("1"));
        assert!(!store.has_task("2"));
    }

    #[test]
    fn test_set_result() {
        let store = TaskStore::new();
        {
            let mut inner = store.inner.lock().unwrap();
            inner.tasks.insert(
                "1".to_string(),
                Task {
                    id: "1".to_string(),
                    title: "Test".to_string(),
                    status: TaskStatus::InProgress,
                    assignee: None,
                    description: None,
                    blocked_by: vec![],
                    notes: vec![],
                    result: None,
                },
            );
        }

        store.set_result("1", "Agent output here".to_string(), TaskStatus::Done);

        let inner = store.inner.lock().unwrap();
        let task = inner.tasks.get("1").unwrap();
        assert_eq!(task.status, TaskStatus::Done);
        assert_eq!(task.result.as_deref(), Some("Agent output here"));
    }

    #[test]
    fn test_set_result_nonexistent_task() {
        let store = TaskStore::new();
        // Should not panic
        store.set_result("999", "output".to_string(), TaskStatus::Done);
    }

    #[tokio::test]
    async fn test_get_task_result_tool() {
        let store = new_store();
        let create = CreateTaskTool::new(store.clone());
        let get_result = GetTaskResultTool::new(store.clone());

        create
            .execute(serde_json::json!({"title": "Background task"}))
            .await
            .unwrap();

        // No result yet
        let result = get_result
            .execute(serde_json::json!({"id": "1"}))
            .await
            .unwrap();
        assert!(result.is_error);
        assert!(result.text_content().contains("no result yet"));

        // Set result
        store.set_result(
            "1",
            "Exploration complete: found 5 files".to_string(),
            TaskStatus::Done,
        );

        // Now has result
        let result = get_result
            .execute(serde_json::json!({"id": "1"}))
            .await
            .unwrap();
        assert!(!result.is_error);
        assert_eq!(result.text_content(), "Exploration complete: found 5 files");
    }

    #[tokio::test]
    async fn test_get_task_result_not_found() {
        let store = new_store();
        let get_result = GetTaskResultTool::new(store);

        let result = get_result
            .execute(serde_json::json!({"id": "999"}))
            .await
            .unwrap();
        assert!(result.is_error);
        assert!(result.text_content().contains("not found"));
    }

    #[tokio::test]
    async fn test_list_tasks_hides_full_result() {
        let store = new_store();
        let create = CreateTaskTool::new(store.clone());
        let list = ListTasksTool::new(store.clone());

        create
            .execute(serde_json::json!({"title": "Background task"}))
            .await
            .unwrap();

        store.set_result(
            "1",
            "Very long agent output...".to_string(),
            TaskStatus::Done,
        );

        let result = list.execute(serde_json::json!({})).await.unwrap();
        let text = result.text_content();
        // Should show availability hint, not full result
        assert!(text.contains("get_task_result"));
        assert!(!text.contains("Very long agent output"));
    }

    #[tokio::test]
    async fn test_wait_for_tasks_immediate_completion() {
        let store = new_store();
        let create = CreateTaskTool::new(store.clone());
        let wait = WaitForTasksTool::new(store.clone());

        create
            .execute(serde_json::json!({"title": "Task 1", "status": "done"}))
            .await
            .unwrap();

        let result = wait
            .execute(serde_json::json!({"task_ids": ["1"], "timeout_secs": 1}))
            .await
            .unwrap();
        assert!(!result.is_error);
        assert!(result.text_content().contains("#1"));
        assert!(result.text_content().contains("completed"));
    }

    #[tokio::test]
    async fn test_wait_for_tasks_blocks_then_completes() {
        let store = new_store();
        let create = CreateTaskTool::new(store.clone());
        let wait = WaitForTasksTool::new(store.clone());

        create
            .execute(serde_json::json!({"title": "Background task"}))
            .await
            .unwrap();

        // Spawn a task that completes the task after a short delay
        let store_clone = store.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(100)).await;
            store_clone.set_result("1", "Done!".to_string(), TaskStatus::Done);
        });

        let result = wait
            .execute(serde_json::json!({"task_ids": ["1"], "timeout_secs": 5}))
            .await
            .unwrap();
        assert!(!result.is_error);
        assert!(result.text_content().contains("#1"));
    }

    /// Smoke test for concurrent `set_result` + `wait_for_completion` under a
    /// multi-threaded runtime.
    ///
    /// Spawns a batch of waiters each watching a distinct task, plus setters
    /// that complete those tasks with tiny jittered delays. The goal is to
    /// exercise the lock + `Notify` protocol under real preemption and fail
    /// loudly on any regression that reintroduces lost wakeups, deadlocks,
    /// or cross-task state corruption. None of these failures show up under
    /// the single-threaded test flavor, so we force `multi_thread` here.
    ///
    /// The test is not a deterministic reproducer for the
    /// `notify_waiters` missed-wakeup race — the race window is narrow
    /// enough that landing it in-process is probabilistic. The fix for
    /// that race (`Notified::enable()` before the state check in
    /// `wait_for_completion`) is a standard tokio pattern documented in
    /// `tokio::sync::Notify`; this test is its belt-and-suspenders guard.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn test_wait_for_completion_stress() {
        const BATCHES: usize = 20;
        const TASKS_PER_BATCH: usize = 10;

        for _batch in 0..BATCHES {
            let store = new_store();
            let create = CreateTaskTool::new(store.clone());

            let mut ids = Vec::with_capacity(TASKS_PER_BATCH);
            for i in 0..TASKS_PER_BATCH {
                create
                    .execute(serde_json::json!({"title": format!("task {}", i)}))
                    .await
                    .unwrap();
                ids.push((i + 1).to_string());
            }

            // Kick off setters that complete each task with a tiny jittered delay.
            let mut setters = Vec::with_capacity(TASKS_PER_BATCH);
            for (i, id) in ids.iter().enumerate() {
                let s = store.clone();
                let id = id.clone();
                let spin_count = (i as u64) * 50;
                setters.push(tokio::spawn(async move {
                    for _ in 0..spin_count {
                        std::hint::spin_loop();
                    }
                    s.set_result(&id, format!("done {}", id), TaskStatus::Done);
                }));
            }

            // Waiters — one per task, each watching one id.
            let mut waiters = Vec::with_capacity(TASKS_PER_BATCH);
            for id in &ids {
                let s = store.clone();
                let id = id.clone();
                waiters.push(tokio::spawn(async move {
                    s.wait_for_completion(&[id], Duration::from_secs(5)).await
                }));
            }

            // Everyone must finish well under the per-waiter 5s budget; a hang
            // here means a lost wakeup or deadlock.
            let all = tokio::time::timeout(Duration::from_secs(10), async {
                for w in waiters {
                    let completed = w.await.unwrap();
                    assert_eq!(completed.len(), 1, "waiter should see its task complete");
                }
                for s in setters {
                    s.await.unwrap();
                }
            })
            .await;
            assert!(all.is_ok(), "stress batch deadlocked or timed out");
        }
    }

    #[tokio::test]
    async fn test_wait_for_tasks_timeout() {
        let store = new_store();
        let create = CreateTaskTool::new(store.clone());
        let wait = WaitForTasksTool::new(store.clone());

        create
            .execute(serde_json::json!({"title": "Never finishes"}))
            .await
            .unwrap();

        let result = wait
            .execute(serde_json::json!({"task_ids": ["1"], "timeout_secs": 1}))
            .await
            .unwrap();
        assert!(!result.is_error);
        assert!(result.text_content().contains("Timeout"));
    }

    #[tokio::test]
    async fn test_wait_for_tasks_empty_ids() {
        let store = new_store();
        let wait = WaitForTasksTool::new(store);

        let result = wait
            .execute(serde_json::json!({"task_ids": []}))
            .await
            .unwrap();
        assert!(result.is_error);
        assert!(result.text_content().contains("At least one"));
    }

    #[tokio::test]
    async fn test_format_board_shows_result_available() {
        let store = new_store();
        let create = CreateTaskTool::new(store.clone());

        create
            .execute(serde_json::json!({"title": "Task with result"}))
            .await
            .unwrap();

        store.set_result("1", "output".to_string(), TaskStatus::Done);

        let board = store.format_board().unwrap();
        assert!(board.contains("result: [available"));
    }
}
