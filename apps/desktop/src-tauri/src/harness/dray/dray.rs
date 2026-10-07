//! Lathe Coding Agent process integration.
//!
//! Lathe keeps one native child alive, discovers global and
//! project skills, and sends every event as one JSON line. The app keeps its
//! own normalized log while Lathe keeps the model-context session file.

use crate::events::{AgentEvent, AgentEventPayload};
use crate::harness::Harness::Dray;
use crate::models::{Effort, Model};
use crate::session::{
    flush_queued, publish_status, write_line, QueuedMessages, Session, StatusTracker,
};
use crate::store::{self, append_session_event, next_seq_by_session_id};
use anyhow::{Context, Result};
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering::Relaxed};
use std::sync::Arc;
use tauri::{AppHandle, Emitter};
use tokio::{
    io::{AsyncBufReadExt, BufReader},
    process::{ChildStderr, ChildStdin, ChildStdout},
    sync::Mutex,
};

pub mod commands;
pub mod mapper;
pub mod parser;
pub use parser::AgentRpcEvent;

const CONTEXT_STATS_REQUEST_ID: &str = "dray-context-stats";

/// Remove only the native history file belonging to this session.
pub async fn delete_session_data(session_id: &str) -> Result<()> {
    uuid::Uuid::parse_str(session_id).context("invalid session ID")?;
    let path = store::get_home_app_dir()
        .await?
        .join("agent-sessions")
        .join(format!("{session_id}.json"));
    match tokio::fs::remove_file(path).await {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.into()),
    }
}
/// Starts one persistent Lathe RPC child for a Lathe session.
///
/// All sessions run locally and keep agent history in a stable Lathe-owned directory.
pub async fn init(
    session_id: &str,
    model: &Model,
    effort: Option<Effort>,
    cwd: &str,
    session_cwd: &str,
    is_new_session: bool,
    fork_from: Option<&str>,
    app: &AppHandle,
) -> Result<Session> {
    let app_dir = store::get_home_app_dir().await?;
    let session_dir = app_dir.join("agent-sessions");
    tokio::fs::create_dir_all(&session_dir).await?;
    let session_dir = session_dir
        .to_str()
        .context("Invalid session directory")?
        .to_string();
    let mut args = vec![
        "--mode".to_string(),
        "rpc".to_string(),
        "--session-dir".to_string(),
        session_dir,
    ];
    if let Some(agent_model) = &model.agent_model {
        args.push("--provider".to_string());
        args.push(agent_model.provider.clone());
        args.push("--model".to_string());
        args.push(agent_model.id.clone());
    }
    if let Some(effort) = effort {
        args.push("--thinking".to_string());
        args.push(effort.as_arg().to_string());
    }
    if let Some(context) = model.context_window {
        args.extend(["--context-window".into(), context.to_string()]);
    }
    if let Some(parent) = fork_from {
        // Lathe performs the lazy fork while opening RPC mode and keeps the new
        // transcript under the id Lathe already assigned to this session.
        args.extend([
            "--fork".to_string(),
            parent.to_string(),
            "--session-id".to_string(),
            session_id.to_string(),
        ]);
    } else {
        // The same form creates a missing session and resumes an existing one.
        args.extend(["--session-id".to_string(), session_id.to_string()]);
    }

    let mut command = crate::binpath::agent_command().await;
    command.args(&args).current_dir(cwd);
    let child = command
        .stdin(Stdio::piped())
        .env("PATH", crate::binpath::agent_path());
    let mut child = child
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .context("couldn't start dray")?;

    #[cfg(windows)]
    let process_job = crate::session::ProcessJob::attach(&child);

    let stdin = Arc::new(Mutex::new(
        child.stdin.take().context("failed to take stdin")?,
    ));
    // Seed older sessions from Lathe's transcript without touching their logs.
    // The runtime ignores this command when its own context already exists.
    let history = store::list_session_events(session_id).await?;
    let input = history
        .iter()
        .filter_map(|event| match &event.payload {
            AgentEventPayload::UserMessage { text, .. } => {
                Some(serde_json::json!({"role":"user","content":text}))
            }
            AgentEventPayload::AssistantText { text, .. } => {
                Some(serde_json::json!({"role":"assistant","content":text}))
            }
            _ => None,
        })
        .collect::<Vec<_>>();
    if !input.is_empty() {
        write_line(&stdin, &serde_json::json!({"type":"history","input":input})).await?;
    }
    let stdout = child.stdout.take().context("failed to take stdout")?;
    let stderr = child.stderr.take().context("failed to take stderr")?;

    let events: Arc<Mutex<Vec<AgentEvent>>> = Arc::new(Mutex::new(Vec::new()));
    let stdout_events = events.clone();
    let status: Arc<Mutex<StatusTracker>> = Arc::new(Mutex::new(StatusTracker::default()));
    let stdout_status = status.clone();
    let stopped = Arc::new(AtomicBool::new(false));
    let refresh_stopped = stopped.clone();
    let refresh_stdin = stdin.clone();
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(std::time::Duration::from_secs(300));
        loop {
            interval.tick().await;
            if refresh_stopped.load(Relaxed) {
                break;
            }
            if let Ok(token) = crate::account::access_token().await {
                if write_line(
                    &refresh_stdin,
                    &serde_json::json!({"type":"auth","accessToken":token}),
                )
                .await
                .is_err()
                {
                    break;
                }
            }
        }
    });
    let stdout_stopped = stopped.clone();
    let seq_start = if is_new_session {
        0
    } else {
        next_seq_by_session_id(session_id).await?
    };
    let seq = Arc::new(AtomicU64::new(seq_start));
    let stdout_seq = seq.clone();
    let flush_seq = seq.clone();
    let queued: QueuedMessages = Arc::new(Mutex::new(Vec::new()));
    let stdout_queued = queued.clone();
    let flush_events = events.clone();
    let flush_stdin = stdin.clone();
    let pending_ui = mapper::PendingUiRequests::default();
    let stdout_pending_ui = pending_ui.clone();
    let session_id_owned = session_id.to_string();
    let session_cwd_owned = session_cwd.to_string();
    let app_for_stdout = app.clone();

    tokio::spawn(async move {
        if let Err(error) = read_stdout(
            stdout,
            &session_id_owned,
            &session_cwd_owned,
            stdout_events,
            stdout_seq,
            stdout_status,
            stdout_stopped,
            stdout_queued,
            flush_seq,
            flush_events,
            flush_stdin,
            stdout_pending_ui,
            &app_for_stdout,
        )
        .await
        {
            eprintln!("Failed to read Lathe stdout: {error}");
        }
    });

    tokio::spawn(async move {
        if let Err(error) = read_stderr(stderr).await {
            eprintln!("Failed to read Lathe stderr: {error}");
        }
    });

    Ok(Session {
        id: session_id.to_string(),
        child,
        stdin,
        harness: Dray,
        cwd: session_cwd.to_string(),
        model: model.id,
        agent_model: model.agent_model.clone(),
        effort,
        events,
        seq,
        status,
        stopped,
        #[cfg(windows)]
        process_job,
        agent_ui_requests: pending_ui,
        queued,
    })
}

