use kameo::actor::{ActorRef, WeakActorRef};
use kameo::message::{Context, Message};
use kameo::{error::BoxError, Actor};
use std::collections::HashMap;
use std::time::{Duration, Instant};
use tokio::time::interval;
use tracing::{debug, error, info, warn};
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

pub struct TaskActor {
    id: Uuid,
    name: String,
    _created_at: Instant,
    watchdog: Option<ActorRef<WatchdogActor>>,
}

impl TaskActor {
    pub fn new(id: Uuid, name: String, watchdog: Option<ActorRef<WatchdogActor>>) -> Self {
        Self {
            id,
            name,
            _created_at: Instant::now(),
            watchdog,
        }
    }
}

impl Actor for TaskActor {
    type Mailbox = kameo::mailbox::unbounded::UnboundedMailbox<Self>;

    async fn on_start(&mut self, _actor_ref: ActorRef<Self>) -> Result<(), BoxError> {
        info!("Task '{}' ({}) started", self.name, self.id);
        Ok(())
    }

    async fn on_stop(
        &mut self,
        _actor_ref: WeakActorRef<Self>,
        _reason: kameo::error::ActorStopReason,
    ) -> Result<(), BoxError> {
        info!("Task '{}' ({}) stopped", self.name, self.id);
        Ok(())
    }
}

pub enum TaskMsg {
    ResetWatchdog,
    GetContext(tokio::sync::oneshot::Sender<TaskContext>),
    Shutdown,
}

impl Message<TaskMsg> for TaskActor {
    type Reply = ();

    async fn handle(&mut self, msg: TaskMsg, ctx: Context<'_, Self, ()>) -> Self::Reply {
        match msg {
            TaskMsg::ResetWatchdog => {
                if let Some(watchdog) = &self.watchdog {
                    let _ = watchdog.tell(WatchdogMsg::Reset).await;
                }
            }
            TaskMsg::GetContext(reply) => {
                let context = TaskContext {
                    id: self.id,
                    name: self.name.clone(),
                    actor_ref: ctx.actor_ref().clone(),
                };
                let _ = reply.send(context);
            }
            TaskMsg::Shutdown => {
                let _ = ctx.actor_ref().stop_gracefully().await;
            }
        }
    }
}

pub struct WatchdogActor {
    task_id: Uuid,
    task_name: String,
    timeout: Duration,
    enable_logging: bool,
    last_reset: Instant,
}

impl WatchdogActor {
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

impl Actor for WatchdogActor {
    type Mailbox = kameo::mailbox::unbounded::UnboundedMailbox<Self>;

    async fn on_start(&mut self, actor_ref: ActorRef<Self>) -> Result<(), BoxError> {
        let weak_ref = actor_ref.downgrade();
        let check_interval = self.timeout / 2;

        tokio::spawn(async move {
            let mut interval_timer = interval(check_interval);

            loop {
                interval_timer.tick().await;

                if let Some(actor_ref) = weak_ref.upgrade() {
                    let _ = actor_ref.tell(WatchdogMsg::Check).await;
                } else {
                    break;
                }
            }
        });

        Ok(())
    }
}

pub enum WatchdogMsg {
    Reset,
    Check,
}

impl Message<WatchdogMsg> for WatchdogActor {
    type Reply = ();

    async fn handle(&mut self, msg: WatchdogMsg, _ctx: Context<'_, Self, ()>) -> Self::Reply {
        match msg {
            WatchdogMsg::Reset => {
                let elapsed = self.last_reset.elapsed();
                if self.enable_logging {
                    debug!(
                        "Watchdog reset for task '{}' ({}): {:?} since last reset",
                        self.task_name, self.task_id, elapsed
                    );
                }
                self.last_reset = Instant::now();
            }
            WatchdogMsg::Check => {
                let elapsed = self.last_reset.elapsed();
                if elapsed >= self.timeout {
                    warn!(
                        "Watchdog timeout for task '{}' ({}): no heartbeat for {:?}",
                        self.task_name, self.task_id, elapsed
                    );
                }
            }
        }
    }
}

pub struct TaskManagerActor {
    config: TaskManagerConfig,
    tasks: HashMap<Uuid, ActorRef<TaskActor>>,
    watchdogs: HashMap<Uuid, ActorRef<WatchdogActor>>,
}

impl TaskManagerActor {
    pub fn new(config: TaskManagerConfig) -> Self {
        Self {
            config,
            tasks: HashMap::new(),
            watchdogs: HashMap::new(),
        }
    }
}

impl Actor for TaskManagerActor {
    type Mailbox = kameo::mailbox::unbounded::UnboundedMailbox<Self>;

    async fn on_stop(
        &mut self,
        _actor_ref: WeakActorRef<Self>,
        _reason: kameo::error::ActorStopReason,
    ) -> Result<(), BoxError> {
        info!("Task manager shutting down");
        // Stop all tasks
        for task in self.tasks.values() {
            let _ = task.stop_gracefully().await;
        }
        // Stop all watchdogs
        for watchdog in self.watchdogs.values() {
            let _ = watchdog.stop_gracefully().await;
        }
        Ok(())
    }
}

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

impl Message<TaskManagerMsg> for TaskManagerActor {
    type Reply = ();

