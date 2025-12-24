use ractor::{Actor, ActorProcessingErr, ActorRef, SpawnErr};
use std::collections::HashMap;
use std::time::{Duration, Instant};
use tokio::time::interval;
use tracing::{debug, info, warn};
use uuid::Uuid;

#[derive(Debug, Clone)]
pub struct TaskManagerConfig {
    pub enable_watchdog_timers: bool,
    pub enable_watchdog_logging: bool,
    pub default_watchdog_timeout: Duration,
}

impl Default for TaskManagerConfig {
    fn default() -> Self {
        Self {
            enable_watchdog_timers: false,
            enable_watchdog_logging: false,
            default_watchdog_timeout: Duration::from_secs(5),
        }
    }
}

#[derive(Debug, Clone)]
pub struct WatchdogConfig {
    pub enable_timers: bool,
    pub enable_logging: bool,
    pub timeout: Option<Duration>,
}

pub struct TaskActor;

pub struct TaskState {
    id: Uuid,
    name: String,
    _created_at: Instant,
    watchdog: Option<ActorRef<WatchdogActor>>,
    completion_notify: Option<tokio::sync::broadcast::Sender<()>>,
}

impl TaskState {
    pub fn new(id: Uuid, name: String, watchdog: Option<ActorRef<WatchdogActor>>) -> Self {
        Self {
            id,
            name,
            _created_at: Instant::now(),
            watchdog,
            completion_notify: None,
        }
    }

    pub fn with_completion_notify(mut self, sender: tokio::sync::broadcast::Sender<()>) -> Self {
        self.completion_notify = Some(sender);
        self
    }
}

#[async_trait::async_trait]
impl Actor for TaskActor {
    type Msg = TaskMsg;
    type State = TaskState;
    type Arguments = TaskState;

    async fn pre_start(
        &self,
        _myself: ActorRef<Self>,
        args: Self::Arguments,
    ) -> Result<Self::State, ActorProcessingErr> {
        info!("Task '{}' ({}) started", args.name, args.id);
        Ok(args)
    }

    async fn post_stop(
        &self,
        _myself: ActorRef<Self>,
        state: &mut Self::State,
    ) -> Result<(), ActorProcessingErr> {
        info!("Task '{}' ({}) stopped", state.name, state.id);
        // Notify any waiters that the task has completed
        if let Some(notify) = &state.completion_notify {
            let _ = notify.send(());
        }
        Ok(())
    }

    async fn handle(
        &self,
        myself: ActorRef<Self>,
        msg: Self::Msg,
        state: &mut Self::State,
    ) -> Result<(), ActorProcessingErr> {
        match msg {
            TaskMsg::ResetWatchdog => {
                if let Some(watchdog) = &state.watchdog {
                    let _ = watchdog.cast(WatchdogMsg::Reset);
                }
            }
            TaskMsg::GetContext(reply) => {
                let context = TaskContext {
                    id: state.id,
                    name: state.name.clone(),
                    actor_ref: myself.clone(),
                };
                let _ = reply.send(context);
            }
            TaskMsg::Shutdown => {
                myself.stop(None);
            }
        }
        Ok(())
    }
}

#[derive(Debug)]
pub enum TaskMsg {
    ResetWatchdog,
    GetContext(tokio::sync::oneshot::Sender<TaskContext>),
    Shutdown,
}

pub struct WatchdogActor;

pub struct WatchdogState {
    task_id: Uuid,
    task_name: String,
    timeout: Duration,
    enable_logging: bool,
    last_reset: Instant,
}

impl WatchdogState {
    pub fn new(task_id: Uuid, task_name: String, timeout: Duration, enable_logging: bool) -> Self {
        Self {
            task_id,
            task_name,
            timeout,
            enable_logging,
            last_reset: Instant::now(),
        }
    }
}

#[async_trait::async_trait]
impl Actor for WatchdogActor {
    type Msg = WatchdogMsg;
    type State = WatchdogState;
    type Arguments = WatchdogState;

    async fn pre_start(
        &self,
        myself: ActorRef<Self>,
        args: Self::Arguments,
    ) -> Result<Self::State, ActorProcessingErr> {
        let check_interval = args.timeout / 2;
        let myself_clone = myself.clone();

        tokio::spawn(async move {
            let mut interval_timer = interval(check_interval);

            loop {
                interval_timer.tick().await;

                // Send check message to self
                if myself_clone.cast(WatchdogMsg::Check).is_err() {
                    break;
                }
            }
        });

        Ok(args)
    }

