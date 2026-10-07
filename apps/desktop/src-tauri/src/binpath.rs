//! Finding agent binaries when the app wasn't launched from a shell.
//!
//! A bundled `.app` started from Finder or the Dock inherits `launchd`'s
//! minimal environment rather than the user's PATH. Resolve Lathe once and reuse
//! it: the login-shell probe on Unix costs real time, and the answer cannot
//! change while the app runs.

use std::path::{Path, PathBuf};
#[cfg(not(windows))]
use std::process::Stdio;
use std::sync::OnceLock;
use tokio::process::Command;

#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x08000000;

static GH_PATH: OnceLock<Option<PathBuf>> = OnceLock::new();
#[cfg(windows)]
pub fn configure_command(command: &mut Command) {
    command.creation_flags(CREATE_NO_WINDOW);
}
#[cfg(not(windows))]
pub fn configure_command(_command: &mut Command) {}

/// The agent ships in the desktop executable; PATH never selects another agent.
pub async fn agent_command() -> Command {
    let mut command = Command::new(std::env::current_exe().expect("desktop executable path"));
    command.arg("--agent");
    configure_command(&mut command);
    command
}

/// Reconstructs the PATH a child needs when the app was launched by Finder or
/// Explorer.
///
/// Bundled macOS apps inherit launchd's minimal PATH, while a Windows GUI app
/// may be started before the user's shell profile has added Node/npm. Existing
/// entries stay first so an explicitly configured binary still wins.
pub fn agent_path() -> String {
    let inherited = std::env::var_os("PATH").unwrap_or_default();
    let mut dirs: Vec<_> = std::env::split_paths(&inherited).collect();

    for dir in known_dirs() {
        if !dirs.contains(&dir) {
            dirs.push(dir);
        }
    }

    std::env::join_paths(dirs)
        .map(|joined| joined.to_string_lossy().into_owned())
        .unwrap_or_else(|_| inherited.to_string_lossy().into_owned())
}

/// The absolute path to `gh`, or `None` where it isn't installed.
///
/// `None` rather than a bare-name fallback: `gh` is optional
/// here, and the caller turns a missing one into a line telling the reader to
/// install it. A spawn error naming the binary would be the same fact worded as
/// a crash.
///
/// The answer is cached including its absence, so installing `gh` while the app
/// runs needs a restart — the same bargain the login-shell probe already makes.
pub async fn gh() -> Option<PathBuf> {
    if let Some(path) = GH_PATH.get() {
        return path.clone();
    }

    let resolved = resolve("gh").await;
    let _ = GH_PATH.set(resolved);
    GH_PATH.get().cloned().flatten()
}

/// Looks for `bin` on the inherited `PATH`, then in the usual install
/// locations, then by asking a login shell. Ordered by cost: the first two are
/// filesystem checks, the last spawns a shell that reads the user's rc files.
async fn resolve(bin: &str) -> Option<PathBuf> {
    if let Some(path) = search_path(bin) {
        return Some(path);
    }

    if let Some(path) = search_known_dirs(bin) {
        return Some(path);
    }

    #[cfg(windows)]
    {
        // Explorer has no POSIX login shell. The Windows search above covers
        // PATH and the common install locations without invoking cmd.exe.
        None
    }

    #[cfg(not(windows))]
    {
        login_shell_which(bin).await
    }
}

/// Walks the `PATH` this process actually inherited. Covers `tauri dev` and any
/// launch from a terminal, where nothing further is needed.
fn search_path(bin: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path).find_map(|dir| {
        executable_candidates(&dir, bin)
            .into_iter()
            .find(|candidate| is_executable(candidate))
    })
}

/// Where a user-installed CLI tends to land.
///
/// Public because the spawn needs them for the *other* direction: a child
/// inherits this process's `PATH`, and a bundled app launched from Finder or
/// Explorer may hold none of these. So a `dray` the user has installed is
/// invisible to the agent unless these are put back — the same failure this
/// module exists to solve for Lathe, one layer out.
pub fn known_dirs() -> Vec<PathBuf> {
    let Some(home) = dirs::home_dir() else {
        return Vec::new();
    };

    let mut dirs = vec![
        // These user-local locations are shared by Unix shells, Bun on
        // Windows, and MSYS installations. They are safe to include on both
        // platforms because they are relative to the user's home directory.
        home.join(".local/bin"),
        home.join(".bun/bin"),
        home.join(".npm-global/bin"),
    ];

    #[cfg(not(windows))]
    {
        dirs.extend([
            PathBuf::from("/opt/homebrew/bin"),
            PathBuf::from("/usr/local/bin"),
        ]);

        // Lathe is commonly installed through npm under nvm. Those directories
        // are not stable enough to list statically, but they must be in a
        // child's PATH as well as in the resolver's search path because Lathe is
        // a Node script.
        if let Ok(versions) = std::fs::read_dir(home.join(".nvm/versions/node")) {
            dirs.extend(versions.flatten().map(|entry| entry.path().join("bin")));
        }
    }

    #[cfg(windows)]
    {
        // npm's per-user Windows prefix is where `npm install -g` puts command
        // launchers such as `dray.cmd`. A GUI app may not inherit the shell PATH,
        // so include it in both resolution and the child environment.
        if let Some(data_dir) = dirs::data_dir() {
            dirs.push(data_dir.join("npm"));
        }

        // Node's installer and per-user installer use these two locations. The
        // inherited PATH usually contains them, but Explorer-launched apps do
        // not reliably see a shell's later PATH mutations.
        if let Some(local_data) = dirs::data_local_dir() {
            dirs.push(local_data.join("Programs/nodejs"));
        }
        for variable in ["ProgramFiles", "ProgramW6432"] {
            if let Some(program_files) = std::env::var_os(variable) {
                dirs.push(PathBuf::from(program_files).join("nodejs"));
            }
        }

        // Scoop is another common way to install Node and global npm tools for
        // a Windows user, and both paths are harmless when Scoop is absent.
        dirs.push(home.join("scoop/shims"));
        dirs.push(home.join("scoop/apps/nodejs/current"));
    }

    dirs
}

