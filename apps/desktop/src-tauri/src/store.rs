use std::{
    path::{Path, PathBuf},
    vec,
};

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::{fs, io::AsyncWriteExt, sync::Mutex};
use ts_rs::TS;
use uuid::Uuid;

use crate::{
    events::{now_rfc3339, AgentEvent},
    models::{AgentModel, Effort, ModelId},
    session::Harness,
};

/// Driven by [`StatusTracker`](crate::session::StatusTracker). `Completed`
/// means finished *and unread* — the transition back to `Idle` is the user
/// looking at the session, not anything the agent does.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export, export_to = "events.ts")]
#[serde(rename_all = "snake_case")]
pub enum SessionStatus {
    Idle,
    InProgress,
    Completed,
}

impl Default for SessionStatus {
    fn default() -> Self {
        Self::Idle
    }
}

#[derive(Debug, Serialize, Deserialize, Clone, TS)]
#[ts(export, export_to = "events.ts")]
#[serde(rename_all = "camelCase")]
pub struct SessionIndexItem {
    pub session_id: String,
    pub harness: Harness,
    /// Where the agent actually runs. Equals `project_path` for a normal
    /// session; points inside `<project>/.lathe/worktrees/<id>` for a Worktree one.
    pub cwd: String,
    /// Project metadata used for sidebar grouping; empty for No Project.
    /// Worktree sessions share this repository through a separate checkout.
    pub project_path: String,
    pub branch: Option<String>,
    /// Identifies a managed local Git worktree.
    #[serde(default)]
    pub worktree_name: Option<String>,
    /// Legacy Docker workspace. Kept for existing session data.
    #[serde(default)]
    pub cloud_name: Option<String>,
    pub title: String,
    /// Remembered per session so switching between sessions restores the model
    /// the user last picked instead of resetting to a default.
    #[serde(default)]
    pub model: ModelId,
    /// The concrete provider/model selected when the harness is Lathe.
    #[serde(default)]
    #[serde(alias = "piModel")]
    pub agent_model: Option<AgentModel>,
    /// `None` for models that take no effort flag.
    #[serde(default)]
    pub effort: Option<Effort>,
    /// Defaulted so index entries written before this field parse as `Idle`.
    #[serde(default)]
    pub status: SessionStatus,
    /// An instruction, not a record of lineage: the session this one was forked
    /// from, set until the CLI has actually forked. The fork is lazy — copying
    /// this app's own log and index entry is instant, while the CLI's half only
    /// happens on a spawn — so the first send resumes the parent with
    /// `--fork-session` and clears this. Every send after is an ordinary resume.
    ///
    /// Distinct from `parent_session_id` below, which is lineage and permanent:
    /// this one is cleared the moment the CLI carries the fork out.
    #[serde(default)]
    pub fork_from: Option<String>,
    /// The session whose agent created this one, rather than one started by a
    /// person in the composer. `Some` is also what the depth guard reads: a
    /// session that was itself spawned may not spawn more.
    #[serde(default)]
    pub parent_session_id: Option<String>,
    pub created: String,
    pub modified: String,
    pub archived: bool,
    pub pinned: bool,
}

