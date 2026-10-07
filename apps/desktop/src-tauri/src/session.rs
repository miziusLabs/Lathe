use crate::{
    attachments,
    events::{now_rfc3339, AgentEvent, AgentEventPayload, ImageRef, MessageSender},
    git,
    harness::{dray, Harness::Dray},
    models::{resolve_effort, AgentModel, Effort, Model, ModelId},
    store::{
        append_session_event, append_session_index_item, clear_fork_from, copy_session_log,
        delete_session, get_session_index_item, list_session_events,
        resolve_unclaimed_worktree_name, set_session_status, touch_session_index_item,
        SessionIndexItem, SessionSnapshot, SessionStatus,
    },
};
use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::json;
use ts_rs::TS;
use uuid::Uuid;

// `Harness` is defined in `crate::harness`; re-exported so existing
// `crate::session::Harness` imports keep working.
pub use crate::harness::Harness;
#[cfg(windows)]
use std::process::Stdio;
use std::{
    collections::HashMap,
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering::Relaxed},
        Arc,
    },
};
use tauri::{AppHandle, Emitter};
#[cfg(windows)]
use tokio::process::Command;
use tokio::{
    io::AsyncWriteExt,
    process::{Child, ChildStdin},
    sync::Mutex,
};
#[cfg(windows)]
use windows_sys::Win32::{
    Foundation::{CloseHandle, HANDLE},
    System::JobObjects::{
        AssignProcessToJobObject, CreateJobObjectW, JobObjectExtendedLimitInformation,
        SetInformationJobObject, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
        JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
    },
};

/// Emitted once a new session's index entry is durable, so the sidebar gains
/// the row without waiting for process startup or a refetch.
///
/// Carries the index item alone, not a `SessionSnapshot`. The frontend's
/// `agent_event` listener writes into sessions it already holds and drops
/// events for ids it doesn't — so shipping a snapshot would open a window where
/// the transcript is half-built in memory and half only on disk. An index item
/// leaves the transcript unread until the row is clicked, and
/// `handleSelectSessionIndexItem` already loads it whole from disk at that
/// point.
pub const SESSION_CREATED: &str = "session_created";

/// Resolves the home-relative form used by the No Project setting. PathBuf
/// handles the native separator on both macOS and Windows; accepting both slash
/// forms keeps a manually entered `~/...` path portable as well.
fn resolve_local_cwd(cwd: &str, home: &std::path::Path) -> PathBuf {
    if cwd == "~" {
        home.to_path_buf()
    } else if let Some(rest) = cwd.strip_prefix("~/").or_else(|| cwd.strip_prefix("~\\")) {
        home.join(rest)
    } else {
        PathBuf::from(cwd)
    }
}

/// Ensures the local launch directory exists before spawning Lathe.
async fn prepare_local_cwd(cwd: &str) -> Result<String> {
    let home = dirs::home_dir().context("could not resolve home directory")?;
    let path = resolve_local_cwd(cwd, &home);
    tokio::fs::create_dir_all(&path)
        .await
        .with_context(|| format!("could not create session directory: {}", path.display()))?;

    Ok(path.to_string_lossy().into_owned())
}

/// Emitted as `session_status` when a session's status changes, so the sidebar
/// and composer update without a refetch. Like `SessionTitleEvent`, this is not
/// an `AgentEvent`: it's derived state, and must never reach the `.jsonl` log.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export, export_to = "events.ts")]
#[serde(rename_all = "camelCase")]
pub struct SessionStatusEvent {
    pub session_id: String,
    pub status: SessionStatus,
    /// The entry's `modified` as the status write left it — completion bumps it,
    /// and the sidebar orders by it, so a session finishing has to move to the
    /// top without a refetch. `None` only when the id is no longer indexed.
    pub modified: Option<String>,
}

/// A prompt typed while a turn was running, held here until the turn reaches a
/// point where handing it to the CLI costs nothing.
///
/// It is *not* persisted while it waits, and that is what makes cancelling it
/// clean: the log is append-only, so a queued message written on arrival could
/// only be retracted with a tombstone event. Held here instead, a cancel leaves
/// no trace at all. Nothing is lost by waiting — the flush persists it, and the
/// only window where it exists solely in memory is one the user is still
/// allowed to take it back from.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export, export_to = "events.ts")]
#[serde(rename_all = "camelCase")]
pub struct QueuedMessage {
    pub id: String,
    pub session_id: String,
    /// The raw prompt. Attachments are resolved at flush rather than now, so
    /// what the composer gets back on a cancel is what the user typed.
    pub text: String,
    pub attachment_paths: Vec<String>,
    /// Held with the prompt rather than looked up at flush: a relayed message
    /// can wait out a long turn, and the sending session may be renamed or
    /// deleted before the boundary that delivers it.
    #[serde(default)]
    pub from: Option<MessageSender>,
}

/// A prompt waiting for either an active-turn steering boundary or the end of
/// the current turn. This mode stays internal; the frontend only needs the
/// cancelable prompt itself.
#[derive(Debug, Clone)]
pub struct PendingPrompt {
    message: QueuedMessage,
    after_turn: bool,
}

/// Held prompts, oldest first. Shared with the stdout task, which sees when
/// active-turn prompts can be steered and when a turn has completed.
pub type QueuedMessages = Arc<Mutex<Vec<PendingPrompt>>>;

/// What a send did. The two fields are mutually exclusive in practice — a
/// session being created cannot already be running a turn — but they answer
/// different questions and the frontend acts on each separately.
#[derive(Debug, Default, Serialize, Deserialize, TS)]
#[ts(export, export_to = "events.ts")]
#[serde(rename_all = "camelCase")]
pub struct SendOutcome {
    /// `Some` only when this send created the session.
    pub snapshot: Option<SessionSnapshot>,
    /// `Some` when a turn was already running, so the prompt is held rather
    /// than sent. The frontend draws it as pending and can still take it back.
    pub queued: Option<QueuedMessage>,
}

/// Drives [`SessionStatus`] from the mapped event stream plus the user's own
/// sends.
///
/// A completed turn ends the active work once no model call is open.
#[derive(Debug, Default)]
pub struct StatusTracker {
    status: SessionStatus,
    /// An `init` opened a model call that no `result` has closed yet.
    model_call_open: bool,
    /// Tool calls started and not yet finished. Not a status input — it decides
    /// whether an arriving prompt is written now or held.
    open_tool_calls: usize,
}

impl StatusTracker {
    /// The user sent a prompt; work is starting regardless of what stdout says.
    pub fn on_send(&mut self) -> Option<SessionStatus> {
        self.model_call_open = true;
        self.set(SessionStatus::InProgress)
    }