/// The common install directories, checked directly so a bundle launch never
/// pays for a shell spawn. Not exhaustive by design — on Unix,
/// [`login_shell_which`] is the general answer and this is the fast path.
fn search_known_dirs(bin: &str) -> Option<PathBuf> {
    let home = dirs::home_dir()?;
    let candidates = known_dirs();

    if let Some(found) = candidates.iter().find_map(|dir| {
        executable_candidates(dir, bin)
            .into_iter()
            .find(|candidate| is_executable(candidate))
    }) {
        return Some(found);
    }

    // nvm keeps one bin directory per installed Node version, so the path
    // depends on which version is current — glob the versions rather than
    // guessing one. `known_dirs` includes these on Unix, but retaining this
    // fallback keeps this lookup correct if that list changes later.
    #[cfg(not(windows))]
    {
        let versions = std::fs::read_dir(home.join(".nvm/versions/node")).ok()?;
        return versions.flatten().find_map(|entry| {
            let dir = entry.path().join("bin");
            executable_candidates(&dir, bin)
                .into_iter()
                .find(|candidate| is_executable(candidate))
        });
    }

    #[cfg(windows)]
    {
        let _ = home;
        None
    }
}

/// Returns candidate paths for a command name.
///
/// Windows npm installs an extensionless shell script, a `.cmd` launcher, and
/// often a `.ps1` launcher together. The resolver returns a `.cmd` when that
/// is the usable launcher; [`agent_command`] turns npm's shim into a direct Node
/// invocation before spawning it.
fn executable_candidates(dir: &Path, bin: &str) -> Vec<PathBuf> {
    #[cfg(windows)]
    {
        [".exe", ".cmd", ".bat", ".com"]
            .into_iter()
            .map(|extension| dir.join(format!("{bin}{extension}")))
            .collect()
    }

    #[cfg(not(windows))]
    {
        vec![dir.join(bin)]
    }
}

/// Asks the user's login shell where `bin` is, which is the only way to see a
/// `PATH` built by rc files the app never sourced.
///
/// `-l` matters more than it looks: without it zsh reads `.zshrc` only, and a
/// `PATH` exported from `.zprofile` — where the installers write it — stays
/// invisible. Windows never compiles this path; Explorer has no POSIX shell to
/// probe, and the native lookup above is enough.
#[cfg(not(windows))]
async fn login_shell_which(bin: &str) -> Option<PathBuf> {
    let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".to_string());

    let output = Command::new(shell)
        .args(["-l", "-c", &format!("command -v {bin}")])
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .await
        .ok()?;

    if !output.status.success() {
        return None;
    }

    let line = String::from_utf8(output.stdout).ok()?;
    // `command -v` prints the name unchanged for a shell builtin or function,
    // which is not something we can spawn.
    let path = PathBuf::from(line.trim());
    is_executable(&path).then_some(path)
}

#[cfg(unix)]
fn is_executable(path: &std::path::Path) -> bool {
    use std::os::unix::fs::PermissionsExt;

    std::fs::metadata(path)
        .map(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
        .unwrap_or(false)
}

#[cfg(windows)]
fn is_executable(path: &std::path::Path) -> bool {
    path.is_file()
        && path
            .extension()
            .and_then(|extension| extension.to_str())
            .is_some_and(|extension| {
                matches!(
                    extension.to_ascii_lowercase().as_str(),
                    "exe" | "cmd" | "bat" | "com"
                )
            })
}

#[cfg(all(not(unix), not(windows)))]
fn is_executable(path: &std::path::Path) -> bool {
    path.is_file()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The CLI resolver also supports workspace tools such as Git.
    #[tokio::test]
    async fn finds_the_git_binary() {
        let Some(found) = resolve("git").await else {
            eprintln!("git not installed; skipping");
            return;
        };

        assert!(found.is_absolute(), "got a bare name: {found:?}");
        assert!(is_executable(&found));
    }

    /// A name that exists nowhere must resolve to nothing rather than to a
    /// path that fails only at spawn time.
    #[tokio::test]
    async fn a_missing_binary_resolves_to_none() {
        assert!(resolve("dray-definitely-not-a-real-binary").await.is_none());
    }

    #[test]
    fn a_directory_is_not_executable() {
        assert!(!is_executable(&PathBuf::from("/usr")));
    }
}
