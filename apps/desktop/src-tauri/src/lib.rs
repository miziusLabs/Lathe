// Tauri embeds Common Controls 6 in binaries, while the library test harness
// also needs that manifest for the Windows dialog imports to load correctly.
#[cfg(all(test, windows, target_env = "gnu"))]
#[link(
    name = "libresource.a",
    kind = "static",
    modifiers = "+verbatim,-bundle"
)]
extern "C" {}

use crate::{
    attachments::Attachment,
    files::FileMatch,
    git::BranchList,
    harness::dray::commands::SlashCommand,
    models::{AgentModel, Effort, Model, ModelId},
    projects::Project,
    session::{Harness, QueuedMessage, SendOutcome, SessionManager},
    store::{SessionIndexByProject, SessionIndexItem, SessionSnapshot, SessionStatus},
};
use std::collections::HashMap;
use tauri::{AppHandle, Manager, State, WindowEvent};

pub mod account;
pub mod attachments;
pub mod binpath;
#[path = "events/events.rs"]
pub mod events;
pub mod files;
pub mod git;
pub mod github;
#[path = "harness/harness.rs"]
pub mod harness;
#[path = "models/models.rs"]
pub mod models;
pub mod notifications;
pub mod profile;
pub mod projects;
pub mod quit;
pub mod session;
pub mod store;
pub mod title;
pub(crate) mod tls;
pub mod usage;
pub mod worktrees;

#[tauri::command]
async fn get_plan_usage() -> Result<usage::PlanUsage, String> {
    usage::fetch().await.map_err(|error| error.to_string())
}

#[tauri::command]
fn get_account_status() -> Result<account::AccountStatus, String> {
    account::status().map_err(|error| error.to_string())
}

