//! Fail-closed policy for the experimental Code Mode security boundary.
//!
//! JavaScript and worker IPC are untrusted. Only operation names declared in
//! [`crate::operation_registry`] may cross into the trusted parent. This module owns the
//! parent-side budgets which cannot be delegated to QuickJS: operation classes, aggregate data
//! movement, and server-wide fairness. The worker receives only the complementary VM limits.

use crate::operation_registry::{OperationClass, operation};
use serde::Serialize;
use std::sync::{
    Arc,
    atomic::{AtomicU32, Ordering},
};
use tokio::sync::{OwnedSemaphorePermit, Semaphore};
use wit_quickjs_spike::{Limits, MAX_SCRIPT_BYTES};

pub const MAX_PAGES: u32 = 16;
pub const MAX_SNAPSHOTS: u32 = 4;
pub const MAX_SERVER_WORKERS: usize = 16;
pub const MAX_SERVER_INVOCATIONS: usize = 16;
pub const MAX_SERVER_HOST_OPERATIONS: usize = 32;

/// Validated default budgets. These are deliberately not sourced from the worker environment or
/// invocation arguments, so untrusted code cannot raise its own authority.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct CodeModePolicy {
    pub source_bytes: usize,
    pub ipc_frame_bytes: usize,
    pub wall_time_ms: u64,
    pub memory_bytes: usize,
    pub stack_bytes: usize,
    pub host_calls: u32,
    pub concurrent_host_calls: u32,
    pub pages: u32,
    pub snapshots: u32,
    pub host_result_bytes: usize,
    pub cumulative_host_result_bytes: usize,
    pub final_result_bytes: usize,
    pub log_bytes: usize,
    pub server_workers: usize,
    pub server_invocations: usize,
    pub server_host_operations: usize,
}

impl Default for CodeModePolicy {
    fn default() -> Self {
        Self {
            source_bytes: 32 * 1024,
            ipc_frame_bytes: 72 * 1024,
            wall_time_ms: 10_000,
            memory_bytes: 16 * 1024 * 1024,
            stack_bytes: 256 * 1024,
            host_calls: 16,
            concurrent_host_calls: 4,
            pages: 8,
            snapshots: 2,
            host_result_bytes: 64 * 1024,
            cumulative_host_result_bytes: 256 * 1024,
            final_result_bytes: 48 * 1024,
            log_bytes: 8 * 1024,
            server_workers: 4,
            server_invocations: 4,
            server_host_operations: 8,
        }
    }
}

impl CodeModePolicy {
    pub fn validate(&self) -> Result<(), PolicyError> {
        if self.source_bytes == 0 || self.source_bytes > MAX_SCRIPT_BYTES {
            return Err(PolicyError::configuration("source_bytes"));
        }
        if self.ipc_frame_bytes != wit_quickjs_spike::MAX_FRAME_BYTES {
            return Err(PolicyError::configuration("ipc_frame_bytes"));
        }
        if self.pages == 0 || self.pages > MAX_PAGES {
            return Err(PolicyError::configuration("pages"));
        }
        if self.snapshots == 0 || self.snapshots > MAX_SNAPSHOTS {
            return Err(PolicyError::configuration("snapshots"));
        }
        if self.server_workers == 0 || self.server_workers > MAX_SERVER_WORKERS {
            return Err(PolicyError::configuration("server_workers"));
        }
        if self.server_invocations == 0 || self.server_invocations > MAX_SERVER_INVOCATIONS {
            return Err(PolicyError::configuration("server_invocations"));
        }
        if self.server_host_operations == 0
            || self.server_host_operations > MAX_SERVER_HOST_OPERATIONS
        {
            return Err(PolicyError::configuration("server_host_operations"));
        }
        self.worker_limits()
            .validate()
            .map_err(|_| PolicyError::configuration("worker_limits"))
    }

    pub fn check_source(&self, source: &str) -> Result<(), PolicyError> {
        if source.len() > self.source_bytes {
            return Err(PolicyError::limit(
                "source_bytes_limit",
                "JavaScript source exceeds the byte budget",
            ));
        }
        Ok(())
    }