/// What crosses the IPC boundary for one session: its index entry plus the
/// replayed event log. Distinct from [`crate::session::Session`], which owns a
/// child process and cannot be serialized.
#[derive(Debug, Serialize, Deserialize, Clone, TS)]
#[ts(export, export_to = "events.ts")]
#[serde(rename_all = "camelCase")]
pub struct SessionSnapshot {
    #[serde(flatten)]
    #[ts(flatten)]
    pub index_item: SessionIndexItem,
    pub events: Vec<AgentEvent>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionIndexByProject {
    pub path: String,
    pub indexes: Vec<SessionIndexItem>,
}

static INDEX_LOCK: Mutex<()> = Mutex::const_new(());

fn configured_home_app_dir() -> Option<PathBuf> {
    std::env::var_os("DRAY_DATA_DIR")
        .filter(|path| !path.is_empty())
        .map(PathBuf::from)
}

/// App data root. Development can override this to keep its sessions, projects,
/// and attachments separate from the installed application.
pub fn app_home_dir() -> Result<PathBuf> {
    if let Some(path) = configured_home_app_dir() {
        return Ok(path);
    }

    dirs::home_dir()
        .map(|home| home.join(".dray"))
        .context("could not resolve home directory")
}

/// `~/.dray`, creating it if this is the first run. If `~/.automedon` exists
/// from before the app's rename and `~/.dray` doesn't yet, the old directory is
/// moved into place. An explicit data directory skips this migration entirely.
pub async fn get_home_app_dir() -> Result<PathBuf> {
    let use_default_dir = configured_home_app_dir().is_none();
    let path = app_home_dir()?;

    if use_default_dir && !fs::try_exists(&path).await.unwrap_or(false) {
        let home = dirs::home_dir().context("could not resolve home directory")?;
        let legacy = home.join(".automedon");
        if fs::try_exists(&legacy).await.unwrap_or(false) {
            fs::rename(&legacy, &path).await?;
        }
    }

    fs::create_dir_all(&path).await?;
    restrict_to_owner(&path).await;

    Ok(path)
}

/// Narrows the app directory to the owner alone on Unix. Windows uses the
/// profile directory's inherited ACLs instead.
///
/// Everything under here is private by content — transcripts hold whole files
/// the agent read and wrote — and the default `0755` left all of it readable
/// by any other local account.
///
/// Best-effort: a directory that cannot be narrowed is worth carrying on with,
/// since the alternative is an app that refuses to start.
#[cfg(unix)]
async fn restrict_to_owner(path: &PathBuf) {
    use std::os::unix::fs::PermissionsExt;

    if let Err(e) = fs::set_permissions(path, std::fs::Permissions::from_mode(0o700)).await {
        eprintln!("[app dir permissions err] {e}");
    }
}

#[cfg(not(unix))]
async fn restrict_to_owner(_path: &PathBuf) {
    // Windows directories inherit the user's profile ACL, so there is no Unix
    // mode bit to apply here.
}

/// `~/.dray/sessions`, creating it if needed.
pub async fn get_sessions_dir() -> Result<PathBuf> {
    let path = get_home_app_dir().await?.join("sessions");

    fs::create_dir_all(&path).await?;

    Ok(path)
}

/// Reads and parses `index.json`. Missing or empty file reads as no sessions,
/// not an error.
pub async fn list_session_index_items() -> Result<Vec<SessionIndexItem>> {
    let path = get_sessions_dir().await?.join("index.json");

    let contents = match fs::read_to_string(path).await {
        Ok(v) => v,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(e).context("could not open session file"),
    };

    if contents.is_empty() {
        return Ok(Vec::new());
    }

    let items = serde_json::from_str::<Vec<SessionIndexItem>>(&contents)?;

    Ok(items)
}

/// The index filtered to one side of `archived` — the sidebar shows exactly one
/// of the two at a time, so a parameter keeps it to one function rather than a
/// pair that would drift. Callers that need every entry (`set_*`, `get_*`) still
/// use [`list_session_index_items`] directly.
pub async fn list_session_index_items_by_archived(archived: bool) -> Result<Vec<SessionIndexItem>> {
    Ok(filter_by_archived(
        list_session_index_items().await?,
        archived,
    ))
}

/// Split out from the async read so it can be tested without an `index.json`.
fn filter_by_archived(items: Vec<SessionIndexItem>, archived: bool) -> Vec<SessionIndexItem> {
    items
        .into_iter()
        .filter(|i| i.archived == archived)
        .collect()
}

/// All sessions, bucketed by `project_path` — the sidebar's project grouping.
pub async fn list_sessions_by_project() -> Result<Vec<SessionIndexByProject>> {
    let sessions = list_session_index_items().await?;
    let mut sessions_grouped: Vec<SessionIndexByProject> = Vec::new();

    for session in sessions {
        if let Some(project) = sessions_grouped
            .iter_mut()
            .find(|p| p.path == session.project_path)
        {
            project.indexes.push(session);
        } else {
            sessions_grouped.push(SessionIndexByProject {
                path: session.project_path.clone(),
                indexes: vec![session],
            });
        }
    }

    Ok(sessions_grouped)
}

impl SessionIndexItem {
    /// Everything the index needs is known when the first prompt is sent, so a
    /// session appears in the list even if its process fails to start.
    pub fn new(
        session_id: &str,
        harness: Harness,
        cwd: &str,
        project_path: &str,
        worktree_name: Option<&str>,
        branch: Option<&str>,
        first_prompt: &str,
        model: ModelId,
        effort: Option<Effort>,
        parent_session_id: Option<&str>,
    ) -> Self {
        let now = now_rfc3339();

        Self {
            session_id: session_id.to_string(),
            harness,
            cwd: cwd.to_string(),
            project_path: project_path.to_string(),
            // The caller knows the branch metadata; nothing is derived from the
            // Worktree name here.
            branch: branch.map(str::to_string),
            worktree_name: worktree_name.map(str::to_string),
            cloud_name: None,
            title: title_from_prompt(first_prompt),
            model,
            agent_model: None,
            effort,
            status: SessionStatus::default(),
            fork_from: None,
            parent_session_id: parent_session_id.map(str::to_string),
            created: now.clone(),
            modified: now,
            archived: false,
            pinned: false,
        }
    }

