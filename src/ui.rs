use std::{
    f64,
    io::{StdoutLock, Write},
    path::PathBuf,
    time::Duration,
};

use libffmpeg::{ffmpeg::FfmpegError, util::cmd::CommandExit};
use serde::Serialize;
use std::time::Instant;
use tokio::task::JoinHandle;
use tokio_util::{future::FutureExt, sync::CancellationToken};
use tracing::{Instrument, Span};
use valuable::Valuable;

use crate::OutputFormat;

#[derive(Debug, Clone)]
pub enum UiMessagePayload {
    Created {
        input: PathBuf,
        output: PathBuf,
        total: Duration,
    },
    Started,
    Finished {
        exit: CommandExit,
    },
    Failed {
        error: FfmpegError,
    },
    Progress {
        total: Duration,
        current: Duration,
    },
}

pub struct UiMessage {
    // auto increment, assigned before `Created`
    pub task_id: usize,
    pub payload: UiMessagePayload,
}

impl UiMessage {
    pub fn new(task_id: usize, payload: UiMessagePayload) -> Self {
        Self { task_id, payload }
    }
}

struct UiTask {
    id: usize,
    input: PathBuf,
    output: PathBuf,
    active: bool,
    started_at: Option<Instant>,
    exited_at: Option<Instant>,
    success: Option<bool>,
    error_description: Option<String>,
    total: Duration,
    current: Duration,
}

impl UiTask {
    pub fn new(id: usize, input: PathBuf, output: PathBuf, total: Duration) -> Self {
        Self {
            id,
            input,
            output,
            active: false,
            started_at: None,
            exited_at: None,
            success: None,
            error_description: None,
            total,
            current: Duration::ZERO,
        }
    }
}

#[derive(Serialize, Valuable)]
struct Row {
    total_tasks: usize,
    active_tasks: usize,
    completed_tasks: usize,
    successful_tasks: usize,
    failed_tasks: usize,
    tasks: Vec<TaskInfo>,
}

#[derive(Serialize, Valuable)]
struct TaskInfo {
    id: usize,
    input: PathBuf,
    output: PathBuf,
    active: bool,
    started_at: Option<String>,
    exited_at: Option<String>,
    success: Option<bool>,
    error_description: Option<String>,
    total: String,
    current: String,
    percent: String,
}

impl Row {
    fn to_string_human(&self) -> String {
        format!(
            "Total: {} | Active: {} | Completed: {} | Success: {} | Failed: {} | [{}]",
            self.total_tasks,
            self.active_tasks,
            self.completed_tasks,
            self.successful_tasks,
            self.failed_tasks,
            self.tasks
                .iter()
                .map(|t| {
                    let status = if t.active {
                        "A"
                    } else if t.success == Some(true) {
                        "S"
                    } else if t.success == Some(false) {
                        "E"
                    } else {
                        "Q"
                    };
                    format!("{}:{} {}", status, t.id, t.percent)
                })
                .collect::<Vec<_>>()
                .join(" : ")
        )
    }