    /// Advances on one mapped event. `Some` when the status changed — the
    /// caller persists and emits only then.
    pub fn on_event(&mut self, payload: &AgentEventPayload) -> Option<SessionStatus> {
        match payload {
            // Fires per model call, not per prompt.
            AgentEventPayload::TurnStarted(_) => {
                self.model_call_open = true;
                self.set(SessionStatus::InProgress)
            }
            AgentEventPayload::TurnCompleted { .. } => {
                self.model_call_open = false;
                self.set(SessionStatus::Completed)
            }
            _ => None,
        }
    }

    /// Whether the session is still working.
    pub fn is_busy(&self) -> bool {
        self.status == SessionStatus::InProgress
    }

    /// Whether a model call is open right now, which is what decides that an
    /// arriving prompt is queued rather than sent. Read *before* `on_send`,
    /// which opens one unconditionally and would answer for itself.
    pub fn turn_in_flight(&self) -> bool {
        self.model_call_open
    }

    /// Counts tool calls in and out. Fed separately from [`Self::on_event`]
    /// because only the caller holds the event envelope.
    pub fn note_tool_call(&mut self, payload: &AgentEventPayload) {
        match payload {
            AgentEventPayload::ToolCallStarted { .. } => self.open_tool_calls += 1,
            AgentEventPayload::ToolCallCompleted { .. } => {
                self.open_tool_calls = self.open_tool_calls.saturating_sub(1)
            }
            // A turn cannot end with a call still running, and a killed child
            // can end one without completing its calls — so the count is reset
            // here rather than left to drift up over a session.
            AgentEventPayload::TurnCompleted { .. } => self.open_tool_calls = 0,
            _ => {}
        }
    }

    /// Whether a main-thread tool call is running right now.
    ///
    /// This is what decides that a prompt goes out immediately instead of being
    /// held: the CLI injects a buffered prompt at the next tool *result*, and
    /// while a tool runs that result is still ahead — so writing now catches it,
    /// where waiting for the boundary this app can see would miss it by the few
    /// milliseconds between the result line and the model call that follows it.
    pub fn tool_in_flight(&self) -> bool {
        self.open_tool_calls > 0
    }

    /// The user read the finished session. Only `Completed` clears — selecting
    /// a running session must not stop it reading as busy.
    pub fn mark_seen(&mut self) -> Option<SessionStatus> {
        (self.status == SessionStatus::Completed)
            .then(|| self.set(SessionStatus::Idle))
            .flatten()
    }

    fn set(&mut self, next: SessionStatus) -> Option<SessionStatus> {
        (self.status != next).then(|| {
            self.status = next;
            next
        })
    }
}

/// Preserve dirty checkouts and checkouts still referenced by another session.
async fn remove_session_worktree(item: &SessionIndexItem) {
    if item.worktree_name.is_none() {
        return;
    }
    let Ok(items) = crate::store::list_session_index_items().await else {
        return;
    };
    if items
        .iter()
        .any(|other| other.session_id != item.session_id && other.cwd == item.cwd)
    {
        return;
    }
    if let Err(error) = crate::worktrees::remove(&item.cwd).await {
        eprintln!("Preserving worktree {}: {error}", item.cwd);
    }
}

/// Persists a status change and tells the frontend. Failures are logged, not
/// propagated: status is derived state, and losing one update must not take
/// down the stdout loop that noticed it.
pub async fn publish_status(session_id: &str, status: SessionStatus, app: &AppHandle) {
    // Read back off the write rather than recomputed here: which statuses bump
    // `modified` is `set_session_status`'s rule, and stating it twice is how the
    // sidebar and the disk drift apart.
    let modified = match set_session_status(session_id, status).await {
        Ok(item) => item.map(|i| i.modified),
        Err(e) => {
            eprintln!("[status write err] {e}");
            None
        }
    };

    let event = SessionStatusEvent {
        session_id: session_id.to_string(),
        status,
        modified,
    };
    if let Err(e) = app.emit("session_status", &event) {
        eprintln!("[status emit err] {e}");
    }
}

#[derive(Debug)]
pub struct SessionManager {
    pub sessions: Mutex<HashMap<String, Session>>,
}

impl Default for SessionManager {
    fn default() -> Self {
        Self {
            sessions: Mutex::new(HashMap::new()),
        }
    }
}

impl SessionManager {
    /// Stop sessions when disconnecting the account and update persisted/UI state.
    pub async fn disconnect(&self, app: &AppHandle) {
        let mut guard = self.sessions.lock().await;
        for (id, session) in std::mem::take(&mut *guard) {
            if let Err(error) = session.kill().await {
                eprintln!("[session disconnect err] {id}: {error}");
            }
            publish_status(&id, SessionStatus::Idle, app).await;
        }
    }

    /// Stops every live session before the app exits. The map is drained before
    /// awaiting any child so no new send can race shutdown, and a failed kill
    /// is logged without preventing the remaining children from being cleaned.
    pub async fn kill_all(&self) {
        let mut guard = self.sessions.lock().await;
        let sessions = std::mem::take(&mut *guard);

        for (session_id, session) in sessions {
            if let Err(error) = session.kill().await {
                eprintln!("[session shutdown err] {session_id}: {error}");
            }
        }
        drop(guard);
    }