    /// The entry for a fork of `self`. Everything deciding *how* the agent runs
    /// is inherited, since a fork continues the same conversation; everything
    /// describing this session's own history starts fresh.
    ///
    /// In-place forks share the existing checkout and identity. A Worktree fork
    /// gets a separate checkout, whose final path and branch the manager sets.
    pub fn fork(&self, session_id: &str, worktree_name: Option<&str>) -> Self {
        let now = now_rfc3339();

        Self {
            session_id: session_id.to_string(),
            harness: self.harness,
            cwd: match worktree_name {
                Some(name) => worktree_path(&self.project_path, name),
                None => self.cwd.clone(),
            },
            project_path: self.project_path.clone(),
            // A managed fork’s branch is set after its checkout is created.
            branch: self.branch.clone(),
            worktree_name: worktree_name
                .map(str::to_string)
                .or_else(|| self.worktree_name.clone()),
            cloud_name: None,
            title: fork_title(&self.title),
            model: self.model,
            agent_model: self.agent_model.clone(),
            effort: self.effort,
            status: SessionStatus::default(),
            fork_from: Some(self.session_id.clone()),
            // Inherited, so the copy sits exactly where the original does: the
            // sidebar draws it beside its source under the same parent, not
            // under its source, and the depth cap counts it at the same depth.
            // A fork that reset this to `None` would surface at the
            // top level and be free to spawn where the session it copied was
            // not — a depth cap a copy could walk around. Detach is the way out
            // for anyone who wants the fork standing on its own.
            parent_session_id: self.parent_session_id.clone(),
            created: now.clone(),
            modified: now,
            archived: false,
            pinned: false,
        }
    }
}

/// Marks a fork in the sidebar without costing a second row's worth of reading.
/// Truncated to the same width every other title is, and the suffix survives
/// truncation because a title cut off mid-word is what most needs the mark.
///
/// Strips a suffix already there before adding one, so forking a fork reads
/// `(fork)` once rather than stacking one per generation. Nothing tracks
/// lineage here anyway — see `fork_from`, which is an instruction and never a
/// record of it — so a title counting generations would promise more than the
/// rest of the feature keeps.
fn fork_title(parent: &str) -> String {
    const MAX: usize = 60;
    const SUFFIX: &str = " (fork)";

    let base = parent.strip_suffix(SUFFIX).unwrap_or(parent);

    let title = format!("{base}{SUFFIX}");
    if title.chars().count() <= MAX {
        return title;
    }

    let keep = MAX - SUFFIX.chars().count() - 1;
    let truncated: String = base.chars().take(keep).collect();
    format!("{}…{SUFFIX}", truncated.trim_end())
}

/// A Worktree id no other session has claimed.
pub async fn resolve_unclaimed_worktree_name(
    project_path: &str,
    _requested: Option<&str>,
) -> Result<String> {
    let claimed: Vec<String> = list_session_index_items()
        .await?
        .into_iter()
        .filter_map(|i| i.worktree_name)
        .collect();

    for _ in 0..16 {
        let id = random_worktree_id();
        if !claimed.contains(&id) && !PathBuf::from(worktree_path(project_path, &id)).exists() {
            return Ok(id);
        }
    }

    bail!("could not find an unused worktree id after 16 attempts")
}

/// Prefer the checkout's current branch, falling back to persisted metadata.
pub fn session_branch(item: &SessionIndexItem, observed: Option<&str>) -> Option<String> {
    observed
        .filter(|b| !b.is_empty())
        .map(str::to_string)
        .or_else(|| item.branch.clone())
}

/// Managed checkouts live inside the selected repository.
pub fn worktree_path(project: &str, id: &str) -> String {
    crate::worktrees::path(project, id)
}

/// A worktree id is a random UUID-4.
fn random_worktree_id() -> String {
    Uuid::new_v4().to_string()
}

/// Char-based so a multi-byte prompt can't panic on a byte-index slice.
fn title_from_prompt(prompt: &str) -> String {
    const MAX: usize = 60;
    let title = prompt.trim().replace('\n', " ");

    if title.is_empty() {
        return "Untitled".to_string();
    }

    if title.chars().count() <= MAX {
        return title;
    }

    let truncated: String = title.chars().take(MAX).collect();
    format!("{}…", truncated.trim_end())
}

/// Adds one entry to the index and rewrites it to disk.
pub async fn append_session_index_item(session: SessionIndexItem) -> Result<()> {
    let _guard = INDEX_LOCK.lock().await;

    let mut sessions = list_session_index_items().await?;
    sessions.push(session);

    write_session_index(&sessions).await
}

/// Bumps `modified`, and the settable per-session fields when they changed.
/// Callers hold the live session's values, so an unchanged send skips the
/// rewrite entirely — the whole index is serialized on every write.
pub async fn touch_session_index_item(
    session_id: &str,
    model: ModelId,
    agent_model: Option<&AgentModel>,
    effort: Option<Effort>,
) -> Result<()> {
    let _guard = INDEX_LOCK.lock().await;

    let mut sessions = list_session_index_items().await?;
    let Some(item) = sessions.iter_mut().find(|i| i.session_id == session_id) else {
        return Ok(());
    };

    item.modified = now_rfc3339();
    item.model = model;
    item.agent_model = agent_model.cloned();
    item.effort = effort;

    write_session_index(&sessions).await
}

/// Sets `archived` and/or `pinned` on one entry. `None` leaves that flag alone,
/// so the two sidebar controls share one command without either clobbering the
/// other's field. Returns the entry as written, or `None` if the id is unknown.
/// Cuts a session loose from the parent that spawned it, so the sidebar draws
/// it as a top-level row rather than nested.
///
/// One-way on purpose: there is no re-attach. Parentage records who *created*
/// a session, which is a fact about the past — a session detached and then
/// re-parented somewhere else would describe a history that never happened,
/// and nothing in the app needs that.
///
/// `modified` is left alone for [`set_session_flags`]'s reason: it orders the
/// list, and detaching must not jump the row to the top of it.
pub async fn detach_session(session_id: &str) -> Result<Option<SessionIndexItem>> {
    let _guard = INDEX_LOCK.lock().await;

    let mut sessions = list_session_index_items().await?;
    let Some(item) = sessions.iter_mut().find(|i| i.session_id == session_id) else {
        return Ok(None);
    };

    item.parent_session_id = None;
    let updated = item.clone();

    write_session_index(&sessions).await?;

    Ok(Some(updated))
}

pub async fn set_session_flags(
    session_id: &str,
    archived: Option<bool>,
    pinned: Option<bool>,
) -> Result<Option<SessionIndexItem>> {
    let _guard = INDEX_LOCK.lock().await;

    let mut sessions = list_session_index_items().await?;
    let Some(item) = sessions.iter_mut().find(|i| i.session_id == session_id) else {
        return Ok(None);
    };

    if let Some(v) = archived {
        item.archived = v;
    }
    if let Some(v) = pinned {
        item.pinned = v;
    }

    // `modified` is deliberately left alone: it orders the list, and flipping a
    // flag would jump the session to the top of it.
    let updated = item.clone();

    write_session_index(&sessions).await?;

    Ok(Some(updated))
}

/// Drops one session from the index and deletes its `.jsonl` log. Returns
/// whether the index held it — a `false` still means the log was removed if one
/// was there, so an orphaned log can't outlive the entry that named it.
///
/// Index first: a log with no entry is invisible, an entry with no log reads
/// back as a session with no events. Only one of those is a lie the UI shows.
pub async fn delete_session(session_id: &str) -> Result<bool> {
    let existed = {
        let _guard = INDEX_LOCK.lock().await;

        let mut sessions = list_session_index_items().await?;
        let before = sessions.len();
        sessions.retain(|i| i.session_id != session_id);

        if sessions.len() == before {
            false
        } else {
            write_session_index(&sessions).await?;
            true
        }
    };

    let path = get_session_path(session_id).await?;
    match fs::remove_file(&path).await {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(e).context("failed to delete session log"),
    }

    Ok(existed)
}

/// Sets one entry's status. Returns the entry as written, or `None` if the id
/// is unknown.
///
/// Only completion bumps `modified`: the field means "last activity", and the
/// agent finishing is activity. `InProgress` is already covered by the send's
/// touch, and clearing the unread mark is a read, not activity.
pub async fn set_session_status(
    session_id: &str,
    status: SessionStatus,
) -> Result<Option<SessionIndexItem>> {
    let _guard = INDEX_LOCK.lock().await;

    let mut sessions = list_session_index_items().await?;
    let Some(item) = sessions.iter_mut().find(|i| i.session_id == session_id) else {
        return Ok(None);
    };

    item.status = status;
    if status == SessionStatus::Completed {
        item.modified = now_rfc3339();
    }
    let updated = item.clone();

    write_session_index(&sessions).await?;

    Ok(Some(updated))
}

/// A persisted `in_progress` is a lie after a restart — no child survives the
/// process — so every one resets to `idle` at startup. `completed` survives:
/// unread is still unread.
pub async fn reset_in_progress_sessions() -> Result<()> {
    let _guard = INDEX_LOCK.lock().await;

    let mut sessions = list_session_index_items().await?;
    let mut changed = false;
    for item in sessions.iter_mut() {
        if item.status == SessionStatus::InProgress {
            item.status = SessionStatus::Idle;
            changed = true;
        }
    }

    if changed {
        write_session_index(&sessions).await?;
    }

    Ok(())
}

/// Replaces one entry's title. Returns the entry as written, or `None` if the
/// id is unknown — a session deleted while its title was being generated.
///
/// `modified` is left alone, like [`set_session_flags`]: it orders the sidebar,
/// and a title landing seconds after the send would jump the session to the top
/// of it for a reason the user never took.
pub async fn set_session_title(session_id: &str, title: &str) -> Result<Option<SessionIndexItem>> {
    let _guard = INDEX_LOCK.lock().await;

    let mut sessions = list_session_index_items().await?;
    let Some(item) = sessions.iter_mut().find(|i| i.session_id == session_id) else {
        return Ok(None);
    };

    item.title = title.to_string();
    let updated = item.clone();

    write_session_index(&sessions).await?;

    Ok(Some(updated))
}

/// Caller must hold `INDEX_LOCK`: this rewrites the whole file, so a concurrent
/// writer would drop the other's entry.
async fn write_session_index(sessions: &[SessionIndexItem]) -> Result<()> {
    let path = get_sessions_dir().await?.join("index.json");
    let contents = serde_json::to_string(sessions)?;
    let tmp = path.with_extension("json.tmp");

    fs::write(&tmp, contents)
        .await
        .context("failed to write session index")?;

    fs::rename(&tmp, &path)
        .await
        .context("failed to rename session index")?;

    Ok(())
}

/// Looks up one session's index entry by id.
pub async fn get_session_index_item(session_id: &str) -> Result<Option<SessionIndexItem>> {
    let items = list_session_index_items().await?;

    Ok(items.into_iter().find(|i| i.session_id == session_id))
}

/// `None` means the id isn't in the index. An indexed session with no log yet
/// is normal — it was written before its process spawned — and yields empty
/// `events` rather than `None`.
pub async fn get_session_by_id(session_id: &str) -> Result<Option<SessionSnapshot>> {
    let Some(index_item) = get_session_index_item(session_id).await? else {
        return Ok(None);
    };

    let events = list_session_events(session_id).await?;

    Ok(Some(SessionSnapshot { index_item, events }))
}

/// Replays a session's `.jsonl` log into its full event list. Missing file
/// reads as no events, not an error.
pub async fn list_session_events(session_id: &str) -> Result<Vec<AgentEvent>> {
    let path = get_session_path(session_id).await?;

    let buffer = match fs::read_to_string(&path).await {
        Ok(buf) => buf,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(e).context("could not open session file"),
    };

    let events = buffer
        .lines()
        .map(serde_json::from_str::<AgentEvent>)
        .collect::<Result<Vec<_>, _>>()
        .context("malformed session file")?;

    Ok(events)
}

/// Copies a session's log onto a new id, repointing every archived image at the
/// fork's own attachment directory. Returns what the fork will replay.
///
/// The rewrite is what makes the copy stand alone. `ImageRef.path` names a file
/// under `~/.dray/attachments/<session-id>/`, so a log copied verbatim would
/// draw its pictures out of the parent's directory — and deleting the parent
/// takes that directory with it, blanking images in a session that outlived it.
///
/// Missing images are the ordinary case (a session that attached none), and the
/// directory copy is best-effort for the same reason the archive write is: a
/// picture that fails to copy costs one image, not the fork.
pub async fn copy_session_log(from: &str, to: &str) -> Result<Vec<AgentEvent>> {
    let mut events = list_session_events(from).await?;

    // A session indexed before its process spawned has no log at all, and
    // nothing is written here rather than an empty one — the caller reads the
    // empty answer as "there is nothing to copy", and leaving no file behind is
    // what keeps a refused fork from stranding one under an unused id.
    if events.is_empty() {
        return Ok(events);
    }

    let attachments = get_home_app_dir().await?.join("attachments");
    let from_dir = attachments.join(from);
    let to_dir = attachments.join(to);
    copy_dir(&from_dir, &to_dir).await?;

    repoint_events(&mut events, to, &from_dir, &to_dir);

    let body: String = events
        .iter()
        .map(|e| serde_json::to_string(e).map(|s| format!("{s}\n")))
        .collect::<Result<Vec<_>, _>>()?
        .concat();

    // Written whole rather than appended to, so a fork onto an id that somehow
    // already has a log replaces it instead of interleaving two conversations.
    fs::write(get_session_path(to).await?, body).await?;

    Ok(events)
}

/// Rewrites a copied log to belong to `to`. Split out from the copy so it can be
/// tested without a `~/.dray` to write into.
fn repoint_events(events: &mut [AgentEvent], to: &str, from_dir: &Path, to_dir: &Path) {
    for event in events {
        // The envelope names the session that produced the event, and the
        // frontend routes live events by it. Left alone, the fork's log would
        // open claiming to be its parent's and then grow new events under its own
        // id — one log describing two sessions. `id` is deliberately not
        // re-minted: these are the same events, and nothing joins across sessions
        // on it.
        event.session_id = to.to_string();

        for image in event.payload.images_mut() {
            let Some(path) = &image.path else { continue };
            let Ok(rest) = Path::new(path).strip_prefix(from_dir) else {
                continue;
            };
            image.path = Some(to_dir.join(rest).to_string_lossy().into_owned());
        }
    }
}

/// Best-effort recursive copy. A missing source is the ordinary case — most
/// sessions attach no images — so it is not an error.
async fn copy_dir(from: &Path, to: &Path) -> Result<()> {
    let mut entries = match fs::read_dir(from).await {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(e).context("failed to read attachments"),
    };

    fs::create_dir_all(to).await?;

    while let Some(entry) = entries.next_entry().await? {
        let target = to.join(entry.file_name());
        if entry.file_type().await?.is_dir() {
            Box::pin(copy_dir(&entry.path(), &target)).await?;
        } else {
            fs::copy(entry.path(), target).await?;
        }
    }

    Ok(())
}

/// Retires a fork's pending-fork instruction once the CLI has carried it out.
/// Returns whether the entry was found; an unknown id means the session was
/// deleted between the spawn and this write, which is nothing to report.
pub async fn clear_fork_from(session_id: &str) -> Result<bool> {
    let _guard = INDEX_LOCK.lock().await;

    let mut sessions = list_session_index_items().await?;
    let Some(item) = sessions.iter_mut().find(|i| i.session_id == session_id) else {
        return Ok(false);
    };

    item.fork_from = None;
    write_session_index(&sessions).await?;

    Ok(true)
}

/// Appends one event as a line to the session's `.jsonl` log.
pub async fn append_session_event(session_id: &str, event: AgentEvent) -> Result<()> {
    let path = get_session_path(session_id).await?;

    let mut file = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .await
        .context("failed to open session file")?;

    let line = format!("{}\n", serde_json::to_string(&event)?);

    file.write_all(line.as_bytes()).await?;

    Ok(())
}

/// One unreadable line, kept for investigation.
#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ParseFailure {
    pub ts: String,
    pub session_id: String,
    /// Which stage gave up: `parse` (no variant matched the wire), `map` (the
    /// mapper errored), or `unknown_subtype` (parsed only by a catch-all).
    pub stage: String,
    pub detail: String,
    /// The raw line, whole. Truncating it would cost exactly the context these
    /// records exist to provide.
    pub raw: String,
}

