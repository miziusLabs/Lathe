//! Local, repository-owned workspaces. Creation updates only the new checkout.
use anyhow::{bail, Context, Result};
use std::path::{Path, PathBuf};
use tokio::process::Command;

async fn git(cwd: &Path, args: &[&str]) -> Result<String> {
    let mut command = Command::new("git");
    crate::binpath::configure_command(&mut command);
    let output = command
        .current_dir(cwd)
        .args(args)
        .env("GIT_TERMINAL_PROMPT", "0")
        .output()
        .await?;
    if !output.status.success() {
        bail!(
            "git {} failed: {}",
            args.first().unwrap_or(&""),
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

pub fn path(project: &str, name: &str) -> String {
    Path::new(project)
        .join(".lathe/worktrees")
        .join(name)
        .to_string_lossy()
        .into_owned()
}

/// Fetch first, then fast-forward the new branch from its source's upstream.
/// A failed pull never changes the source checkout or leaves a partial workspace.
pub async fn create(project: &str, name: &str, base: Option<&str>) -> Result<(String, String)> {
    uuid::Uuid::parse_str(name).context("invalid worktree ID")?;
    let project = Path::new(project);
    let root = git(project, &["rev-parse", "--show-toplevel"])
        .await
        .context("Choose a Git project for the Worktree session.")?;
    let project = Path::new(&root);
    let base = match base {
        Some(base) => base.to_string(),
        None => git(project, &["symbolic-ref", "--short", "HEAD"])
            .await
            .context("Choose a source branch for the Worktree session.")?,
    };
    git(project, &["check-ref-format", "--branch", &base]).await?;
    // Resolve a local source ref explicitly, avoiding options and ambiguous revisions.
    let source = format!("refs/heads/{base}");
    git(project, &["rev-parse", "--verify", &source]).await?;
    let upstream = git(
        project,
        &["for-each-ref", "--format=%(upstream:short)", &source],
    )
    .await?;
    let remote = if upstream.is_empty() {
        git(
            project,
            &["config", "--get", &format!("branch.{base}.latheRemote")],
        )
        .await
        .unwrap_or_else(|_| "origin".to_string())
    } else {
        git(
            project,
            &["config", "--get", &format!("branch.{base}.remote")],
        )
        .await?
    };
    if remote == "." {
        bail!("The source branch needs a remote upstream to pull a Worktree.");
    }
    let merge = if upstream.is_empty() {
        git(
            project,
            &["config", "--get", &format!("branch.{base}.latheMerge")],
        )
        .await
        .unwrap_or_else(|_| source.clone())
    } else {
        git(
            project,
            &["config", "--get", &format!("branch.{base}.merge")],
        )
        .await?
    };
    git(project, &["fetch", "--prune", "--", &remote]).await?;

    let cwd = path(&root, name);
    let branch = format!("lathe/{name}");
    // Ignore nested checkouts for all worktrees without modifying tracked project files.
    let common = git(project, &["rev-parse", "--git-common-dir"]).await?;
    let common = PathBuf::from(common);
    let common = if common.is_absolute() {
        common
    } else {
        project.join(common)
    };
    let exclude = common.join("info/exclude");
    tokio::fs::create_dir_all(common.join("info")).await?;
    let mut content = match tokio::fs::read_to_string(&exclude).await {
        Ok(content) => content,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(error) => return Err(error.into()),
    };
    if !content.lines().any(|line| line == "/.lathe/worktrees/") {
        content.push_str("\n/.lathe/worktrees/\n");
        tokio::fs::write(&exclude, content).await?;
    }
    tokio::fs::create_dir_all(project.join(".lathe/worktrees")).await?;
    git(
        project,
        &[
            "worktree",
            "add",
            "--no-track",
            "-b",
            &branch,
            &cwd,
            &source,
        ],
    )
    .await?;
    if let Err(error) = git(
        Path::new(&cwd),
        &["pull", "--ff-only", "--", &remote, &merge],
    )
    .await
    {
        let _ = git(project, &["worktree", "remove", &cwd]).await;
        let _ = git(project, &["branch", "-D", &branch]).await;
        return Err(
            error.context("Could not update the Worktree from its remote; creation was cancelled.")
        );
    }
    // Remember the source for forks without making the new branch track (or push to) it.
    git(
        project,
        &["config", &format!("branch.{branch}.latheRemote"), &remote],
    )
    .await?;
    git(
        project,
        &["config", &format!("branch.{branch}.latheMerge"), &merge],
    )
    .await?;
    Ok((cwd, branch))
}

/// Never force removal: uncommitted work must survive session deletion.
pub async fn remove(cwd: &str) -> Result<()> {
    let cwd = Path::new(cwd);
    if !cwd.exists() {
        return Ok(());
    }
    // Run outside the checkout being deleted so Windows can release its directory.
    let common = PathBuf::from(git(cwd, &["rev-parse", "--git-common-dir"]).await?);
    let common = if common.is_absolute() {
        common
    } else {
        cwd.join(common)
    };
    git(
        &common,
        &[
            "worktree",
            "remove",
            cwd.to_str().context("invalid worktree path")?,
        ],
    )
    .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    struct TempDir(PathBuf);
    impl TempDir {
        fn path(&self) -> &Path {
            &self.0
        }
    }
    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
    async fn fixture() -> (TempDir, PathBuf, PathBuf) {
        let temp = TempDir(
            std::env::temp_dir().join(format!("lathe-worktree-test-{}", uuid::Uuid::new_v4())),
        );
        tokio::fs::create_dir_all(temp.path()).await.unwrap();
        let remote = temp.path().join("remote.git");
        let source = temp.path().join("project");
        git(temp.path(), &["init", "--bare", remote.to_str().unwrap()])
            .await
            .unwrap();
        git(
            temp.path(),
            &["init", "-b", "main", source.to_str().unwrap()],
        )
        .await
        .unwrap();
        git(&source, &["config", "user.name", "Test"])
            .await
            .unwrap();
        git(&source, &["config", "user.email", "test@example.com"])
            .await
            .unwrap();
        tokio::fs::write(source.join("file.txt"), "initial")
            .await
            .unwrap();
        git(&source, &["add", "."]).await.unwrap();
        git(&source, &["commit", "-m", "Initial"]).await.unwrap();
        git(
            &source,
            &["remote", "add", "origin", remote.to_str().unwrap()],
        )
        .await
        .unwrap();
        git(&source, &["push", "-u", "origin", "main"])
            .await
            .unwrap();
        (temp, source.canonicalize().unwrap(), remote)
    }

    #[tokio::test]
    async fn pulls_latest_without_changing_dirty_source_and_forks_locally() {
        let (temp, source, remote) = fixture().await;
        let writer = temp.path().join("writer");
        git(
            temp.path(),
            &[
                "clone",
                "-b",
                "main",
                remote.to_str().unwrap(),
                writer.to_str().unwrap(),
            ],
        )
        .await
        .unwrap();
        git(&writer, &["config", "user.name", "Test"])
            .await
            .unwrap();
        git(&writer, &["config", "user.email", "test@example.com"])
            .await
            .unwrap();
        tokio::fs::write(writer.join("file.txt"), "remote update")
            .await
            .unwrap();
        git(&writer, &["commit", "-am", "Remote update"])
            .await
            .unwrap();
        git(&writer, &["push"]).await.unwrap();
        let before = git(&source, &["rev-parse", "HEAD"]).await.unwrap();
        tokio::fs::write(source.join("file.txt"), "unsaved work")
            .await
            .unwrap();
        let id = uuid::Uuid::new_v4().to_string();
        let (cwd, branch) = create(source.to_str().unwrap(), &id, Some("main"))
            .await
            .unwrap();
        assert_eq!(cwd, path(source.to_str().unwrap(), &id));
        assert_eq!(
            git(Path::new(&cwd), &["symbolic-ref", "--short", "HEAD"])
                .await
                .unwrap(),
            branch
        );
        assert_eq!(
            tokio::fs::read_to_string(Path::new(&cwd).join("file.txt"))
                .await
                .unwrap(),
            "remote update"
        );
        assert_eq!(git(&source, &["rev-parse", "HEAD"]).await.unwrap(), before);
        assert_eq!(
            tokio::fs::read_to_string(source.join("file.txt"))
                .await
                .unwrap(),
            "unsaved work"
        );
        assert!(!git(&source, &["status", "--porcelain"])
            .await
            .unwrap()
            .contains(".lathe"));
        assert!(git(Path::new(&cwd), &["rev-parse", "@{u}"]).await.is_err());
        let (fork, _) = create(
            source.to_str().unwrap(),
            &uuid::Uuid::new_v4().to_string(),
            Some(&branch),
        )
        .await
        .unwrap();
        remove(&fork).await.unwrap();
        tokio::fs::write(Path::new(&cwd).join("untracked.txt"), "keep")
            .await
            .unwrap();
        assert!(remove(&cwd).await.is_err());
        assert!(Path::new(&cwd).join("untracked.txt").exists());
        tokio::fs::remove_file(Path::new(&cwd).join("untracked.txt"))
            .await
            .unwrap();
        remove(&cwd).await.unwrap();
        assert!(!Path::new(&cwd).exists());
    }

    #[tokio::test]
    async fn failed_pull_cleans_up_checkout_and_branch() {
        let (_temp, source, _) = fixture().await;
        git(&source, &["checkout", "-b", "unpublished"])
            .await
            .unwrap();
        let id = uuid::Uuid::new_v4().to_string();
        assert!(create(source.to_str().unwrap(), &id, Some("unpublished"))
            .await
            .is_err());
        assert!(!Path::new(&path(source.to_str().unwrap(), &id)).exists());
        assert!(git(
            &source,
            &["rev-parse", "--verify", &format!("refs/heads/lathe/{id}")]
        )
        .await
        .is_err());
    }
}