    /// Routes a prompt to a session: spawns a new child, reuses a live one, or
    /// respawns via `--resume` when the id is known but its process is gone.
    pub async fn send_msg(
        &self,
        session_id: &str,
        prompt: &str,
        // Absolute paths of what the composer had attached. Re-read here rather
        // than uploaded: the frontend holds a thumbnail, not bytes.
        attachment_paths: &[String],
        harness: Harness,
        _model: ModelId,
        agent_model: Option<AgentModel>,
        effort: Option<Effort>,
        title_model: Option<AgentModel>,
        title_effort: Option<Effort>,
        cwd: &str,
        // The attached project used for sidebar/Git metadata. `None` means the
        // built-in No Project choice and is kept out of project grouping.
        project_path: Option<&str>,
        // Source branch for a new worktree; the project checkout stays untouched.
        branch: Option<&str>,
        use_worktree: bool,
        worktree_name: Option<&str>,
        // Optional source ref supplied by non-composer callers.
        base_ref: Option<&str>,
        is_new_session: bool,
        queue_after_turn: bool,
        // Recorded rather than acted on — the depth cap reads it back off the
        // index on the *next* create.
        parent_session_id: Option<&str>,
        // The session that relayed this prompt, drawn on the `user_message`
        // when present. `None` from the composer: its prompts are the user's
        // own, and a message with a sender is drawn differently.
        from: Option<MessageSender>,
        app: &AppHandle,
    ) -> Result<SendOutcome> {
        // An existing session's harness is durable state, just like its cwd;
        // the frontend's control state can lag while a session is being opened.
        let harness = if is_new_session {
            harness
        } else {
            get_session_index_item(session_id)
                .await?
                .map(|item| item.harness)
                .unwrap_or(harness)
        };
        debug_assert_eq!(harness, Dray);
        let (model, agent_model) = (ModelId::Dray, agent_model);
        crate::account::access_token().await?;
        let catalog = crate::harness::dray::commands::list_models(None).await?;
        let model_spec = catalog
            .into_iter()
            .find(|spec| spec.agent_model == agent_model)
            .context("Choose a supported OpenAI model in the model selector.")?;
        let effort = resolve_effort(&model_spec, effort);

        if is_new_session {
            let local_cwd = if use_worktree || project_path.is_some() {
                None
            } else {
                Some(prepare_local_cwd(cwd).await?)
            };
            let worktree_name = if use_worktree {
                Some(resolve_unclaimed_worktree_name(cwd, worktree_name).await?)
            } else {
                None
            };

            let (session_cwd, recorded_branch) = match &worktree_name {
                Some(name) => {
                    let project =
                        project_path.context("Choose a Git project for the Worktree session.")?;
                    let (path, branch) =
                        crate::worktrees::create(project, name, branch.or(base_ref)).await?;
                    (path, Some(branch))
                }
                None => {
                    let path = local_cwd.clone().unwrap_or_else(|| cwd.to_string());
                    let branch = git::current_branch(&path).await;
                    (path, branch)
                }
            };
            let recorded_project_path = project_path.unwrap_or("");

            let mut item = SessionIndexItem::new(
                session_id,
                harness,
                &session_cwd,
                recorded_project_path,
                worktree_name.as_deref(),
                recorded_branch.as_deref(),
                prompt,
                model,
                effort,
                parent_session_id,
            );
            item.agent_model = agent_model.clone();

            // Index before the process starts, so startup failures remain
            // visible and the user can retry.
            if let Err(error) = append_session_index_item(item.clone()).await {
                if worktree_name.is_some() {
                    let _ = crate::worktrees::remove(&session_cwd).await;
                }
                return Err(error);
            }
            app.emit(SESSION_CREATED, &item).ok();

            let baseline = git::snapshot_tree(&session_cwd).await;

            let launch_cwd = session_cwd.as_str();
            let mut session = Session::init(
                session_id,
                harness,
                &model_spec,
                effort,
                launch_cwd,
                &session_cwd,
                is_new_session,
                None,
                app,
            )
            .await?;
            session
                .send_msg(prompt, attachment_paths, baseline, from, app)
                .await?;
            let events = list_session_events(session_id).await?;
            self.sessions
                .lock()
                .await
                .insert(session_id.to_string(), session);

            crate::title::spawn_title_generation(
                session_id,
                prompt,
                &session_cwd,
                title_model.as_ref(),
                title_effort,
                app,
            );

            return Ok(SendOutcome {
                snapshot: Some(SessionSnapshot {
                    index_item: item,
                    events,
                }),
                queued: None,
            });
        }

        let mut sessions_guard = self.sessions.lock().await;

        // The caller's `cwd` is a hint for a new session only. From here on the
        // recorded one wins: with a project picker the two can disagree, and
        // resuming in the wrong directory is both silent and destructive. It is
        // also where the baseline gets snapshotted, so a stale value would
        // diff the wrong tree.
        let indexed = get_session_index_item(session_id).await?;
        if indexed
            .as_ref()
            .is_some_and(|item| item.cloud_name.is_some())
        {
            bail!("This legacy Cloud session uses Docker. Start a new Worktree session to continue locally.");
        }
        let session_harness = indexed.as_ref().map(|item| item.harness).unwrap_or(harness);
        let session_cwd = match &indexed {
            Some(item) => item.cwd.clone(),
            None => cwd.to_string(),
        };

        if let Some(s) = sessions_guard.get_mut(session_id) {
            // Before the send, so the index reflects intent even if writing to
            // the child fails — the prompt event is persisted ahead of stdin too.
            touch_session_index_item(session_id, model, model_spec.agent_model.as_ref(), effort)
                .await?;

            // Re-read after the index write: a turn may finish while that disk
            // operation is in flight. Hold the status lock through queueing a
            // Ctrl+Enter prompt so the completion flush cannot pass it first.
            let (turn_in_flight, tool_in_flight) = {
                let tracker = s.status.lock().await;
                let turn_in_flight = tracker.turn_in_flight();
                if turn_in_flight && queue_after_turn {
                    let queued = s.queue_msg(prompt, attachment_paths, from, true).await;
                    return Ok(SendOutcome {
                        snapshot: None,
                        queued: Some(queued),
                    });
                }
                (turn_in_flight, tracker.tool_in_flight())
            };

            // A model call is open, so this prompt is held rather than sent.
            //
            // Gated on the turn, not on `busy`: a session holding a background
            // task reads busy with its main thread idle, and queueing there left
            // the prompt waiting on a boundary that task would never produce.
            if turn_in_flight {
                // A tool is running, so the CLI's next injection point — that
                // tool's result — is still ahead, and writing now is what lands
                // the prompt on it. Holding for the `tool_call_completed` this
                // app can see would miss it: the CLI dispatches the next model
                // call within a few milliseconds of emitting the result line, so
                // the prompt would sit in its buffer through another whole tool
                // call before being read. Measured, and the reason this branch
                // exists rather than one uniform hold.
                //
                // The cost is that there is no window to cancel in — which the
                // UI states by itself, since a prompt written straight through
                // draws no pending row and so offers no Esc.
                if tool_in_flight && !queue_after_turn {
                    s.queue_and_flush(prompt, attachment_paths, from, app).await;
                    return Ok(SendOutcome::default());
                }

                let queued = s.queue_msg(prompt, attachment_paths, from, false).await;
                return Ok(SendOutcome {
                    snapshot: None,
                    queued: Some(queued),
                });
            }

            if s.effort != effort {
                write_line(
                    &s.stdin,
                    &json!({"type":"set_thinking_level","level":effort.map(|e|e.as_arg())}),
                )
                .await?;
                s.effort = effort;
            }
            if s.model != model || s.agent_model != model_spec.agent_model {
                s.set_model(&model_spec).await?;
            }

            // Last thing before the prompt goes down the pipe: the child is idle
            // but alive, so the narrower the gap the less of the user's own
            // editing lands on the turn's side of the diff.
            let baseline = git::snapshot_tree(&session_cwd).await;
            s.send_msg(prompt, attachment_paths, baseline, from, app)
                .await?;
            return Ok(SendOutcome::default());
        }

        touch_session_index_item(session_id, model, model_spec.agent_model.as_ref(), effort)
            .await?;

        let fork_from = indexed.as_ref().and_then(|i| i.fork_from.clone());
        let baseline = git::snapshot_tree(&session_cwd).await;
        let launch_cwd = session_cwd.as_str();
        let mut session = Session::init(
            session_id,
            session_harness,
            &model_spec,
            effort,
            launch_cwd,
            &session_cwd,
            is_new_session,
            fork_from.as_deref(),
            app,
        )
        .await?;

        // After the spawn, so a child that fails to start leaves the instruction
        // standing and the next send forks again. Cleared before the prompt goes
        // out for the opposite reason: from here the CLI owns a session under
        // this id, and forking the parent a second time would abandon it.
        if fork_from.is_some() {
            clear_fork_from(session_id).await?;
        }

        session
            .send_msg(prompt, attachment_paths, baseline, from, app)
            .await?;
        sessions_guard.insert(session_id.to_string(), session);
        Ok(SendOutcome::default())
    }