static FAILURES_LOCK: Mutex<()> = Mutex::const_new(());

/// Records a line the harness layer could not turn into an event.
///
/// One file for the whole app, not one per session: these describe how well
/// this build covers the wire format, which is a property of the build rather
/// than of any conversation — and hunting a coverage gap across N session logs
/// is the thing that makes it never get done. Deliberately kept out of the
/// session `.jsonl`, whose contract is the normalized event model.
///
/// Unlike a session log this file has many concurrent writers, so it takes a
/// lock: a raw line can exceed the size the OS appends atomically, and two
/// sessions failing at once would interleave into unparseable records.
pub async fn record_parse_failure(
    session_id: &str,
    stage: &str,
    detail: &str,
    raw: &str,
) -> Result<()> {
    let failure = ParseFailure {
        ts: now_rfc3339(),
        session_id: session_id.to_string(),
        stage: stage.to_string(),
        detail: detail.to_string(),
        raw: raw.to_string(),
    };

    let path = get_home_app_dir().await?.join("parse_failures.jsonl");
    let line = format!("{}\n", serde_json::to_string(&failure)?);

    let _guard = FAILURES_LOCK.lock().await;

    let mut file = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .await
        .context("failed to open parse failure log")?;

    file.write_all(line.as_bytes()).await?;

    Ok(())
}