    async fn handle(&mut self, msg: TaskManagerMsg, ctx: Context<'_, Self, ()>) -> Self::Reply {
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
                    .unwrap_or(self.config.enable_watchdog_timers)
                {
                    let timeout = watchdog_config
                        .as_ref()
                        .and_then(|c| c.timeout)
                        .unwrap_or(self.config.default_watchdog_timeout);

                    let enable_logging = watchdog_config
                        .as_ref()
                        .map(|c| c.enable_logging)
                        .unwrap_or(self.config.enable_watchdog_logging);

                    let watchdog_actor =
                        WatchdogActor::new(task_id, name.clone(), timeout, enable_logging);

                    let watchdog_ref = kameo::spawn(watchdog_actor);
                    self.watchdogs.insert(task_id, watchdog_ref.clone());
                    Some(watchdog_ref)
                } else {
                    None
                };

                // Create task actor
                let task_actor = TaskActor::new(task_id, name.clone(), watchdog);

                let task_ref = kameo::spawn(task_actor);
                self.tasks.insert(task_id, task_ref.clone());

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
                if let Some(task_ref) = self.tasks.get(&task_id) {
                    let task_ref = task_ref.clone();

                    tokio::spawn(async move {
                        let result = if let Some(timeout_dur) = timeout_duration {
                            match tokio::time::timeout(timeout_dur, task_ref.wait_for_stop()).await
                            {
                                Ok(_) => Ok(()),
                                Err(_) => Err(TaskError::Timeout),
                            }
                        } else {
                            task_ref.wait_for_stop().await;
                            Ok(())
                        };
                        let _ = reply.send(result);
                    });
                } else {
                    let _ = reply.send(Err(TaskError::NotFound));
                }
            }
            TaskManagerMsg::CancelTask {
                task_id,
                timeout_duration,
                reply,
            } => {
                if let Some(task_ref) = self.tasks.remove(&task_id) {
                    let _ = task_ref.stop_gracefully().await;

                    if let Some(timeout_dur) = timeout_duration {
                        tokio::spawn(async move {
                            match tokio::time::timeout(timeout_dur, task_ref.wait_for_stop()).await
                            {
                                Ok(_) => {
                                    let _ = reply.send(Ok(()));
                                }
                                Err(_) => {
                                    warn!("Task {} did not cancel within timeout", task_id);
                                    let _ = reply.send(Ok(()));
                                }
                            }
                        });
                    } else {
                        tokio::spawn(async move {
                            task_ref.wait_for_stop().await;
                            let _ = reply.send(Ok(()));
                        });
                    }

                    // Remove watchdog
                    if let Some(watchdog) = self.watchdogs.remove(&task_id) {
                        let _ = watchdog.stop_gracefully().await;
                    }
                } else {
                    let _ = reply.send(Err(TaskError::NotFound));
                }
            }
            TaskManagerMsg::CurrentTasks { reply } => {
                let mut tasks_info = Vec::new();

                for (id, task_ref) in &self.tasks {
                    // Get task info
                    let (tx, rx) = tokio::sync::oneshot::channel();
                    let _ = task_ref.tell(TaskMsg::GetContext(tx)).await;
                    if let Ok(context) = rx.await {
                        tasks_info.push(TaskInfo {
                            id: context.id,
                            name: context.name,
                            created_at: Instant::now(), // Note: would need to store this
                            watchdog_enabled: self.watchdogs.contains_key(id),
                        });
                    }
                }

                let _ = reply.send(tasks_info);
            }
            TaskManagerMsg::Shutdown => {
                let _ = ctx.actor_ref().stop_gracefully().await;
            }
        }
    }
}

#[derive(Clone)]
pub struct TaskContext {
    pub id: Uuid,
    pub name: String,
    actor_ref: ActorRef<TaskActor>,
}

impl TaskContext {
    pub async fn reset_watchdog(&self) {
        let _ = self.actor_ref.tell(TaskMsg::ResetWatchdog).await;
    }

    pub fn watchdog_enabled(&self) -> bool {
        true
    }
}

#[derive(Clone)]
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
) -> Result<ActorRef<TaskManagerActor>, BoxError> {
    let manager = TaskManagerActor::new(config);
    Ok(kameo::spawn(manager))
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
    let _ = manager
        .tell(TaskManagerMsg::CreateTask {
            name,
            watchdog_config,
            reply: tx,
        })
        .await;
    let handle = rx
        .await
        .map_err(|e| TaskError::ActorError(e.to_string()))??;

    let (ctx_tx, ctx_rx) = tokio::sync::oneshot::channel();
    let _ = handle.actor_ref.tell(TaskMsg::GetContext(ctx_tx)).await;
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
            },
            None,
        )
        .await
        .unwrap();

        let (tx, rx) = tokio::sync::oneshot::channel();
        let _ = manager
            .tell(TaskManagerMsg::WaitForTask {
                task_id: handle.id,
                timeout_duration: None,
                reply: tx,
            })
            .await;
        let result = rx.await.unwrap();
        assert!(result.is_ok());
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
                    ctx.reset_watchdog().await;
                }
            },
            None,
        )
        .await
        .unwrap();

        let (tx, rx) = tokio::sync::oneshot::channel();
        let _ = manager
            .tell(TaskManagerMsg::WaitForTask {
                task_id: handle.id,
                timeout_duration: Some(Duration::from_secs(5)),
                reply: tx,
            })
            .await;
        let result = rx.await.unwrap();
        assert!(result.is_ok());
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
        let _ = manager
            .tell(TaskManagerMsg::CancelTask {
                task_id: handle.id,
                timeout_duration: Some(Duration::from_secs(1)),
                reply: tx,
            })
            .await;
        let result = rx.await.unwrap();
        assert!(result.is_ok());
    }
}