#[tauri::command]
async fn get_local_profile_picture() -> Result<Option<String>, String> {
    profile::picture_path()
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
async fn save_local_profile_picture(source_path: &str) -> Result<String, String> {
    profile::save_picture(source_path)
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
async fn sign_in_chatgpt(app: AppHandle) -> Result<(), String> {
    account::begin(app).await.map_err(|error| error.to_string())
}

#[tauri::command]
async fn cancel_chatgpt_sign_in() {
    account::cancel().await;
}

#[tauri::command]
async fn sign_out_chatgpt(
    app: AppHandle,
    manager: State<'_, SessionManager>,
) -> Result<(), String> {
    // A signed-out account must not continue issuing requests in existing children.
    manager.disconnect(&app).await;
    let result = account::sign_out(&app).await;
    manager.disconnect(&app).await;
    result.map_err(|error| error.to_string())
}

#[tauri::command]
async fn send_msg(
    session_id: &str,
    prompt: &str,
    attachment_paths: Vec<String>,
    harness: &str,
    model: ModelId,
    agent_model: Option<AgentModel>,
    effort: Option<Effort>,
    title_model: Option<AgentModel>,
    title_effort: Option<Effort>,
    cwd: &str,
    project_path: Option<&str>,
    branch: Option<&str>,
    use_worktree: bool,
    worktree_name: Option<&str>,
    is_new_session: bool,
    queue_after_turn: bool,
    app: AppHandle,
    manager: State<'_, SessionManager>,
) -> Result<SendOutcome, String> {
    let harness = match harness {
        "dray" | "dray_agent" => Harness::Dray,
        _ => return Err("invalid harness".into()),
    };

    manager
        .send_msg(
            session_id,
            prompt,
            &attachment_paths,
            harness,
            model,
            agent_model,
            effort,
            title_model,
            title_effort,
            cwd,
            project_path,
            branch,
            use_worktree,
            worktree_name,
            // The composer has no explicit base ref; its selected branch is
            // recorded and used as the worktree's starting point.
            None,
            is_new_session,
            queue_after_turn,
            // The composer has no parent session, and its prompts are the
            // user's own.
            None,
            None,
            &app,
        )
        .await
        .map_err(|e| e.to_string())
}

/// Describes dropped or picked paths for the composer's tray. Returns only the
/// ones that can be attached — a folder dragged in alongside two files leaves
/// the two files.
#[tauri::command]
async fn read_attachments(paths: Vec<String>) -> Vec<Attachment> {
    attachments::read_attachments(paths).await
}

/// Saves image bytes from a clipboard paste and returns the same tray shape as a
/// picked or dropped image.
#[tauri::command]
async fn save_pasted_image(bytes: Vec<u8>, mime_type: String) -> Result<Attachment, String> {
    attachments::save_pasted_image(bytes, mime_type)
        .await
        .map_err(|e| e.to_string())
}

/// Saves non-image bytes from a clipboard paste when the webview did not expose
/// the original filesystem path.
#[tauri::command]
async fn save_pasted_file(bytes: Vec<u8>, name: String) -> Result<Attachment, String> {
    attachments::save_pasted_file(bytes, name)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
async fn list_models(harness: Option<&str>, cwd: Option<&str>) -> Result<Vec<Model>, String> {
    match harness {
        None | Some("dray") | Some("dray_agent") => harness::dray::commands::list_models(cwd)
            .await
            .map_err(|error| error.to_string()),
        Some(_) => Err("invalid harness".into()),
    }
}

/// The slash commands available in a directory. Cached per directory in the
/// backend, so the composer may call this whenever the project changes.
#[tauri::command]
async fn list_slash_commands(
    cwd: &str,
    harness: Option<&str>,
) -> Result<Vec<SlashCommand>, String> {
    let commands = match harness {
        None | Some("dray") | Some("dray_agent") => {
            harness::dray::commands::list_commands(cwd).await
        }
        Some(_) => return Err("invalid harness".into()),
    };
    commands.map_err(|e| e.to_string())
}

/// Starts indexing a directory's files so the `@` picker opens on a warm index.
/// Fire-and-forget: the walk runs on its own thread and this returns at once.
#[tauri::command]
async fn warm_file_index(cwd: String) -> Result<(), String> {
    tokio::task::spawn_blocking(move || files::warm(&cwd))
        .await
        .map_err(|e| e.to_string())?
        .map_err(|e| e.to_string())
}

/// Fuzzy file search for the `@` picker.
///
/// On `spawn_blocking` because the index is synchronous throughout — the search
/// holds a `parking_lot` read guard across the whole scoring pass, which is not
/// something that may be held across an await point.
#[tauri::command]
async fn search_files(cwd: String, query: String, limit: usize) -> Result<Vec<FileMatch>, String> {
    tokio::task::spawn_blocking(move || files::search(&cwd, &query, limit))
        .await
        .map_err(|e| e.to_string())?
        .map_err(|e| e.to_string())
}

#[tauri::command]
async fn list_sessions_by_project() -> Result<Vec<SessionIndexByProject>, String> {
    store::list_sessions_by_project()
        .await
        .map_err(|e| e.to_string())
}

/// One side of the archived split. The sidebar's toggle is the only caller, and
/// it never wants both at once, so the flag it holds is the argument.
#[tauri::command]
async fn list_session_index_items(archived: bool) -> Result<Vec<SessionIndexItem>, String> {
    store::list_session_index_items_by_archived(archived)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
async fn get_session_by_id(session_id: &str) -> Result<Option<SessionSnapshot>, String> {
    store::get_session_by_id(session_id)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
async fn list_projects() -> Result<Vec<Project>, String> {
    projects::read_projects().await.map_err(|e| e.to_string())
}

#[tauri::command]
async fn add_project(path: &str) -> Result<Vec<Project>, String> {
    projects::add_project(path).await.map_err(|e| e.to_string())
}

#[tauri::command]
async fn rename_project(path: &str, name: &str) -> Result<Vec<Project>, String> {
    projects::rename_project(path, name)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
async fn remove_project(path: &str) -> Result<Vec<Project>, String> {
    projects::remove_project(path)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
async fn set_last_selected_project(path: &str) -> Result<(), String> {
    projects::set_last_selected_project(path)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
async fn reorder_projects(paths: Vec<String>) -> Result<Vec<Project>, String> {
    projects::reorder_projects(&paths)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
async fn list_branches(cwd: &str) -> Result<BranchList, String> {
    git::list_branches(cwd).await.map_err(|e| e.to_string())
}

/// Returns the branch list as it stands after the switch, so the picker
/// re-renders from one round trip rather than following up with its own.
#[tauri::command]
async fn checkout_branch(cwd: &str, branch: &str, stash: bool) -> Result<BranchList, String> {
    git::checkout_branch(cwd, branch, stash)
        .await
        .map_err(|e| e.to_string())?;

    git::list_branches(cwd).await.map_err(|e| e.to_string())
}

/// What changed in `cwd` since `baseline` — the tree id carried on a
/// `user_message`, so "since the last prompt" is the caller picking which one.
///
/// `head` is the frozen snapshot from the turn's own `turn_completed`, passed
/// for a finished turn so the diff describes the turn rather than everything
/// that has touched the checkout since. Absent — a live turn, or one that
/// never closed — the working tree is snapshotted to answer, and that
/// snapshot's id comes back on `head`. Pass it to [`file_change`] either way:
/// the agent keeps writing while the panel is open, and a list and a diff
/// taken from two different snapshots would disagree about what the file says.
#[tauri::command]
async fn changes_since(
    cwd: &str,
    baseline: &str,
    head: Option<&str>,
) -> Result<git::ChangeSet, String> {
    git::changes_since(cwd, baseline, head)
        .await
        .map_err(|e| e.to_string())
}

/// Both sides of one file, fetched only when the reader opens that row. The
/// file list is cheap and the contents are not, so a turn touching thirty files
/// costs thirty rows and nothing else until something is expanded.
#[tauri::command]
async fn file_change(
    cwd: &str,
    base: &str,
    head: &str,
    path: &str,
    old_path: Option<&str>,
) -> Result<git::FileVersions, String> {
    git::file_versions(cwd, base, head, path, old_path)
        .await
        .map_err(|e| e.to_string())
}

/// The committed side of the repo view's uncommitted list. Paired with a `None`
/// head on [`changes_since`], which snapshots the working tree to answer — so
/// the two together are "what have I changed but not committed".
///
/// Takes an owned `cwd` where every command around it borrows: an async command
/// with a borrowed argument has to return `Result`, and neither this nor
/// [`sync_status`] can fail — a `Result` here would be a lie the caller then has
/// to handle.
#[tauri::command]
async fn head_tree(cwd: String) -> Option<String> {
    git::head_tree(&cwd).await
}

/// A page of the current branch's history, newest first. `skip` is what the
/// list's "load more" advances; the page size is the caller's, capped in `git`.
#[tauri::command]
async fn log_commits(cwd: &str, limit: u32, skip: u32) -> Result<Vec<git::Commit>, String> {
    git::log_commits(cwd, limit, skip)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
async fn sync_status(cwd: String) -> git::SyncStatus {
    git::sync_status(&cwd).await
}

#[tauri::command]
async fn work_status(cwd: String) -> git::WorkStatus {
    git::work_status(&cwd).await
}

/// Commits the checked files alone. `paths` are this session's own change list,
/// so a path the reader unchecked is one this never sees.
#[tauri::command]
async fn commit_files(
    cwd: &str,
    summary: &str,
    description: Option<&str>,
    paths: Vec<String>,
) -> Result<(), String> {
    git::commit_files(cwd, summary, description, &paths)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
async fn push_branch(cwd: &str) -> Result<(), String> {
    git::push_branch(cwd).await.map_err(|e| e.to_string())
}

/// Returns the entry as written so the sidebar re-renders from the stored value
/// rather than its own guess at it. `None` for an unknown id.
#[tauri::command]
async fn set_session_flags(
    session_id: &str,
    archived: Option<bool>,
    pinned: Option<bool>,
) -> Result<Option<SessionIndexItem>, String> {
    store::set_session_flags(session_id, archived, pinned)
        .await
        .map_err(|e| e.to_string())
}

/// Cuts a spawned session loose from its parent, so the sidebar stops nesting
/// it. Returns the entry as written; `None` for an unknown id.
#[tauri::command]
async fn detach_session(session_id: &str) -> Result<Option<SessionIndexItem>, String> {
    store::detach_session(session_id)
        .await
        .map_err(|e| e.to_string())
}

/// Removes a session for good: its child, its index entry, and its log. `false`
/// means the index never held the id, which the sidebar treats the same as a
/// success — either way the row it was asked to remove is gone.
#[tauri::command]
async fn delete_session(
    session_id: &str,
    manager: State<'_, SessionManager>,
) -> Result<bool, String> {
    manager.delete(session_id).await.map_err(|e| e.to_string())
}

/// Copies a session onto `fork_id`, to be carried on separately from the one it
/// came from. `worktree` gives the fork a tree of its own rather than leaving it
/// in the parent's directory.
///
/// The id comes from the caller for the same reason a new session's does: this
/// app chooses session ids and the CLI adopts them, and `--fork-session` honours
/// `--session-id` like any other spawn.
///
/// Returns what the fork replays — the parent's log, already copied — so the
/// frontend can open it without a second read.
#[tauri::command]
async fn fork_session(
    session_id: &str,
    fork_id: &str,
    worktree: bool,
    manager: State<'_, SessionManager>,
) -> Result<SessionSnapshot, String> {
    manager
        .fork(session_id, fork_id, worktree)
        .await
        .map_err(|e| e.to_string())
}

/// Stops all work for a session immediately. The live Lathe child is terminated;
/// the next prompt resumes the persisted session in a fresh process.
#[tauri::command]
async fn interrupt_session(
    session_id: &str,
    app: AppHandle,
    manager: State<'_, SessionManager>,
) -> Result<(), String> {
    manager
        .interrupt(session_id, &app)
        .await
        .map_err(|e| e.to_string())
}

/// Takes back the newest prompt still held for a running turn, returning its
/// text so the composer can restore it. `None` once the flush has written it —
/// past that point the CLI owns the prompt and there is no way to retract it.
#[tauri::command]
async fn cancel_queued(
    session_id: &str,
    manager: State<'_, SessionManager>,
) -> Result<Option<QueuedMessage>, String> {
    Ok(manager.cancel_queued(session_id).await)
}

/// Answers the questions on a `questions_asked` event. `answers` is keyed by
/// each question's verbatim text — the CLI matches on the string — and a
/// question left out of it is one the user skipped, which is a real answer
/// rather than a refusal.
#[tauri::command]
async fn answer_questions(
    session_id: &str,
    request_id: &str,
    answers: HashMap<String, String>,
    manager: State<'_, SessionManager>,
    app: AppHandle,
) -> Result<(), String> {
    manager
        .answer_questions(session_id, request_id, answers, &app)
        .await
        .map_err(|e| e.to_string())
}

/// Clears a finished session's unread mark. The frontend calls this when the
/// user views the session; a `completed` badge is "finished and unread", so
/// reading is what retires it. Returns the status as written, `None` when
/// nothing changed — the session wasn't `completed`, or the id is unknown.
#[tauri::command]
async fn mark_session_idle(
    session_id: &str,
    manager: State<'_, SessionManager>,
) -> Result<Option<SessionStatus>, String> {
    manager
        .mark_idle(session_id)
        .await
        .map_err(|e| e.to_string())
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tls::initialize();
    let builder = tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .manage(SessionManager::default())
        .manage(quit::PendingQuit::default());

    // macOS needs a custom menu so Cmd+Q reaches the in-app confirmation.
    // Other platforms use the window's preventable close event and should not
    // get Tauri's default native menu bar.
    #[cfg(target_os = "macos")]
    let builder = builder.menu(quit::menu).on_menu_event(|app, event| {
        if event.id() == quit::QUIT_ID {
            quit::request(app);
        }
    });

    builder
        .on_window_event(|window, event| {
            // The dialog answers with `confirm_quit`, which exits outright — so
            // this arm never has to let a close through.
            if let WindowEvent::CloseRequested { api, .. } = event {
                api.prevent_close();
                quit::request(window.app_handle());
            }
        })
        .setup(|_app| {
            // A persisted `in_progress` can't be true anymore — no child
            // survived the restart. Spawned, not awaited: the reset needs no
            // window, and the frontend's first fetch lands well after it.
            tauri::async_runtime::spawn(async {
                if let Err(e) = store::reset_in_progress_sessions().await {
                    eprintln!("[status reset err] {e}");
                }
            });

            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            get_plan_usage,
            get_account_status,
            get_local_profile_picture,
            save_local_profile_picture,
            sign_in_chatgpt,
            sign_out_chatgpt,
            cancel_chatgpt_sign_in,
            send_msg,
            read_attachments,
            save_pasted_image,
            save_pasted_file,
            list_models,
            list_slash_commands,
            warm_file_index,
            search_files,
            list_sessions_by_project,
            list_session_index_items,
            get_session_by_id,
            list_projects,
            add_project,
            rename_project,
            remove_project,
            set_last_selected_project,
            reorder_projects,
            list_branches,
            checkout_branch,
            changes_since,
            file_change,
            head_tree,
            log_commits,
            sync_status,
            work_status,
            commit_files,
            push_branch,
            set_session_flags,
            detach_session,
            delete_session,
            fork_session,
            mark_session_idle,
            interrupt_session,
            cancel_queued,
            answer_questions,
            notifications::notify_session,
            github::prs_for_branch,
            github::pr_marks,
            github::merge_pr,
            github::reopen_pr,
            github::mark_pr_ready,
            quit::confirm_quit,
            quit::dismiss_quit,
            quit::prepare_for_update,
            quit::restart_after_update,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