/// Tail-reads the log's last line to continue its `seq` counter on resume.
pub async fn next_seq_by_session_id(session_id: &str) -> Result<u64> {
    let path = get_session_path(session_id).await?;

    let buf = match fs::read_to_string(&path).await {
        Ok(buf) => buf,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(0),
        Err(e) => return Err(e).context("could not read session file"),
    };

    let seq = match buf.lines().next_back() {
        Some(v) => {
            let json: Value = serde_json::from_str(v)?;
            json.get("seq").and_then(|s| s.as_u64()).unwrap_or(0)
        }
        None => 0,
    };

    Ok(seq + 1)
}

/// Path to a session's `.jsonl` log under the sessions dir.
pub async fn get_session_path(session_id: &str) -> Result<PathBuf> {
    let path = get_sessions_dir()
        .await?
        .join(format!("{session_id}.jsonl"));

    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::events::{AgentEventPayload, ImageRef, ToolResult};

    #[test]
    fn an_empty_prompt_uses_the_untitled_fallback() {
        assert_eq!(title_from_prompt(""), "Untitled");
        assert_eq!(title_from_prompt(" \n\t "), "Untitled");
    }

    #[test]
    fn index_entries_written_before_these_fields_still_read() {
        let legacy = r#"{"sessionId":"a","harness":"claude_code","cwd":"/p","projectPath":"/p",
            "branch":null,"cloudName":null,"title":"t","created":"c","modified":"m",
            "archived":false,"pinned":false}"#;

        let item: SessionIndexItem = serde_json::from_str(legacy).unwrap();

        assert_eq!(item.status, SessionStatus::Idle);
        // Reads back as a model no build lists, so it can never reach a spawn.
        assert_eq!(item.model, ModelId::Unknown);
        assert!(crate::models::find_model(item.model, item.agent_model.as_ref()).is_none());
    }

    #[test]
    fn legacy_index_entry_reads_as_no_pending_fork() {
        let legacy = r#"{"sessionId":"a","harness":"claude_code","cwd":"/p","projectPath":"/p",
            "branch":null,"cloudName":null,"title":"t","created":"c","modified":"m",
            "archived":false,"pinned":false}"#;

        let item: SessionIndexItem = serde_json::from_str(legacy).unwrap();

        // The field is an instruction, so absent has to read as "nothing to do".
        // Reading it any other way would fork every session predating it on its
        // next send.
        assert_eq!(item.fork_from, None);
    }

    /// In-place forks retain the worktree identity so cleanup checks all users.
    #[test]
    fn forking_in_place_shares_the_managed_tree() {
        let mut parent = SessionIndexItem::new(
            "parent",
            Harness::Dray,
            "/p/.lathe/worktrees/wt",
            "/p",
            Some("wt"),
            Some("main"),
            "add the PR panel",
            ModelId::Dray,
            Some(Effort::High),
            None,
        );
        parent.archived = true;
        parent.pinned = true;
        parent.status = SessionStatus::Completed;

        let fork = parent.fork("child", None);

        assert_eq!(fork.cwd, parent.cwd, "the fork runs where the parent does");
        assert_eq!(fork.branch, parent.branch, "so its PR tab finds the branch");
        assert_eq!(
            fork.worktree_name.as_deref(),
            Some("wt"),
            "the checkout is shared"
        );
        assert_eq!(fork.fork_from.as_deref(), Some("parent"));

        // How the agent runs is inherited; this session's own history is not.
        assert_eq!(fork.model, parent.model);
        assert_eq!(fork.effort, parent.effort);
        assert_eq!(fork.status, SessionStatus::Idle);
        assert!(!fork.archived, "a fork is new work, not settled work");
        assert!(!fork.pinned);
    }

    /// The four cases `sessionBranch` in `pr.ts` is tested on, so `--from` and
    /// the PR tab cannot come to disagree about which branch a session is on.
    #[test]
    fn a_sessions_branch_reads_the_same_way_the_pr_tab_reads_it() {
        let worktree = SessionIndexItem::new(
            "a",
            Harness::Dray,
            "/p/.lathe/worktrees/calm-owl",
            "/p",
            Some("calm-owl"),
            Some("main"),
            "hi",
            ModelId::Dray,
            None,
            None,
        );
        // Worktree branch metadata is recorded directly rather than rebuilt from
        // the Worktree directory name.
        assert_eq!(session_branch(&worktree, None).as_deref(), Some("main"));
        // Git's own reading outranks the recorded branch: anything checking out
        // another branch inside the tree leaves the record describing one it left.
        assert_eq!(
            session_branch(&worktree, Some("fix/thing")).as_deref(),
            Some("fix/thing")
        );

        let plain = SessionIndexItem::new(
            "b",
            Harness::Dray,
            "/p",
            "/p",
            None,
            Some("feature"),
            "hi",
            ModelId::Dray,
            None,
            None,
        );
        assert_eq!(session_branch(&plain, None).as_deref(), Some("feature"));
    }

    /// A fork is a copy, so it sits exactly where the original sits: beside its
    /// source under the same parent, at the same depth. Resetting this would
    /// surface the copy at the top level and let it spawn where the session it
    /// copied could not — a depth cap a copy could walk around.
    #[test]
    fn a_fork_keeps_its_source_place_in_the_spawn_chain() {
        let mut spawned = SessionIndexItem::new(
            "spawned",
            Harness::Dray,
            "/p",
            "/p",
            None,
            None,
            "work the issue",
            ModelId::Dray,
            None,
            Some("orchestrator"),
        );
        assert_eq!(spawned.parent_session_id.as_deref(), Some("orchestrator"));

        let fork = spawned.fork("child", None);
        assert_eq!(
            fork.parent_session_id.as_deref(),
            Some("orchestrator"),
            "the copy is a sibling of its source, not a root"
        );

        // A session nobody spawned forks to one nobody spawned.
        spawned.parent_session_id = None;
        assert_eq!(spawned.fork("child", None).parent_session_id, None);
    }

    #[test]
    fn forking_into_a_worktree_takes_a_tree_of_its_own() {
        let parent = SessionIndexItem::new(
            "parent",
            Harness::Dray,
            "/p",
            "/p",
            None,
            Some("main"),
            "add the PR panel",
            ModelId::Dray,
            None,
            None,
        );

        let fork = parent.fork("child", Some("bold-otter"));

        assert_eq!(fork.cwd, worktree_path("/p", "bold-otter"));
        assert_eq!(fork.project_path, "/p", "it still groups under the project");
        assert_eq!(fork.worktree_name.as_deref(), Some("bold-otter"));
        assert_eq!(
            fork.branch.as_deref(),
            parent.branch.as_deref(),
            "the manager assigns a new branch after checkout creation"
        );
    }

    /// The suffix is the whole point of the title, so truncation takes its room
    /// from the parent's text rather than from the mark.
    #[test]
    fn a_long_title_keeps_its_fork_mark() {
        let long = "a".repeat(80);
        let title = fork_title(&long);

        assert!(title.ends_with(" (fork)"), "{title}");
        assert_eq!(title.chars().count(), 60);
        assert_eq!(fork_title("short"), "short (fork)");
    }

    /// Forking a fork must not stack the mark — nothing here tracks lineage, so
    /// counting generations in the title would promise more than the feature
    /// keeps.
    #[test]
    fn forking_a_fork_keeps_one_suffix_not_two() {
        assert_eq!(fork_title("Add PR panel (fork)"), "Add PR panel (fork)");
        assert_eq!(
            fork_title(&fork_title("Add PR panel")),
            "Add PR panel (fork)"
        );
    }

    /// A fork's log has to stand on its own. Both rewrites are about outliving
    /// the parent: events still naming it would put one log's worth of history
    /// under two session ids, and an image still pointing into its attachment
    /// directory goes blank the moment the parent is deleted.
    #[test]
    fn a_copied_log_belongs_to_the_fork_that_replays_it() {
        let from_dir = Path::new("/home/.dray/attachments/parent");
        let to_dir = Path::new("/home/.dray/attachments/child");

        let image = |path: &str| ImageRef {
            path: Some(path.to_string()),
            url: None,
            mime_type: None,
        };
        let event = |payload| AgentEvent {
            id: "e1".into(),
            session_id: "parent".into(),
            harness: Harness::Dray,
            seq: 0,
            ts: "t".into(),
            turn_id: None,
            payload,
            raw: None,
        };

        let mut events = vec![
            event(AgentEventPayload::UserMessage {
                text: "look at this".into(),
                images: vec![image("/home/.dray/attachments/parent/a.png")],
                baseline: None,
                queued: false,
                from: None,
            }),
            event(AgentEventPayload::ToolCallCompleted {
                call_id: "c1".into(),
                result: ToolResult {
                    text: "shot".into(),
                    is_error: false,
                    structured: None,
                    exit_code: None,
                    duration_ms: None,
                    images: vec![
                        image("/home/.dray/attachments/parent/b.png"),
                        // Not ours to move: an image the archive never took.
                        image("/tmp/elsewhere.png"),
                    ],
                },
            }),
        ];

        repoint_events(&mut events, "child", from_dir, to_dir);

        assert!(events.iter().all(|e| e.session_id == "child"));

        let paths: Vec<_> = events
            .iter_mut()
            .flat_map(|e| e.payload.images_mut().to_vec())
            .filter_map(|i| i.path)
            .collect();
        assert_eq!(
            paths,
            vec![
                to_dir.join("a.png").to_string_lossy().into_owned(),
                to_dir.join("b.png").to_string_lossy().into_owned(),
                "/tmp/elsewhere.png".to_owned(),
            ]
        );
    }

    #[test]
    fn archived_filter_splits_the_index_into_two_disjoint_views() {
        let item = |id: &str, archived: bool| {
            let mut i = SessionIndexItem::new(
                id,
                Harness::Dray,
                "/p",
                "/p",
                None,
                None,
                "hi",
                ModelId::Dray,
                None,
                None,
            );
            i.archived = archived;
            i
        };
        let items = vec![item("a", false), item("b", true), item("c", false)];

        let active = filter_by_archived(items.clone(), false);
        let settled = filter_by_archived(items, true);

        assert_eq!(
            active
                .iter()
                .map(|i| i.session_id.as_str())
                .collect::<Vec<_>>(),
            ["a", "c"]
        );
        assert_eq!(
            settled
                .iter()
                .map(|i| i.session_id.as_str())
                .collect::<Vec<_>>(),
            ["b"]
        );
    }

    /// A Worktree session preserves the branch metadata supplied by the session manager.
    #[test]
    fn a_worktree_session_records_the_branch_its_work_lands_on() {
        let item = SessionIndexItem::new(
            "a",
            Harness::Dray,
            "/p/.lathe/worktrees/calm-owl",
            "/p",
            Some("calm-owl"),
            Some("main"),
            "hi",
            ModelId::Dray,
            None,
            None,
        );

        assert_eq!(item.branch.as_deref(), Some("main"));
    }

    #[test]
    fn snapshot_flattens_index_fields_beside_events() {
        let item = SessionIndexItem::new(
            "a",
            Harness::Dray,
            "/p",
            "/p",
            None,
            Some("main"),
            "hi",
            ModelId::Dray,
            Some(Effort::High),
            None,
        );
        let json = serde_json::to_value(SessionSnapshot {
            index_item: item,
            events: vec![],
        })
        .unwrap();

        assert_eq!(json["sessionId"], "a");
        assert_eq!(json["status"], "idle");
        assert!(json["events"].is_array());
        assert!(
            json.get("indexItem").is_none(),
            "must stay flat for the generated TS type"
        );
    }
}