    async fn handle(
        &self,
        _myself: ActorRef<Self>,
        msg: Self::Msg,
        state: &mut Self::State,
    ) -> Result<(), ActorProcessingErr> {
        match msg {
            WatchdogMsg::Reset => {
                let elapsed = state.last_reset.elapsed();
                if state.enable_logging {
                    debug!(
                        "Watchdog reset for task '{}' ({}): {:?} since last reset",
                        state.task_name, state.task_id, elapsed
                    );
                }
                state.last_reset = Instant::now();
            }
            WatchdogMsg::Check => {
                let elapsed = state.last_reset.elapsed();
                if elapsed >= state.timeout {
                    warn!(
                        "Watchdog timeout for task '{}' ({}): no heartbeat for {:?}",
                        state.task_name, state.task_id, elapsed
                    );
                }
            }
        }
        Ok(())
    }
}

#[derive(Debug)]
pub enum WatchdogMsg {
    Reset,
    Check,
}

pub struct TaskManagerActor;

pub struct TaskManagerState {
    config: TaskManagerConfig,
    tasks: HashMap<Uuid, ActorRef<TaskActor>>,
    watchdogs: HashMap<Uuid, ActorRef<WatchdogActor>>,
    completion_channels: HashMap<Uuid, tokio::sync::broadcast::Sender<()>>,
}

impl TaskManagerState {
    pub fn new(config: TaskManagerConfig) -> Self {
        Self {
            config,
            tasks: HashMap::new(),
            watchdogs: HashMap::new(),
            completion_channels: HashMap::new(),
        }
    }
}

#[async_trait::async_trait]
impl Actor for TaskManagerActor {
    type Msg = TaskManagerMsg;
    type State = TaskManagerState;
    type Arguments = TaskManagerState;

    async fn pre_start(
        &self,
        _myself: ActorRef<Self>,
        args: Self::Arguments,
    ) -> Result<Self::State, ActorProcessingErr> {
        Ok(args)
    }

    async fn post_stop(
        &self,
        _myself: ActorRef<Self>,
        state: &mut Self::State,
    ) -> Result<(), ActorProcessingErr> {
        info!("Task manager shutting down");
        // Stop all tasks
        for task in state.tasks.values() {
            task.stop(None);
        }
        // Stop all watchdogs
        for watchdog in state.watchdogs.values() {
            watchdog.stop(None);
        }
        Ok(())
    }