    /// Copies a session onto a new id, to be continued separately from the one
    /// it came from. `worktree` puts the fork in a tree of its own rather than
    /// leaving it in the parent's directory.
    ///
    /// Worktree forks create their checkout here; no agent process spawns. The CLI's fork only happens on a spawn, and spawning
    /// one to sit idle would cost a child process per fork and a turn's wait
    /// before the row appeared — so this writes the app's half now and leaves
    /// [`fork_from`](crate::store::SessionIndexItem::fork_from) as the
    /// instruction for the first send. The copied log is what the fork replays
    /// meanwhile, so it opens reading exactly like its parent.
    ///
    /// Refused while the parent is working. The CLI forks by reading the
    /// parent's transcript, which a live child is still appending to, so a fork
    /// taken mid-turn can inherit half of one.
    pub async fn fork(
        &self,
        session_id: &str,
        fork_id: &str,
        worktree: bool,
    ) -> Result<SessionSnapshot> {
        let parent = get_session_index_item(session_id)
            .await?
            .with_context(|| format!("unknown session {session_id}"))?;

        if let Some(s) = self.sessions.lock().await.get(session_id) {
            if s.status.lock().await.is_busy() {
                bail!("wait for the session to finish before forking it");
            }
        }

        // In-place forks share the checkout; separate forks get a newly pulled branch.
        if parent.cloud_name.is_some() {
            bail!("Legacy Cloud sessions cannot be forked into a local checkout.");
        }
        let worktree_name = if worktree {
            Some(resolve_unclaimed_worktree_name(&parent.project_path, None).await?)
        } else {
            None
        };

        let events = copy_session_log(session_id, fork_id).await?;

        // The parent never got a conversation off the ground — indexed, then its
        // spawn failed — so the CLI has no transcript under that id to fork
        // from. Refused here, where it can be said plainly; left to the first
        // send it would come back as the CLI's own "no conversation found".
        // Checked after the copy because that read is what answers it, and it
        // writes nothing when there is nothing to write.
        if events.is_empty() {
            bail!("this session has no conversation to fork yet");
        }

        let mut item = parent.fork(fork_id, worktree_name.as_deref());
        if let Some(name) = worktree_name.as_deref() {
            let base = git::current_branch(&parent.cwd)
                .await
                .or(parent.branch.clone());
            let (cwd, branch) =
                crate::worktrees::create(&parent.project_path, name, base.as_deref()).await?;
            item.cwd = cwd;
            item.branch = Some(branch);
        }
        if let Err(error) = append_session_index_item(item.clone()).await {
            if worktree {
                let _ = crate::worktrees::remove(&item.cwd).await;
            }
            return Err(error);
        }

        Ok(SessionSnapshot {
            index_item: item,
            events,
        })
    }

    /// Stops everything the session is doing immediately.
    ///
    /// Remove the child from the live map and terminate its process tree. The
    /// next prompt resumes the persisted Lathe session in a new child.
    pub async fn interrupt(&self, session_id: &str, app: &AppHandle) -> Result<()> {
        // Keep the manager lock until the idle status is published. Otherwise a
        // prompt sent in the small window after removal could respawn the
        // session and then receive this stop's late idle event.
        let mut sessions_guard = self.sessions.lock().await;
        let session = sessions_guard
            .remove(session_id)
            .with_context(|| format!("no running session {session_id}"))?;

        // `kill` marks the stdout reader before terminating the child, so data
        // already buffered in the pipe cannot publish a late in-progress or
        // completed status after this stop has been acknowledged.
        let stopped_at = now_rfc3339();
        let events = session.events.clone();
        let seq = session.seq.clone();
        let cwd = session.cwd.clone();
        session.kill().await?;

        // Stop bypasses the runtime's settled event. Close the turn ourselves
        // so its elapsed time survives transcript reloads. Use the stop time,
        // not the time spent terminating the process or taking the snapshot.
        let event = {
            let events = events.lock().await;
            interrupted_turn(&events, &stopped_at, &seq)
        };
        if let Some(mut event) = event {
            if let AgentEventPayload::TurnCompleted { head, .. } = &mut event.payload {
                *head = git::snapshot_tree(&cwd).await;
            }
            if let Err(error) = append_session_event(session_id, event.clone()).await {
                eprintln!("[stop write err] {error}");
            }
            if let Err(error) = app.emit("agent_event", &event) {
                eprintln!("[stop emit err] {error}");
            }
        }
        publish_status(session_id, SessionStatus::Idle, app).await;
        drop(sessions_guard);
        Ok(())
    }

    /// Takes back the newest prompt still waiting on a boundary, returning it
    /// so the composer can put the text back where the user left it.
    ///
    /// A session with no live child answers `None` rather than erroring: the
    /// queue died with the process, which is the same "nothing to take back"
    /// the frontend already handles.
    pub async fn cancel_queued(&self, session_id: &str) -> Option<QueuedMessage> {
        let sessions_guard = self.sessions.lock().await;
        let session = sessions_guard.get(session_id)?;
        session.cancel_queued().await
    }

    /// Answers an `AskUserQuestion`. Only the process that asked can be told.
    pub async fn answer_questions(
        &self,
        session_id: &str,
        request_id: &str,
        answers: HashMap<String, String>,
        app: &AppHandle,
    ) -> Result<()> {
        let mut sessions_guard = self.sessions.lock().await;
        let Some(session) = sessions_guard.get_mut(session_id) else {
            bail!("no running session {session_id}");
        };
        session.answer_questions(request_id, answers, app).await
    }

