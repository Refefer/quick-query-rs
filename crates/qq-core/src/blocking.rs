//! Blocking utilities for CPU-intensive operations.
//!
//! This module provides helpers for offloading CPU-bound work to Tokio's
//! blocking threadpool, preventing async runtime starvation.

use crate::Error;

/// Execute a CPU-intensive closure on Tokio's blocking threadpool.
///
/// Use this for operations like HTML parsing, regex matching over large
/// datasets, or any other CPU-bound work that could block the async runtime.
///
/// # Example
///
/// ```ignore
/// let result = run_blocking(|| {
///     // CPU-intensive work here
///     expensive_computation()
/// }).await?;
/// ```
pub async fn run_blocking<F, T>(f: F) -> Result<T, Error>
where
    F: FnOnce() -> T + Send + 'static,
    T: Send + 'static,
{
    tokio::task::spawn_blocking(f)
        .await
        .map_err(|e| Error::Unknown(format!("Blocking task failed: {}", e)))
}