    async fn handle(
        &self,
        myself: ActorRef<Self>,
        msg: Self::Msg,
        state: &mut Self::State,
    ) -> Result<(), ActorProcessingErr> {
        match msg {
            TaskManagerMsg::CreateTask {
                name,
                watchdog_config,
                reply,
            } => {
                let task_id = Uuid::new_v4();

                // Setup watchdog if enabled
                let watchdog = if watchdog_config
                    .as_ref()
                    .map(|c| c.enable_timers)
                    .unwrap_or(state.config.enable_watchdog_timers)
                {
                    let timeout = watchdog_config
                        .as_ref()
                        .and_then(|c| c.timeout)
                        .unwrap_or(state.config.default_watchdog_timeout);

                    let enable_logging = watchdog_config
                        .as_ref()
                        .map(|c| c.enable_logging)
                        .unwrap_or(state.config.enable_watchdog_logging);

                    let watchdog_state =
                        WatchdogState::new(task_id, name.clone(), timeout, enable_logging);

                    let (watchdog_ref, _) = Actor::spawn(None, WatchdogActor, watchdog_state)
                        .await
                        .map_err(|_| ActorProcessingErr::from("Failed to spawn watchdog"))?;
                    state.watchdogs.insert(task_id, watchdog_ref.clone());
                    Some(watchdog_ref)
                } else {
                    None
                };

                // Create completion channel for this task
                let (completion_tx, _) = tokio::sync::broadcast::channel(1);

                // Create task actor
                let task_state = TaskState::new(task_id, name.clone(), watchdog)
                    .with_completion_notify(completion_tx.clone());

                let (task_ref, _) = Actor::spawn(None, TaskActor, task_state)
                    .await
                    .map_err(|_| ActorProcessingErr::from("Failed to spawn task"))?;
                state.tasks.insert(task_id, task_ref.clone());
                state.completion_channels.insert(task_id, completion_tx);

                let handle = TaskHandle {
                    id: task_id,
                    name: name.clone(),
                    actor_ref: task_ref,
                };

                let _ = reply.send(Ok(handle));
            }
            TaskManagerMsg::WaitForTask {
                task_id,
                timeout_duration,
                reply,
            } => {
                if let Some(completion_tx) = state.completion_channels.get(&task_id) {
                    let mut completion_rx = completion_tx.subscribe();

                    tokio::spawn(async move {
                        let result = if let Some(timeout_dur) = timeout_duration {
                            match tokio::time::timeout(timeout_dur, completion_rx.recv()).await {
                                Ok(Ok(_)) => Ok(()),
                                Ok(Err(_)) => Ok(()), // Channel closed, task completed
                                Err(_) => Err(TaskError::Timeout),
                            }
                        } else {
                            // Wait indefinitely
                            match completion_rx.recv().await {
                                Ok(_) | Err(_) => Ok(()), // Either received signal or channel closed
                            }
                        };
                        let _ = reply.send(result);
                    });
                } else {
                    let _ = reply.send(Err(TaskError::NotFound));
                }
            }
            TaskManagerMsg::CancelTask {
                task_id,
                timeout_duration: _,
                reply,
            } => {
                if let Some(task_ref) = state.tasks.remove(&task_id) {
                    task_ref.stop(None);

                    // Remove watchdog
                    if let Some(watchdog) = state.watchdogs.remove(&task_id) {
                        watchdog.stop(None);
                    }

                    // Remove completion channel
                    state.completion_channels.remove(&task_id);

                    let _ = reply.send(Ok(()));
                } else {
                    let _ = reply.send(Err(TaskError::NotFound));
                }
            }
            TaskManagerMsg::CurrentTasks { reply } => {
                let mut tasks_info = Vec::new();

                for (id, task_ref) in &state.tasks {
                    // Get task info
                    let (tx, rx) = tokio::sync::oneshot::channel();
                    let _ = task_ref.cast(TaskMsg::GetContext(tx));
                    if let Ok(context) = rx.await {
                        tasks_info.push(TaskInfo {
                            id: context.id,
                            name: context.name,
                            created_at: Instant::now(), // Note: would need to store this
                            watchdog_enabled: state.watchdogs.contains_key(id),
                        });
                    }
                }

                let _ = reply.send(tasks_info);
            }
            TaskManagerMsg::Shutdown => {
                myself.stop(None);
            }
        }
        Ok(())
    }
}

#[derive(Debug)]
pub enum TaskManagerMsg {
    CreateTask {
        name: String,
        watchdog_config: Option<WatchdogConfig>,
        reply: tokio::sync::oneshot::Sender<Result<TaskHandle, TaskError>>,
    },
    WaitForTask {
        task_id: Uuid,
        timeout_duration: Option<Duration>,
        reply: tokio::sync::oneshot::Sender<Result<(), TaskError>>,
    },
    CancelTask {
        task_id: Uuid,
        timeout_duration: Option<Duration>,
        reply: tokio::sync::oneshot::Sender<Result<(), TaskError>>,
    },
    CurrentTasks {
        reply: tokio::sync::oneshot::Sender<Vec<TaskInfo>>,
    },
    Shutdown,
}

#[derive(Clone, Debug)]
pub struct TaskContext {
    pub id: Uuid,
    pub name: String,
    actor_ref: ActorRef<TaskActor>,
}

impl TaskContext {
    pub fn reset_watchdog(&self) {
        let _ = self.actor_ref.cast(TaskMsg::ResetWatchdog);
    }

    pub fn watchdog_enabled(&self) -> bool {
        true
    }
}

#[derive(Clone, Debug)]
pub struct TaskHandle {
    pub id: Uuid,
    pub name: String,
    actor_ref: ActorRef<TaskActor>,
}

impl TaskHandle {
    pub fn actor_ref(&self) -> &ActorRef<TaskActor> {
        &self.actor_ref
    }
}

#[derive(Debug, Clone)]
pub struct TaskInfo {
    pub id: Uuid,
    pub name: String,
    pub created_at: Instant,
    pub watchdog_enabled: bool,
}

#[derive(Debug, thiserror::Error)]
pub enum TaskError {
    #[error("Task timed out")]
    Timeout,
    #[error("Task not found")]
    NotFound,
    #[error("Spawn error: {0}")]
    SpawnError(String),
    #[error("Actor error: {0}")]
    ActorError(String),
}