    /// Deletes a session: kills its child if one is running, then drops the
    /// index entry and the log. Returns whether the index held it.
    ///
    /// The child goes first and its lock is released before the disk work, so a
    /// dying process can't append one last event to a file we just removed.
    pub async fn delete(&self, session_id: &str) -> Result<bool> {
        let running = self.sessions.lock().await.remove(session_id);
        if let Some(session) = running {
            session.kill().await?;
        }

        // All local sessions keep their context files beside Lathe.
        if let Err(e) = dray::delete_session_data(session_id).await {
            eprintln!("could not delete Lathe session data for {session_id}: {e}");
        }

        if let Some(item) = get_session_index_item(session_id).await? {
            remove_session_worktree(&item).await;
        }

        // Best-effort: the images are a convenience for the transcript that is
        // about to stop existing, so failing to remove them must not fail the
        // delete the user asked for.
        if let Err(e) = attachments::delete_session_attachments(session_id).await {
            eprintln!("could not delete attachments for {session_id}: {e}");
        }

        delete_session(session_id).await
    }

    /// Clears a finished session's unread mark: `Completed` → `Idle`, anything
    /// else untouched. Returns the status as written, `None` for no change.
    ///
    /// The live tracker is updated first so the in-memory machine agrees with
    /// the index; a session with no live process falls back to the index alone.
    pub async fn mark_idle(&self, session_id: &str) -> Result<Option<SessionStatus>> {
        let sessions_guard = self.sessions.lock().await;

        if let Some(session) = sessions_guard.get(session_id) {
            let Some(next) = session.status.lock().await.mark_seen() else {
                return Ok(None);
            };
            set_session_status(session_id, next).await?;
            return Ok(Some(next));
        }
        drop(sessions_guard);

        match get_session_index_item(session_id).await? {
            Some(item) if item.status == SessionStatus::Completed => {
                set_session_status(session_id, SessionStatus::Idle).await?;
                Ok(Some(SessionStatus::Idle))
            }
            _ => Ok(None),
        }
    }
}

/// Closes only an unfinished prompted turn. Queued prompts belong to the
/// existing turn and must not reset its start time.
fn interrupted_turn(
    events: &[AgentEvent],
    stopped_at: &str,
    seq: &AtomicU64,
) -> Option<AgentEvent> {
    let start = events
        .iter()
        .rposition(|event| matches!(event.payload, AgentEventPayload::TurnCompleted { .. }))
        .map_or(0, |index| index + 1);
    let pending = &events[start..];
    let boundary = pending
        .iter()
        .rev()
        .find(|event| {
            matches!(
                event.payload,
                AgentEventPayload::UserMessage { queued: false, .. }
            )
        })
        // A queued prompt flushed after completion starts a new turn.
        .or_else(|| {
            pending
                .iter()
                .find(|event| matches!(event.payload, AgentEventPayload::UserMessage { .. }))
        })?;

    Some(AgentEvent {
        id: Uuid::now_v7().to_string(),
        session_id: boundary.session_id.clone(),
        harness: boundary.harness,
        seq: seq.fetch_add(1, Relaxed),
        // The UI derives elapsed time from the prompt and completion timestamps.
        ts: stopped_at.to_string(),
        turn_id: None,
        payload: AgentEventPayload::TurnCompleted {
            status: crate::events::TurnStatus::Error,
            stop_reason: Some("aborted".to_string()),
            final_text: None,
            usage: None,
            duration_ms: None,
            head: None,
        },
        raw: None,
    })
}

/// Owns a Windows job containing the Lathe process and all of its descendants.
/// Closing a job configured with `KILL_ON_JOB_CLOSE` terminates the whole tree
/// without waiting for `taskkill` to enumerate and reap every process.
#[cfg(windows)]
#[derive(Debug)]
pub struct ProcessJob {
    // Raw Windows handles are pointers and are not marked `Send` by Rust,
    // although this kernel handle is safe to move between threads. Keep its
    // numeric representation so SessionManager remains transferable.
    handle: usize,
}

#[cfg(windows)]
impl ProcessJob {
    /// Creates and configures a job after Lathe is spawned. A failure falls back
    /// to the taskkill path, since being unable to install the optimization
    /// must not prevent a session from starting.
    pub fn attach(child: &Child) -> Option<Self> {
        let process = child.raw_handle()?;
        let handle = unsafe { CreateJobObjectW(std::ptr::null(), std::ptr::null()) };
        if handle.is_null() {
            eprintln!("[process job err] could not create Windows job");
            return None;
        }

        let mut limits = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
        limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
        let configured = unsafe {
            SetInformationJobObject(
                handle,
                JobObjectExtendedLimitInformation,
                &limits as *const _ as *const std::ffi::c_void,
                std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
            ) != 0
        };
        let assigned =
            configured && unsafe { AssignProcessToJobObject(handle, process as HANDLE) != 0 };

        if !assigned {
            unsafe { CloseHandle(handle) };
            eprintln!("[process job err] could not assign Lathe to Windows job");
            return None;
        }

        Some(Self {
            handle: handle as usize,
        })
    }
}

#[cfg(windows)]
impl Drop for ProcessJob {
    fn drop(&mut self) {
        unsafe { CloseHandle(self.handle as HANDLE) };
    }
}

#[derive(Debug)]
pub struct Session {
    pub id: String,
    pub child: Child,
    /// Shared with the stdout task, which writes extension UI responses on
    /// behalf of the frontend.
    pub stdin: Arc<Mutex<ChildStdin>>,
    pub harness: Harness,
    /// The checkout used by the agent and local UI.
    pub cwd: String,
    pub model: ModelId,
    pub agent_model: Option<AgentModel>,
    pub effort: Option<Effort>,
    pub events: Arc<Mutex<Vec<AgentEvent>>>,
    pub seq: Arc<AtomicU64>,
    /// Shared with the stdout task: sends and mapped lifecycle events update it.
    pub status: Arc<Mutex<StatusTracker>>,
    /// Set before a stop kills the child, so buffered stdout cannot publish
    /// stale events after the session has been stopped and removed from the map.
    pub stopped: Arc<AtomicBool>,
    /// Native Windows process-tree ownership. Unix keeps its existing child
    /// termination path and does not need an extra handle.
    #[cfg(windows)]
    pub process_job: Option<ProcessJob>,
    /// Lathe extension dialogs waiting for an answer from the frontend.
    pub agent_ui_requests: dray::mapper::PendingUiRequests,
    /// Prompts typed during a running turn, waiting for the next boundary.
    /// Shared with the stdout task, which is what flushes them.
    pub queued: QueuedMessages,
}