/// Reads and persists Lathe's normalized events one RPC line at a time.
async fn read_stdout(
    stdout: ChildStdout,
    session_id: &str,
    session_cwd: &str,
    events: Arc<Mutex<Vec<AgentEvent>>>,
    stdout_seq: Arc<AtomicU64>,
    status: Arc<Mutex<StatusTracker>>,
    stopped: Arc<AtomicBool>,
    queued: QueuedMessages,
    flush_seq: Arc<AtomicU64>,
    flush_events: Arc<Mutex<Vec<AgentEvent>>>,
    flush_stdin: Arc<Mutex<ChildStdin>>,
    pending_ui: mapper::PendingUiRequests,
    app: &AppHandle,
) -> Result<()> {
    let mut lines = BufReader::new(stdout).lines();
    let mut mapper =
        mapper::Mapper::with_seq_and_ui(session_id, session_cwd, stdout_seq, pending_ui);

    while let Some(line) = lines.next_line().await? {
        if stopped.load(Relaxed) {
            break;
        }
        if line.trim().is_empty() {
            continue;
        }

        let runtime_event = match parser::parse_line(&line) {
            Ok(event) => event,
            Err(error) => {
                record_failure(session_id, "parse", &error.to_string(), &line).await;
                continue;
            }
        };

        if matches!(runtime_event, AgentRpcEvent::Unrecognized) {
            record_failure(
                session_id,
                "unknown_subtype",
                "unmodeled Lathe event",
                &line,
            )
            .await;
        }

        let is_session = matches!(&runtime_event, AgentRpcEvent::Session { .. });
        // A Lathe turn is one model response plus its tool calls. Requesting stats
        // here updates the context meter between model turns instead of making
        // it wait for the whole agent operation to settle.
        let is_turn_end = matches!(&runtime_event, AgentRpcEvent::TurnEnd { .. });
        let mapped = match mapper.map(runtime_event) {
            Ok(events) => events,
            Err(error) => {
                record_failure(session_id, "map", &error.to_string(), &line).await;
                continue;
            }
        };

        // Lathe can answer a stats request sent immediately after spawn with the
        // empty pre-session value. Wait until its session event has been read so
        // a resumed session's initial reading cannot race with the first turn
        // and overwrite the real context usage with zero.
        if is_session {
            if let Err(error) = request_context_stats(&flush_stdin).await {
                eprintln!("[dray context stats request err] {error}");
            }
        }

        for mut agent_event in mapped {
            if stopped.load(Relaxed) {
                return Ok(());
            }

            if let AgentEventPayload::TurnCompleted { ref mut head, .. } = agent_event.payload {
                *head = crate::git::snapshot_tree(session_cwd).await;
                if stopped.load(Relaxed) {
                    return Ok(());
                }
            }

            if let AgentEventPayload::ToolCallCompleted { ref mut result, .. } = agent_event.payload
            {
                crate::attachments::archive_result_images(session_id, &mut result.images).await;
            }

            let at_boundary = matches!(
                agent_event.payload,
                AgentEventPayload::ToolCallStarted { .. }
                    | AgentEventPayload::ToolCallCompleted { .. }
                    | AgentEventPayload::TurnCompleted { .. }
            );

            if stopped.load(Relaxed) {
                return Ok(());
            }
            if let Err(error) = app.emit("agent_event", &agent_event) {
                eprintln!("[dray emit err] {error}");
            }

            let next_status = {
                let mut tracker = status.lock().await;
                tracker.note_tool_call(&agent_event.payload);
                tracker.on_event(&agent_event.payload)
            };
            if let Some(next) = next_status {
                if stopped.load(Relaxed) {
                    return Ok(());
                }
                publish_status(session_id, next, app).await;
            }

            if stopped.load(Relaxed) {
                return Ok(());
            }

            // Deltas are streaming previews. Their
            // committed counterparts are the assistant message and settled
            // event, so they do not belong in Lathe's append-only transcript.
            // Request usage and context stats are persisted; they are the
            // source for usage indicators and the composer context meter.
            let transient = match &agent_event.payload {
                AgentEventPayload::Delta(_)
                | AgentEventPayload::ModelRequestStarted
                | AgentEventPayload::QuestionsAsked { .. }
                | AgentEventPayload::ExtensionNotification { .. } => true,
                AgentEventPayload::UsageUpdate(_) => false,
                _ => false,
            };
            if transient {
                continue;
            }

            let turn_completed = matches!(
                &agent_event.payload,
                AgentEventPayload::TurnCompleted { .. }
            );
            let turn_baseline = match &agent_event.payload {
                AgentEventPayload::TurnCompleted { head, .. } => head.clone(),
                _ => None,
            };

            events.lock().await.push(agent_event.clone());
            if let Err(error) = append_session_event(session_id, agent_event).await {
                eprintln!("[dray write err] {error}");
            }

            if stopped.load(Relaxed) {
                return Ok(());
            }

            if at_boundary {
                flush_queued(
                    session_id,
                    Dray,
                    &queued,
                    &flush_seq,
                    &flush_events,
                    &flush_stdin,
                    &status,
                    turn_completed,
                    turn_baseline,
                    app,
                )
                .await;
            }

            if turn_completed {
                if let Err(error) = request_context_stats(&flush_stdin).await {
                    eprintln!("[dray context stats request err] {error}");
                }
            }
        }

        if is_turn_end {
            if let Err(error) = request_context_stats(&flush_stdin).await {
                eprintln!("[dray context stats request err] {error}");
            }
        }
    }

    if !stopped.swap(true, Relaxed) && status.lock().await.turn_in_flight() {
        let mut terminal = mapper.map(AgentRpcEvent::MessageEnd {
            message: serde_json::json!({"role":"assistant","content":[],"stopReason":"error","errorMessage":"Lathe's agent process exited unexpectedly. Send a follow-up to resume."}),
        })?;
        terminal.extend(mapper.map(AgentRpcEvent::AgentSettled)?);
        for event in terminal {
            app.emit("agent_event", &event).ok();
            let next = status.lock().await.on_event(&event.payload);
            events.lock().await.push(event.clone());
            append_session_event(session_id, event).await?;
            if let Some(next) = next {
                publish_status(session_id, next, app).await;
            }
        }
    }
    Ok(())
}

/// Requests Lathe's current context estimate. The response is handled by the
/// same stdout mapper as every other RPC record.
async fn request_context_stats(stdin: &Arc<Mutex<ChildStdin>>) -> Result<()> {
    write_line(
        stdin,
        &serde_json::json!({
            "id": CONTEXT_STATS_REQUEST_ID,
            "type": "get_session_stats",
        }),
    )
    .await
}

/// Logs a malformed or unsupported Lathe record without stopping the read loop.
async fn record_failure(session_id: &str, stage: &str, detail: &str, raw: &str) {
    eprintln!("[dray {stage} err] {detail}\n[{stage} err] raw line: {raw}");
    if let Err(error) = store::record_parse_failure(session_id, stage, detail, raw).await {
        eprintln!("[dray failure log err] {error}");
    }
}

/// Copies Lathe's stderr to the app process for diagnostics.
async fn read_stderr(stderr: ChildStderr) -> Result<()> {
    let mut lines = BufReader::new(stderr).lines();
    while let Some(line) = lines.next_line().await? {
        eprintln!("Lathe stderr: {line}");
    }
    Ok(())
}