pub async fn create_task_manager(
    config: TaskManagerConfig,
) -> Result<ActorRef<TaskManagerActor>, SpawnErr> {
    let state = TaskManagerState::new(config);
    let (actor_ref, _) = Actor::spawn(None, TaskManagerActor, state).await?;
    Ok(actor_ref)
}

pub async fn create_task<F, Fut>(
    manager: &ActorRef<TaskManagerActor>,
    name: String,
    future: F,
    watchdog_config: Option<WatchdogConfig>,
) -> Result<TaskHandle, TaskError>
where
    F: FnOnce(TaskContext) -> Fut + Send + 'static,
    Fut: std::future::Future<Output = ()> + Send + 'static,
{
    let (tx, rx) = tokio::sync::oneshot::channel();
    let _ = manager.cast(TaskManagerMsg::CreateTask {
        name,
        watchdog_config,
        reply: tx,
    });
    let handle = rx
        .await
        .map_err(|e| TaskError::ActorError(e.to_string()))??;

    let (ctx_tx, ctx_rx) = tokio::sync::oneshot::channel();
    let _ = handle.actor_ref.cast(TaskMsg::GetContext(ctx_tx));
    let context = ctx_rx
        .await
        .map_err(|e| TaskError::ActorError(e.to_string()))?;

    tokio::spawn(async move {
        future(context).await;
    });

    Ok(handle)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_basic_task_creation() {
        let config = TaskManagerConfig::default();
        let manager = create_task_manager(config).await.unwrap();

        let handle = create_task(
            &manager,
            "test_task".to_string(),
            |ctx| async move {
                println!("Task {} running", ctx.name);
                tokio::time::sleep(Duration::from_millis(100)).await;
                let _ = ctx.actor_ref.cast(TaskMsg::Shutdown);
            },
            None,
        )
        .await
        .unwrap();

        let (tx, rx) = tokio::sync::oneshot::channel();
        let _ = manager.cast(TaskManagerMsg::WaitForTask {
            task_id: handle.id,
            timeout_duration: None,
            reply: tx,
        });
        let result = rx.await.unwrap();
        assert!(result.is_ok());

        // Ensure all actors are stopped after test
        let _ = manager.cast(TaskManagerMsg::Shutdown);
        tokio::time::sleep(Duration::from_millis(200)).await;
    }

    #[tokio::test]
    async fn test_watchdog_functionality() {
        let mut config = TaskManagerConfig::default();
        config.enable_watchdog_timers = true;
        config.enable_watchdog_logging = true;
        config.default_watchdog_timeout = Duration::from_millis(100);

        let manager = create_task_manager(config).await.unwrap();

        let handle = create_task(
            &manager,
            "watchdog_test".to_string(),
            |ctx| async move {
                for i in 0..5 {
                    println!("Working on iteration {}", i);
                    tokio::time::sleep(Duration::from_millis(50)).await;
                    ctx.reset_watchdog();
                }
                let _ = ctx.actor_ref.cast(TaskMsg::Shutdown);
            },
            None,
        )
        .await
        .unwrap();

        let (tx, rx) = tokio::sync::oneshot::channel();
        let _ = manager.cast(TaskManagerMsg::WaitForTask {
            task_id: handle.id,
            timeout_duration: Some(Duration::from_secs(5)),
            reply: tx,
        });
        let result = rx.await.unwrap();
        assert!(result.is_ok());

        // Ensure all actors are stopped after test
        let _ = manager.cast(TaskManagerMsg::Shutdown);
        tokio::time::sleep(Duration::from_millis(200)).await;
    }

    #[tokio::test]
    async fn test_task_cancellation() {
        let config = TaskManagerConfig::default();
        let manager = create_task_manager(config).await.unwrap();

        let handle = create_task(
            &manager,
            "long_task".to_string(),
            |_ctx| async move {
                tokio::time::sleep(Duration::from_secs(10)).await;
            },
            None,
        )
        .await
        .unwrap();

        // Cancel after a short delay
        tokio::time::sleep(Duration::from_millis(100)).await;
        let (tx, rx) = tokio::sync::oneshot::channel();
        let _ = manager.cast(TaskManagerMsg::CancelTask {
            task_id: handle.id,
            timeout_duration: Some(Duration::from_secs(1)),
            reply: tx,
        });
        let result = rx.await.unwrap();
        assert!(result.is_ok());
    }
}