impl Session {
    /// Spawns the child process for the selected harness.
    pub async fn init(
        session_id: &str,
        harness: Harness,
        model: &Model,
        effort: Option<Effort>,
        cwd: &str,
        // The checkout used for Git snapshots.
        session_cwd: &str,
        is_new_session: bool,
        fork_from: Option<&str>,
        app: &AppHandle,
    ) -> Result<Session> {
        debug_assert_eq!(harness, Dray);
        dray::init(
            session_id,
            model,
            effort,
            cwd,
            session_cwd,
            is_new_session,
            fork_from,
            app,
        )
        .await
    }

    /// Builds and saves the user's own prompt event, then writes it to the
    /// child's stdin — the CLI never echoes it back.
    ///
    /// `baseline` is the caller's working-tree snapshot, taken before this
    /// prompt reaches the child.
    pub async fn send_msg(
        &mut self,
        prompt: &str,
        attachment_paths: &[String],
        baseline: Option<String>,
        from: Option<MessageSender>,
        app: &AppHandle,
    ) -> Result<()> {
        deliver_prompt(
            &self.id,
            self.harness,
            prompt,
            attachment_paths,
            baseline,
            false,
            None,
            from,
            &self.seq,
            &self.events,
            &self.stdin,
            app,
        )
        .await?;

        if let Some(next) = self.status.lock().await.on_send() {
            publish_status(&self.id, next, app).await;
        }

        Ok(())
    }

    /// Holds a prompt typed during a running turn. Nothing is written or
    /// persisted here — [`flush_queued`] does both once the turn reaches a
    /// boundary, which is what leaves a cancel possible until then.
    pub async fn queue_msg(
        &self,
        prompt: &str,
        attachment_paths: &[String],
        from: Option<MessageSender>,
        after_turn: bool,
    ) -> QueuedMessage {
        let message = QueuedMessage {
            id: Uuid::now_v7().to_string(),
            session_id: self.id.clone(),
            text: prompt.to_string(),
            attachment_paths: attachment_paths.to_vec(),
            from,
        };
        self.queued.lock().await.push(PendingPrompt {
            message: message.clone(),
            after_turn,
        });
        message
    }

    /// Takes back the newest held prompt, newest-first because that is the one
    /// the user just typed and the only one the composer is offering to undo.
    ///
    /// `None` means the flush won the race, which needs no handling beyond
    /// leaving the composer alone: the prompt is on its way and the frontend
    /// learns so from the `user_message` that follows.
    pub async fn cancel_queued(&self) -> Option<QueuedMessage> {
        self.queued
            .lock()
            .await
            .pop()
            .map(|pending| pending.message)
    }

    /// Holds a prompt and immediately hands it over, for the case where a tool
    /// call is already running.
    ///
    /// Through the queue rather than written directly, so a prompt already
    /// waiting goes out ahead of this one instead of being overtaken.
    pub async fn queue_and_flush(
        &self,
        prompt: &str,
        attachment_paths: &[String],
        from: Option<MessageSender>,
        app: &AppHandle,
    ) {
        self.queue_msg(prompt, attachment_paths, from, false).await;
        flush_queued(
            &self.id,
            self.harness,
            &self.queued,
            &self.seq,
            &self.events,
            &self.stdin,
            &self.status,
            false,
            None,
            app,
        )
        .await;
    }

    /// Switches the model of a running child. Verified against the CLI: the
    /// reply after this arrives from the new model, so no respawn is needed.
    /// There is no `set_effort` counterpart — the CLI rejects that subtype, and
    /// an `effort` field on this request is accepted but ignored.
    pub async fn set_model(&mut self, model: &Model) -> Result<()> {
        if self.harness == Dray {
            let agent_model = model
                .agent_model
                .as_ref()
                .context("Lathe model is missing its provider")?;
            write_line(
                &self.stdin,
                &serde_json::json!({
                    "type": "set_model",
                    "provider": agent_model.provider,
                    "modelId": agent_model.id,
                    "contextWindow":model.context_window,
                }),
            )
            .await?;
            write_line(
                &self.stdin,
                &json!({"type":"set_thinking_level","level":self.effort.map(|e|e.as_arg())}),
            )
            .await?;
            self.model = model.id;
            self.agent_model = Some(agent_model.clone());
            return Ok(());
        }

        Ok(())
    }

    /// Sends the user's answers back and retires the card.
    ///
    /// Single-shot and reply-first: the request is removed before the response
    /// is written, so a second answer cannot be sent for the same dialog.
    /// An empty map is a skip, not an error: the harness turns it into "the user
    /// did not answer", which is the truthful thing to tell the agent.
    pub async fn answer_questions(
        &mut self,
        request_id: &str,
        answers: HashMap<String, String>,
        app: &AppHandle,
    ) -> Result<()> {
        let pending = self
            .agent_ui_requests
            .lock()
            .expect("Lathe UI request mutex poisoned")
            .remove(request_id)
            .with_context(|| format!("no pending Lathe UI request {request_id}"))?;
        write_line(&self.stdin, &pending.response(&answers)).await?;

        let answered = AgentEvent {
            id: Uuid::now_v7().to_string(),
            session_id: self.id.clone(),
            harness: self.harness,
            seq: self.seq.fetch_add(1, Relaxed),
            ts: now_rfc3339(),
            turn_id: None,
            payload: AgentEventPayload::QuestionAnswered {
                request_id: request_id.to_string(),
                tool_use_id: format!("dray-ui-{request_id}"),
            },
            raw: None,
        };
        app.emit("agent_event", &answered)?;

        Ok(())
    }

    /// Kills the child process. Takes `self` by value — a killed session can't
    /// be reused. The stdout reader is marked first so buffered events cannot
    /// revive a session after it has been stopped.
    pub async fn kill(mut self) -> Result<()> {
        self.stopped.store(true, Relaxed);

        #[cfg(windows)]
        if let Some(process_job) = self.process_job.take() {
            // Closing the job is the fast, native tree termination path. Wait
            // only for the direct child to be reaped; no process enumeration or
            // synchronous taskkill command remains on the Stop critical path.
            drop(process_job);
            self.child.wait().await?;
            return Ok(());
        }

        terminate_child(&mut self.child).await
    }
}

