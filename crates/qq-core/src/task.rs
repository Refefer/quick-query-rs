//! Async task management for parallel execution and background tasks.
//!
//! This module provides:
//! - `TaskManager` for spawning and tracking async tasks
//! - `TaskHandle` for monitoring and cancelling individual tasks
//! - Parallel execution helpers for tools and LLM completions

use std::collections::HashMap;
use std::future::Future;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Instant;

use tokio::sync::{oneshot, watch, RwLock};
use tokio::task::JoinHandle;

use crate::error::Error;
use crate::message::ToolCall;
use crate::provider::{CompletionRequest, CompletionResponse, Provider};
use crate::tool::ToolRegistry;

/// Unique identifier for a task.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct TaskId(pub u64);

impl std::fmt::Display for TaskId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// Current state of a task.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TaskState {
    /// Task is waiting to start.
    Pending,
    /// Task is currently running.
    Running,
    /// Task completed successfully.
    Completed,
    /// Task failed with an error.
    Failed(String),
    /// Task was cancelled.
    Cancelled,
}

impl TaskState {
    /// Returns true if the task has finished (completed, failed, or cancelled).
    pub fn is_finished(&self) -> bool {
        matches!(
            self,
            TaskState::Completed | TaskState::Failed(_) | TaskState::Cancelled
        )
    }
}

/// Information about a task for listing and monitoring.
#[derive(Debug, Clone)]
pub struct TaskInfo {
    /// Unique task identifier.
    pub id: TaskId,
    /// Human-readable task name.
    pub name: String,
    /// Current task state.
    pub state: TaskState,
    /// When the task was created.
    pub created_at: Instant,
}

/// Handle for monitoring and controlling a spawned task.
///
/// The handle provides:
/// - Task identification and metadata
/// - State monitoring via a watch channel
/// - Cancellation support
/// - Ability to await the task result
pub struct TaskHandle<T> {
    /// Unique task identifier.
    pub id: TaskId,
    /// Human-readable task name.
    pub name: String,
    /// Join handle to await the task result.
    join_handle: JoinHandle<T>,
    /// Sender to signal cancellation.
    cancel_tx: Option<oneshot::Sender<()>>,
    /// Receiver to watch task state changes.
    state_rx: watch::Receiver<TaskState>,
}

impl<T> TaskHandle<T> {
    /// Get the current task state.
    pub fn state(&self) -> TaskState {
        self.state_rx.borrow().clone()
    }

    /// Check if the task is still running.
    pub fn is_running(&self) -> bool {
        matches!(*self.state_rx.borrow(), TaskState::Running)
    }

    /// Check if the task has finished.
    pub fn is_finished(&self) -> bool {
        self.state_rx.borrow().is_finished()
    }

    /// Wait for the task state to change.
    pub async fn state_changed(&mut self) -> Result<TaskState, Error> {
        self.state_rx
            .changed()
            .await
            .map_err(|_| Error::stream("Task state channel closed"))?;
        Ok(self.state_rx.borrow().clone())
    }

    /// Request cancellation of the task.
    ///
    /// Note: The task must cooperate with cancellation by checking the
    /// cancellation signal. This only sends the signal.
    pub fn cancel(&mut self) -> bool {
        if let Some(tx) = self.cancel_tx.take() {
            tx.send(()).is_ok()
        } else {
            false
        }
    }

    /// Await the task result.
    pub async fn join(self) -> Result<T, Error> {
        self.join_handle
            .await
            .map_err(|e| Error::Unknown(format!("Task panicked: {}", e)))
    }
}

/// Manager for spawning and tracking async tasks.
///
/// The TaskManager maintains a registry of all spawned tasks and provides
/// methods for listing and monitoring them.
#[derive(Clone)]
pub struct TaskManager {
    /// Counter for generating unique task IDs.
    next_id: Arc<AtomicU64>,
    /// Registry of all tasks.
    tasks: Arc<RwLock<HashMap<TaskId, TaskInfo>>>,
}

impl Default for TaskManager {
    fn default() -> Self {
        Self::new()
    }
}