    pub fn worker_limits(&self) -> Limits {
        Limits {
            wall_time_ms: self.wall_time_ms,
            memory_bytes: self.memory_bytes,
            stack_bytes: self.stack_bytes,
            max_host_calls: self.host_calls,
            max_concurrent_host_calls: self.concurrent_host_calls,
            max_host_result_bytes: self.host_result_bytes,
            max_cumulative_host_result_bytes: self.cumulative_host_result_bytes,
            max_result_bytes: self.final_result_bytes,
            max_log_bytes: self.log_bytes,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct PolicyError {
    pub code: &'static str,
    pub message: String,
}

impl PolicyError {
    fn configuration(field: &str) -> Self {
        Self {
            code: "invalid_policy",
            message: format!("invalid Code Mode policy field: {field}"),
        }
    }

    fn limit(code: &'static str, message: &'static str) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }
}

#[derive(Debug)]
pub struct ServerCapacity {
    workers: Arc<Semaphore>,
    invocations: Arc<Semaphore>,
    host_operations: Arc<Semaphore>,
}

impl ServerCapacity {
    pub fn new(policy: &CodeModePolicy) -> Self {
        Self {
            workers: Arc::new(Semaphore::new(policy.server_workers)),
            invocations: Arc::new(Semaphore::new(policy.server_invocations)),
            host_operations: Arc::new(Semaphore::new(policy.server_host_operations)),
        }
    }

    pub fn try_start(&self) -> Result<InvocationPermit, PolicyError> {
        let invocation = Arc::clone(&self.invocations)
            .try_acquire_owned()
            .map_err(|_| {
                PolicyError::limit(
                    "server_invocations_limit",
                    "server invocation capacity is exhausted",
                )
            })?;
        let worker = Arc::clone(&self.workers).try_acquire_owned().map_err(|_| {
            PolicyError::limit(
                "server_workers_limit",
                "server worker capacity is exhausted",
            )
        })?;
        Ok(InvocationPermit {
            _invocation: invocation,
            _worker: worker,
        })
    }

    pub async fn acquire_host_operation(&self) -> Result<OwnedSemaphorePermit, PolicyError> {
        Arc::clone(&self.host_operations)
            .acquire_owned()
            .await
            .map_err(|_| {
                PolicyError::limit(
                    "server_host_operations_limit",
                    "server host-operation capacity is unavailable",
                )
            })
    }
}

#[derive(Debug)]
pub struct InvocationPermit {
    _invocation: OwnedSemaphorePermit,
    _worker: OwnedSemaphorePermit,
}

pub struct InvocationBudget {
    max_pages: u32,
    max_snapshots: u32,
    pages: AtomicU32,
    snapshots: AtomicU32,
}

impl InvocationBudget {
    pub fn new(policy: &CodeModePolicy) -> Self {
        Self {
            max_pages: policy.pages,
            max_snapshots: policy.snapshots,
            pages: AtomicU32::new(0),
            snapshots: AtomicU32::new(0),
        }
    }

    /// Reserve authorization and cost before privileged dispatch. Failed operations still consume
    /// a unit, preventing cheap probing from bypassing the budget.
    pub fn reserve_operation(&self, name: &str) -> Result<(), PolicyError> {
        let descriptor = operation(name).ok_or_else(|| {
            PolicyError::limit(
                "capability_denied",
                "host operation is not registered for Code Mode",
            )
        })?;
        match descriptor.classification {
            OperationClass::Snapshot => charge_count(
                &self.snapshots,
                self.max_snapshots,
                "snapshots_limit",
                "snapshot budget exhausted",
            ),
            OperationClass::Discovery | OperationClass::Read | OperationClass::Search => {
                charge_count(
                    &self.pages,
                    self.max_pages,
                    "pages_limit",
                    "page budget exhausted",
                )
            }
        }
    }
}

fn charge_count(
    counter: &AtomicU32,
    maximum: u32,
    code: &'static str,
    message: &'static str,
) -> Result<(), PolicyError> {
    counter
        .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |current| {
            current.checked_add(1).filter(|next| *next <= maximum)
        })
        .map(|_| ())
        .map_err(|_| PolicyError::limit(code, message))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_valid_and_do_not_exceed_absolute_maxima() {
        let policy = CodeModePolicy::default();
        policy.validate().unwrap();
        assert!(policy.source_bytes <= MAX_SCRIPT_BYTES);
        assert!(policy.host_calls <= wit_quickjs_spike::MAX_HOST_CALLS);
        assert!(policy.pages <= MAX_PAGES);
        assert!(policy.snapshots <= MAX_SNAPSHOTS);
        assert!(policy.server_workers <= MAX_SERVER_WORKERS);
        assert!(policy.server_invocations <= MAX_SERVER_INVOCATIONS);
    }

    #[test]
    fn unregistered_operations_fail_before_any_budgeted_dispatch() {
        let budget = InvocationBudget::new(&CodeModePolicy::default());
        let error = budget.reserve_operation("filesystem_read").unwrap_err();
        assert_eq!(error.code, "capability_denied");
    }

    #[test]
    fn repeated_pagination_and_snapshot_fanout_are_bounded() {
        let policy = CodeModePolicy {
            pages: 2,
            snapshots: 1,
            ..CodeModePolicy::default()
        };
        let budget = InvocationBudget::new(&policy);
        budget.reserve_operation("wit_search_code").unwrap();
        budget.reserve_operation("wit_search_code").unwrap();
        assert_eq!(
            budget
                .reserve_operation("wit_search_code")
                .unwrap_err()
                .code,
            "pages_limit"
        );
        budget.reserve_operation("wit_open").unwrap();
        assert_eq!(
            budget.reserve_operation("wit_open").unwrap_err().code,
            "snapshots_limit"
        );
    }

    #[tokio::test]
    async fn server_capacity_fails_closed_without_oversubscription() {
        let policy = CodeModePolicy {
            server_workers: 1,
            server_invocations: 1,
            ..CodeModePolicy::default()
        };
        let capacity = ServerCapacity::new(&policy);
        let first = capacity.try_start().unwrap();
        assert_eq!(
            capacity.try_start().unwrap_err().code,
            "server_invocations_limit"
        );
        drop(first);
        capacity.try_start().unwrap();
    }

    #[tokio::test]
    async fn server_host_capacity_queues_without_oversubscription() {
        let policy = CodeModePolicy {
            server_host_operations: 1,
            ..CodeModePolicy::default()
        };
        let capacity = ServerCapacity::new(&policy);
        let first = capacity.acquire_host_operation().await.unwrap();
        assert!(
            tokio::time::timeout(
                std::time::Duration::from_millis(20),
                capacity.acquire_host_operation()
            )
            .await
            .is_err()
        );
        drop(first);
        let _permit = capacity.acquire_host_operation().await.unwrap();
    }
}