/// Terminates a session's Lathe process and, on Windows, every descendant tool.
///
/// New sessions use [`ProcessJob`] above. `taskkill /T` remains as a fallback
/// for a process that could not be assigned to a job (for example, when the
/// host has already placed it in an incompatible job).
async fn terminate_child(child: &mut Child) -> Result<()> {
    if matches!(child.try_wait(), Ok(Some(_))) {
        return Ok(());
    }

    #[cfg(windows)]
    {
        let Some(pid) = child.id() else {
            return Ok(());
        };

        let mut command = Command::new("taskkill");
        crate::binpath::configure_command(&mut command);
        let killed = command
            .args(["/PID", &pid.to_string(), "/T", "/F"])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .await
            .is_ok_and(|status| status.success());

        if killed {
            child.wait().await?;
            return Ok(());
        }

        // The process may have exited between the first check and taskkill.
        // Reaping that race is preferable to reporting a failed Stop.
        if matches!(child.try_wait(), Ok(Some(_))) {
            return Ok(());
        }
    }

    child.kill().await?;
    Ok(())
}

/// Writes one JSON line to a child's stdin. The CLI's input format is
/// line-delimited, so the newline and the flush are part of the message rather
/// than tidiness.
///
/// Takes anything serializable rather than a built [`Value`](serde_json::Value),
/// so a typed line goes out without being rendered into one first.
pub async fn write_line(stdin: &Arc<Mutex<ChildStdin>>, value: &impl Serialize) -> Result<()> {
    let mut line = serde_json::to_string(value)?;
    line.push('\n');

    let mut guard = stdin.lock().await;
    guard.write_all(line.as_bytes()).await?;
    guard.flush().await?;
    Ok(())
}

/// Persists the user's own prompt event, emits it, then writes it to the
/// child's stdin — the CLI never echoes a prompt back, so this is the only
/// place it enters the transcript.
///
/// Free rather than a method because a queued prompt is delivered from the
/// stdout task, which holds the same handles but no `Session`.
#[allow(clippy::too_many_arguments)]
async fn deliver_prompt(
    session_id: &str,
    harness: Harness,
    prompt: &str,
    attachment_paths: &[String],
    baseline: Option<String>,
    queued: bool,
    event_id: Option<&str>,
    from: Option<MessageSender>,
    seq: &Arc<AtomicU64>,
    events: &Arc<Mutex<Vec<AgentEvent>>>,
    stdin: &Arc<Mutex<ChildStdin>>,
    app: &AppHandle,
) -> Result<()> {
    let seq = seq.fetch_add(1, Relaxed);

    // Ahead of the event, because it is what decides the event's own text:
    // a non-image attachment becomes an `@path` mention on the prompt, and
    // the transcript has to show what the model was actually given.
    let prepared = attachments::prepare(session_id, prompt, attachment_paths).await?;

    let payload = AgentEventPayload::UserMessage {
        text: prepared.text.clone(),
        images: prepared
            .images
            .iter()
            .map(|i| ImageRef {
                path: Some(i.stored_path.clone()),
                url: None,
                mime_type: Some(i.mime_type.clone()),
            })
            .collect(),
        baseline,
        queued,
        from,
    };
    let agent_event = AgentEvent {
        id: event_id
            .map(str::to_string)
            .unwrap_or_else(|| Uuid::now_v7().to_string()),
        session_id: session_id.to_string(),
        harness,
        seq,
        ts: now_rfc3339(),
        // Nothing tracks turns yet; Lathe opens one per `init`.
        turn_id: None,
        payload,
        raw: None,
    };

    app.emit("agent_event", &agent_event)?;

    let mut events_guard = events.lock().await;
    events_guard.push(agent_event.clone());
    drop(events_guard);

    append_session_event(session_id, agent_event).await?;

    debug_assert_eq!(harness, Dray);
    // `$name` is Lathe's user-facing skill syntax; Lathe's RPC parser expects the
    // equivalent `/skill:name` command. Keep the stored event in `$` form so
    // the transcript reflects what the user typed.
    let agent_prompt = normalize_skill_prompt(&prepared.text);
    let mut line = json!({"type": "prompt", "message": agent_prompt});
    if !prepared.images.is_empty() {
        line["images"] = json!(prepared
            .images
            .iter()
            .map(|image| json!({
                "type": "image",
                "data": image.data,
                "mimeType": image.mime_type,
            }))
            .collect::<Vec<_>>());
    }
    if queued {
        // Steering prompts use the next tool boundary. A queued follow-up waits
        // for the current turn and starts a new one without this mode.
        line["streamingBehavior"] = json!("steer");
    }
    let token = crate::account::access_token().await?;
    write_line(stdin, &json!({"type":"auth", "accessToken":token})).await?;
    write_line(stdin, &line).await
}

fn normalize_skill_prompt(prompt: &str) -> String {
    let Some(rest) = prompt.strip_prefix('$') else {
        return prompt.to_string();
    };
    let split = rest.find(char::is_whitespace).unwrap_or(rest.len());
    if split == 0 {
        return prompt.to_string();
    }
    format!("/skill:{}{}", &rest[..split], &rest[split..])
}

/// Takes prompts that are ready at this boundary, preserving follow-ups until
/// the active turn completes. Enter-submitted prompts are steered into the live
/// turn; Ctrl+Enter prompts start one new turn at a time after it.
fn take_ready_prompts(queue: &mut Vec<PendingPrompt>, turn_completed: bool) -> Vec<PendingPrompt> {
    let mut steering = Vec::new();
    let mut after_turn = Vec::new();

    for pending in queue.drain(..) {
        if pending.after_turn {
            after_turn.push(pending);
        } else {
            steering.push(pending);
        }
    }

    if !steering.is_empty() {
        *queue = after_turn;
        return steering;
    }

    if turn_completed && !after_turn.is_empty() {
        let first = after_turn.remove(0);
        *queue = after_turn;
        return vec![first];
    }

    *queue = after_turn;
    Vec::new()
}