impl TaskManager {
    /// Create a new task manager.
    pub fn new() -> Self {
        Self {
            next_id: Arc::new(AtomicU64::new(1)),
            tasks: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Spawn a new task with the given name.
    ///
    /// Returns a handle that can be used to monitor and await the task.
    pub fn spawn<F, T>(&self, name: &str, future: F) -> TaskHandle<T>
    where
        F: Future<Output = T> + Send + 'static,
        T: Send + 'static,
    {
        let id = TaskId(self.next_id.fetch_add(1, Ordering::SeqCst));
        let name = name.to_string();

        // Create state watch channel
        let (state_tx, state_rx) = watch::channel(TaskState::Running);

        // Create cancellation channel (not used in basic spawn)
        let (cancel_tx, _cancel_rx) = oneshot::channel();

        // Track task info
        let info = TaskInfo {
            id,
            name: name.clone(),
            state: TaskState::Running,
            created_at: Instant::now(),
        };

        let tasks = self.tasks.clone();
        let task_id = id;

        // Spawn the task with state updates
        let join_handle = tokio::spawn(async move {
            // Insert task info
            {
                let mut tasks = tasks.write().await;
                tasks.insert(task_id, info);
            }

            // Run the future
            let result = future.await;

            // Update state to completed
            let _ = state_tx.send(TaskState::Completed);
            {
                let mut tasks = tasks.write().await;
                if let Some(info) = tasks.get_mut(&task_id) {
                    info.state = TaskState::Completed;
                }
            }

            result
        });

        TaskHandle {
            id,
            name,
            join_handle,
            cancel_tx: Some(cancel_tx),
            state_rx,
        }
    }

    /// Spawn a cancellable task.
    ///
    /// The provided closure receives a cancellation receiver that it should
    /// periodically check. When cancellation is requested, the receiver
    /// will resolve.
    pub fn spawn_cancellable<F, Fut, T>(&self, name: &str, f: F) -> TaskHandle<Option<T>>
    where
        F: FnOnce(oneshot::Receiver<()>) -> Fut + Send + 'static,
        Fut: Future<Output = T> + Send,
        T: Send + 'static,
    {
        let id = TaskId(self.next_id.fetch_add(1, Ordering::SeqCst));
        let name = name.to_string();

        // Create state watch channel
        let (state_tx, state_rx) = watch::channel(TaskState::Running);

        // Create cancellation channel
        let (cancel_tx, cancel_rx) = oneshot::channel();

        // Track task info
        let info = TaskInfo {
            id,
            name: name.clone(),
            state: TaskState::Running,
            created_at: Instant::now(),
        };

        let tasks = self.tasks.clone();
        let task_id = id;

        // Spawn the task
        let join_handle = tokio::spawn(async move {
            // Insert task info
            {
                let mut tasks = tasks.write().await;
                tasks.insert(task_id, info);
            }

            // Run the future with cancellation support
            let future = f(cancel_rx);

            tokio::select! {
                result = future => {
                    // Task completed normally
                    let _ = state_tx.send(TaskState::Completed);
                    {
                        let mut tasks = tasks.write().await;
                        if let Some(info) = tasks.get_mut(&task_id) {
                            info.state = TaskState::Completed;
                        }
                    }
                    Some(result)
                }
            }
        });

        TaskHandle {
            id,
            name,
            join_handle,
            cancel_tx: Some(cancel_tx),
            state_rx,
        }
    }

    /// List all tracked tasks.
    pub async fn list_tasks(&self) -> Vec<TaskInfo> {
        let tasks = self.tasks.read().await;
        tasks.values().cloned().collect()
    }

    /// Get information about a specific task.
    pub async fn get_task(&self, id: TaskId) -> Option<TaskInfo> {
        let tasks = self.tasks.read().await;
        tasks.get(&id).cloned()
    }

    /// Remove completed tasks from the registry.
    pub async fn cleanup_finished(&self) {
        let mut tasks = self.tasks.write().await;
        tasks.retain(|_, info| !info.state.is_finished());
    }
}

/// Result of a parallel tool execution.
pub struct ToolExecutionResult {
    /// The tool call ID.
    pub tool_call_id: String,
    /// The result content or error message.
    pub content: String,
    /// Whether the result is an error.
    pub is_error: bool,
}

/// Execute multiple tool calls in parallel.
///
/// Returns results in the same order as the input tool calls.
pub async fn execute_tools_parallel(
    registry: &ToolRegistry,
    tool_calls: Vec<ToolCall>,
) -> Vec<ToolExecutionResult> {
    use futures::future::join_all;

    let futures: Vec<_> = tool_calls
        .into_iter()
        .map(|tool_call| {
            let tool_call_id = tool_call.id.clone();
            let tool_name = tool_call.name.clone();
            let arguments = tool_call.arguments.clone();

            async move {
                let Some(tool) = registry.get(&tool_name) else {
                    return ToolExecutionResult {
                        tool_call_id,
                        content: format!("Error: Unknown tool '{}'", tool_name),
                        is_error: true,
                    };
                };

                match tool.execute(arguments).await {
                    Ok(output) => ToolExecutionResult {
                        tool_call_id,
                        content: if output.is_error {
                            format!("Error: {}", output.content)
                        } else {
                            output.content
                        },
                        is_error: output.is_error,
                    },
                    Err(e) => ToolExecutionResult {
                        tool_call_id,
                        content: format!("Error executing tool: {}", e),
                        is_error: true,
                    },
                }
            }
        })
        .collect();

    join_all(futures).await
}

/// Execute multiple LLM completion requests in parallel.
///
/// Returns results in the same order as the input requests.
pub async fn complete_parallel(
    provider: &dyn Provider,
    requests: Vec<CompletionRequest>,
) -> Vec<Result<CompletionResponse, Error>> {
    use futures::future::join_all;

    let futures: Vec<_> = requests
        .into_iter()
        .map(|request| provider.complete(request))
        .collect();

    join_all(futures).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_task_manager_spawn() {
        let manager = TaskManager::new();

        let handle = manager.spawn("test-task", async { 42 });

        assert_eq!(handle.name, "test-task");
        assert!(handle.is_running() || handle.is_finished());

        let result = handle.join().await.unwrap();
        assert_eq!(result, 42);
    }

    #[tokio::test]
    async fn test_task_manager_list() {
        let manager = TaskManager::new();

        let handle1 = manager.spawn("task-1", async {
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
            1
        });

        let handle2 = manager.spawn("task-2", async {
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
            2
        });

        // Give tasks time to register
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;

        let tasks = manager.list_tasks().await;
        assert_eq!(tasks.len(), 2);

        // Clean up
        handle1.join().await.unwrap();
        handle2.join().await.unwrap();
    }

    #[tokio::test]
    async fn test_task_state() {
        let state = TaskState::Running;
        assert!(!state.is_finished());

        let state = TaskState::Completed;
        assert!(state.is_finished());

        let state = TaskState::Failed("error".to_string());
        assert!(state.is_finished());

        let state = TaskState::Cancelled;
        assert!(state.is_finished());
    }

    #[tokio::test]
    async fn test_task_id_display() {
        let id = TaskId(42);
        assert_eq!(format!("{}", id), "42");
    }
}