    fn to_string_json(&self) -> String {
        serde_json::to_string(&self)
            .unwrap_or(r#"{"$meta":{"error":"Failed to serialize"}}"#.into())
    }
    fn to_string_json_pretty(&self) -> String {
        serde_json::to_string_pretty(&self)
            .unwrap_or(r#"{"$meta":{"error":"Failed to serialize"}}"#.into())
    }
}

struct UiState {
    // tasks[task_id] => state for that task
    tasks: Vec<Option<UiTask>>,
}

impl UiState {
    pub fn new() -> Self {
        Self {
            tasks: Vec::with_capacity(256),
        }
    }

    fn get(&mut self, task_id: usize) -> &mut Option<UiTask> {
        while self.tasks.len() < task_id + 1 {
            self.tasks.push(None);
        }

        // Safe because of the above padding
        self.tasks.get_mut(task_id).unwrap()
    }

    pub fn update(&mut self, delivery: UiMessage) -> anyhow::Result<()> {
        let task = self.get(delivery.task_id);
        if task.is_none() {
            match &delivery.payload {
                // This is the only time `get` should return None
                UiMessagePayload::Created {
                    input,
                    output,
                    total,
                } => {
                    // Initialize the task
                    let _ = task.insert(UiTask::new(
                        delivery.task_id,
                        input.clone(),
                        output.clone(),
                        *total,
                    ));
                }
                _ => {
                    anyhow::bail!(
                        "Received {:?} for non-existent task id={}",
                        delivery.payload,
                        delivery.task_id
                    );
                }
            }
        } else {
            let task = task.as_mut().unwrap();
            match delivery.payload {
                UiMessagePayload::Created { .. } => { /* nop, should be unreachable */ }
                UiMessagePayload::Started => {
                    task.active = true;
                    task.started_at = Some(Instant::now());
                }
                UiMessagePayload::Finished { exit } => {
                    task.active = false;
                    task.exited_at = Some(Instant::now());
                    task.success = Some(exit.exit_code.is_some_and(|ec| ec.success));
                }
                UiMessagePayload::Failed { error } => {
                    task.active = false;
                    task.exited_at = Some(Instant::now());
                    task.success = Some(false);
                    task.error_description = Some(error.to_string());
                }
                UiMessagePayload::Progress { total, current } => {
                    task.current = current;
                    task.total = total;
                }
            }
        }

        Ok(())
    }

    pub fn draw(&self, stdout: &mut StdoutLock, format: OutputFormat) -> anyhow::Result<()> {
        let tasks: Vec<TaskInfo> = self
            .tasks
            .iter()
            .flatten()
            .map(|t| TaskInfo {
                id: t.id,
                input: t.input.clone(),
                output: t.output.clone(),
                active: t.active,
                started_at: t
                    .started_at
                    .map(|i| format!("T-{:.0}", Instant::now().duration_since(i).as_secs_f64())),
                exited_at: t
                    .exited_at
                    .map(|i| format!("T-{:.0}", Instant::now().duration_since(i).as_secs_f64())),
                success: t.success,
                error_description: t.error_description.clone(),
                total: format!("{:.1}s", t.total.as_secs_f64()),
                current: format!("{:.1}s", t.current.as_secs_f64()),
                percent: format!(
                    "{:.1}%",
                    t.current.as_secs_f64() / t.total.as_secs_f64().max(f64::EPSILON) * 100.0
                ),
            })
            .collect();

        let row: Row = Row {
            total_tasks: tasks.len(),
            active_tasks: tasks.iter().filter(|t| t.active).count(),
            completed_tasks: tasks.iter().filter(|t| t.exited_at.is_some()).count(),
            successful_tasks: tasks.iter().filter(|t| t.success == Some(true)).count(),
            failed_tasks: tasks.iter().filter(|t| t.success == Some(false)).count(),
            tasks,
        };

        let output = match format {
            OutputFormat::Human => row.to_string_human(),
            OutputFormat::Json => row.to_string_json(),
            OutputFormat::JsonPretty => row.to_string_json_pretty(),
            OutputFormat::Verbose => {
                tracing::info!(row = row.as_value());
                return Ok(());
            }
        };
        writeln!(stdout, "{output}")?;
        Ok(())
    }
}

pub async fn ui_main(
    mut rx: tokio::sync::mpsc::Receiver<UiMessage>,
    cancellation_token: CancellationToken,
    format: OutputFormat,
) -> anyhow::Result<()> {
    use std::io::stdout;

    let mut state = UiState::new();

    while !cancellation_token.is_cancelled() {
        let delivery_fut = tokio::time::timeout(
            Duration::from_secs_f64(1.0 / 12.0),
            rx.recv().with_cancellation_token(&cancellation_token),
        );
        let delivery = match delivery_fut.await {
            Ok(Some(Some(delivery))) => Some(delivery),
            Ok(Some(None)) /* Closed */ => break,
            Ok(None) /* Cancelled */ => break,
            Err(_timeout) => None
        };

        if let Some(delivery) = delivery {
            state.update(delivery)?;
        }

        let mut stdout = stdout().lock();
        state.draw(&mut stdout, format)?;
    }

    Ok(())
}

pub fn ui_spawn(
    rx: tokio::sync::mpsc::Receiver<UiMessage>,
    cancellation_token: CancellationToken,
    format: OutputFormat,
    span: Span,
) -> (CancellationToken, JoinHandle<anyhow::Result<()>>) {
    let ct = cancellation_token.child_token();
    let handle = tokio::spawn(ui_main(rx, ct.clone(), format).instrument(span));
    (ct, handle)
}