/// Hands ready prompts to the child at a safe boundary. A turn completion can
/// release only one queued follow-up, so each accepted follow-up becomes its
/// own turn and later ones remain cancelable until that turn also completes.
///
/// Failures are logged, not propagated — the stdout loop must survive anything,
/// and a prompt that cannot be written is one the user can retype.
pub async fn flush_queued(
    session_id: &str,
    harness: Harness,
    queued: &QueuedMessages,
    seq: &Arc<AtomicU64>,
    events: &Arc<Mutex<Vec<AgentEvent>>>,
    stdin: &Arc<Mutex<ChildStdin>>,
    status: &Arc<Mutex<StatusTracker>>,
    turn_completed: bool,
    turn_baseline: Option<String>,
    app: &AppHandle,
) {
    // Select and remove under one lock so cancellation either takes a prompt
    // back before this flush or finds that it has already reached the child.
    let batch = {
        let mut pending = queued.lock().await;
        take_ready_prompts(&mut pending, turn_completed)
    };

    if batch.is_empty() {
        return;
    }

    for pending in batch {
        let after_turn = pending.after_turn;
        let message = pending.message;
        if let Err(err) = deliver_prompt(
            session_id,
            harness,
            &message.text,
            &message.attachment_paths,
            if after_turn {
                turn_baseline.clone()
            } else {
                None
            },
            !after_turn,
            Some(&message.id),
            message.from,
            seq,
            events,
            stdin,
            app,
        )
        .await
        {
            eprintln!("[queued flush err] {err}");
        }
    }

    // A flush at `turn_completed` lands just after the tracker marked the
    // session finished, and the prompt it just wrote opens a new turn the CLI
    // has not announced yet. Without this the composer reads idle for the
    // second or so until `init` arrives — offering to send into a session that
    // is already working. Redundant at a tool boundary, where the session is
    // in-progress and `on_send` reports no change.
    if let Some(next) = status.lock().await.on_send() {
        publish_status(session_id, next, app).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn queue_policy_steers_at_boundaries_and_releases_follow_ups_one_at_a_time() {
        let pending_prompt = |text: &str, after_turn: bool| PendingPrompt {
            message: QueuedMessage {
                id: text.to_string(),
                session_id: "test-session".to_string(),
                text: text.to_string(),
                attachment_paths: Vec::new(),
                from: None,
            },
            after_turn,
        };
        let mut pending = vec![
            pending_prompt("follow-up 1", true),
            pending_prompt("steer", false),
            pending_prompt("follow-up 2", true),
        ];

        let steering = take_ready_prompts(&mut pending, false);
        assert_eq!(
            steering
                .iter()
                .map(|prompt| prompt.message.text.as_str())
                .collect::<Vec<_>>(),
            vec!["steer"]
        );
        assert_eq!(pending.len(), 2);
        assert!(take_ready_prompts(&mut pending, false).is_empty());

        let first_follow_up = take_ready_prompts(&mut pending, true);
        assert_eq!(first_follow_up[0].message.text, "follow-up 1");
        assert_eq!(pending.len(), 1);

        let second_follow_up = take_ready_prompts(&mut pending, true);
        assert_eq!(second_follow_up[0].message.text, "follow-up 2");
        assert!(pending.is_empty());
    }

    #[test]
    fn normalizes_skill_prompts_for_pi() {
        assert_eq!(
            normalize_skill_prompt("$commit-and-push"),
            "/skill:commit-and-push"
        );
        assert_eq!(
            normalize_skill_prompt("$commit-and-push now"),
            "/skill:commit-and-push now"
        );
        assert_eq!(normalize_skill_prompt("fix the bug"), "fix the bug");
    }

    #[test]
    fn resolves_home_relative_paths_with_both_separator_styles() {
        let home = std::path::Path::new("/home/tester");

        assert_eq!(
            resolve_local_cwd("~/Coding/Sandbox", home),
            home.join("Coding/Sandbox")
        );
        assert_eq!(
            resolve_local_cwd(r"~\Coding\Sandbox", home),
            home.join(r"Coding\Sandbox")
        );
        assert_eq!(
            resolve_local_cwd("/tmp/sandbox", home),
            std::path::PathBuf::from("/tmp/sandbox")
        );
    }

    fn turn_completed() -> AgentEventPayload {
        AgentEventPayload::TurnCompleted {
            status: crate::events::TurnStatus::Success,
            stop_reason: None,
            final_text: None,
            usage: None,
            duration_ms: None,
            head: None,
        }
    }

    fn prompt_event(ts: &str, queued: bool) -> AgentEvent {
        AgentEvent {
            id: Uuid::now_v7().to_string(),
            session_id: "test-session".to_string(),
            harness: Dray,
            seq: 0,
            ts: ts.to_string(),
            turn_id: None,
            payload: AgentEventPayload::UserMessage {
                text: "work".to_string(),
                images: Vec::new(),
                baseline: None,
                queued,
                from: None,
            },
            raw: None,
        }
    }

    #[test]
    fn interrupted_turn_preserves_elapsed_time_from_original_prompt() {
        let events = vec![
            prompt_event("2026-01-01T12:00:00Z", false),
            prompt_event("2026-01-01T12:01:00Z", true),
        ];
        let seq = AtomicU64::new(2);
        let event = interrupted_turn(&events, "2026-01-01T12:01:26Z", &seq).unwrap();
        assert_eq!(event.ts, "2026-01-01T12:01:26Z");
        assert_eq!(event.seq, 2);
        assert!(matches!(
            event.payload,
            AgentEventPayload::TurnCompleted {
                status: crate::events::TurnStatus::Error,
                stop_reason: Some(ref reason),
                ..
            } if reason == "aborted"
        ));

        // Replay retains the end timestamp used by the UI's elapsed timer.
        let saved = serde_json::to_string(&event).unwrap();
        let replayed: AgentEvent = serde_json::from_str(&saved).unwrap();
        assert_eq!(replayed.ts, event.ts);
    }

    #[test]
    fn interrupted_turn_does_not_close_an_already_completed_turn() {
        let seq = AtomicU64::new(2);
        assert!(interrupted_turn(&[], "2026-01-01T12:01:26Z", &seq).is_none());
        let prompt = prompt_event("2026-01-01T12:00:00Z", false);
        let mut completed = prompt.clone();
        completed.payload = turn_completed();
        let mut events = vec![prompt, completed];
        assert!(interrupted_turn(&events, "2026-01-01T12:01:26Z", &seq).is_none());
        assert_eq!(seq.load(Relaxed), 2);

        events.push(prompt_event("2026-01-01T12:01:20Z", false));
        let event = interrupted_turn(&events, "2026-01-01T12:01:26Z", &seq).unwrap();
        assert_eq!(event.ts, "2026-01-01T12:01:26Z");
    }

    /// Only a finished-and-unread session clears on read; selecting a running
    /// one must not stop it reading as busy.
    #[test]
    fn mark_seen_clears_only_completed() {
        let mut tracker = StatusTracker::default();
        assert_eq!(tracker.mark_seen(), None, "idle has nothing to clear");

        tracker.on_send();
        assert_eq!(tracker.mark_seen(), None, "a running session stays busy");

        tracker.on_event(&turn_completed());
        assert_eq!(tracker.mark_seen(), Some(SessionStatus::Idle));
        assert_eq!(tracker.mark_seen(), None, "already read");
    }
}
