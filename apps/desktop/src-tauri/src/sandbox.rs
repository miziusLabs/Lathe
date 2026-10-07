//! Docker sandbox integration for Cloud sessions.
//!
//! A Cloud session gets a disposable container and a private named volume. The
//! volume keeps Lathe's conversation and workspace between turns, but the host
//! project is never mounted into it. Lathe configuration is copied from a
//! read-only seed mount by the image entrypoint, while GitHub credentials stay
//! in the container environment for the lifetime of the container only.

use anyhow::{bail, Context, Result};
use serde::Serialize;
use std::process::Stdio;
use tokio::process::Command;

const CLOUD_WORKSPACE: &str = "/home/agent/workspace";
const DEFAULT_IMAGE: &str = "lathe-cloud:latest";
const VOLUME_PREFIX: &str = "dray-cloud-";
const CONTAINER_PREFIX: &str = "dray-cloud-";

/// The configured Cloud image. An override is useful for development and for
/// hosts that publish the image to a private registry.
pub fn image() -> String {
    std::env::var("DRAY_CLOUD_IMAGE")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| DEFAULT_IMAGE.to_string())
}

pub fn volume_name(cloud_name: &str) -> String {
    format!("{VOLUME_PREFIX}{cloud_name}")
}

pub fn container_name(session_id: &str) -> String {
    // Session ids are UUIDs minted by Lathe, but keep this constrained because
    // this name is handed directly to Docker as an argument.
    let safe: String = session_id
        .chars()
        .filter(|character| {
            character.is_ascii_alphanumeric() || *character == '-' || *character == '_'
        })
        .collect();
    format!("{CONTAINER_PREFIX}{safe}")
}

/// Checks whether the Docker CLI can reach a running daemon. Cloud creation
/// performs the more specific image check later; this is only the lightweight
/// capability query used to enable the composer's Cloud control.
pub async fn is_available() -> bool {
    let mut command = docker_command();
    command
        .args(["info", "--format", "{{.ServerVersion}}"])
        .stdout(Stdio::null())
        .stderr(Stdio::null());

    command_succeeded(command).await
}

async fn command_succeeded(mut command: Command) -> bool {
    command.status().await.is_ok_and(|status| status.success())
}

#[derive(Debug, Clone, Serialize)]
pub struct CloudAvailability {
    pub available: bool,
    pub reason: Option<String>,
}

/// Checks the prerequisites shown by the New Task Cloud control. Keep the
/// daemon and image checks separate so the UI can tell the user what to fix.
pub async fn availability() -> CloudAvailability {
    if !is_available().await {
        return CloudAvailability {
            available: false,
            reason: Some("Docker is not installed or is not running.".to_string()),
        };
    }

    if ensure_image().await.is_err() {
        return CloudAvailability {
            available: false,
            reason: Some(format!(
                "Cloud image {:?} is not available. Build it with `pnpm build:sandbox`.",
                image()
            )),
        };
    }

    CloudAvailability {
        available: true,
        reason: None,
    }
}

/// Verifies that the requested image exists before a session is indexed. A
/// missing image otherwise looks like a successful `docker run` until the
/// first stderr line arrives, leaving a session that cannot be recovered.
pub async fn ensure_image() -> Result<()> {
    let image = image();
    let mut command = docker_command();
    let output = command
        .args(["image", "inspect", &image])
        .output()
        .await
        .context("couldn't run Docker")?;

    if output.status.success() {
        return Ok(());
    }

    let detail = String::from_utf8_lossy(&output.stderr).trim().to_string();
    bail!(
        "Cloud image {image:?} is not available. Build it with `pnpm build:sandbox`.{}",
        if detail.is_empty() {
            String::new()
        } else {
            format!(" Docker: {detail}")
        }
    )
}

/// Builds a Docker command that runs Lathe through the image entrypoint.
///
/// The session manager verifies the image before indexing a new or resumed
/// Cloud. Do not inspect it again here: every inspection launches another
/// Docker client, which makes Windows session startup noticeably slower.
///
/// `GITHUB_TOKEN` is deliberately passed as an environment name rather than a
/// `-e KEY=value` argument. Docker reads its value from this process's
/// environment, so the secret does not appear in the command-line argument
/// visible to local process inspectors. The entrypoint mirrors Agentsmith by
/// exporting it as `GH_TOKEN` and running `gh auth setup-git`.
pub async fn agent_command(
    session_id: &str,
    cloud_name: &str,
    agent_args: &[String],
) -> Result<Command> {
    let home = dirs::home_dir().context("could not resolve home directory")?;
    lathe_agent::skills::install_bundled()
        .context("could not install bundled skills in ~/.mizius/skills")?;
    let skills = home.join(".mizius").join("skills");
    let volume = volume_name(cloud_name);
    let container = container_name(session_id);

    let mut command = docker_command();
    command.args([
        "run",
        "--rm",
        "--init",
        "--interactive",
        "--cap-drop=ALL",
        "--security-opt=no-new-privileges",
        "--name",
        &container,
        "--workdir",
        CLOUD_WORKSPACE,
        "--mount",
        &format!("type=volume,source={volume},target=/home/agent"),
    ]);

    // Skills are shared read-only; OpenAI credentials are delivered over stdin.
    if skills.is_dir() {
        let source = skills
            .to_str()
            .context("Skills directory is not valid UTF-8")?;
        command.args([
            "--mount",
            &format!("type=bind,source={source},target=/home/agent/.mizius/skills,readonly"),
        ]);
    }

    if let Some(token) = github_token().await {
        // Docker receives only the variable name. The value is inherited from
        // this command's environment rather than exposed in its argv.
        command.env("GITHUB_TOKEN", token);
        command.arg("--env").arg("GITHUB_TOKEN");
    }

    command.arg(image()).arg("lathe-agent").args(agent_args);
    Ok(command)
}

/// Returns the host's GitHub token without writing it anywhere. Agentsmith uses
/// a configured token when available; Lathe also accepts the standard local
/// `gh auth token` store so existing Lathe GitHub features work unchanged.
async fn github_token() -> Option<String> {
    for key in ["GITHUB_TOKEN", "GH_TOKEN"] {
        if let Ok(value) = std::env::var(key) {
            if !value.trim().is_empty() {
                return Some(value);
            }
        }
    }

    let gh = crate::binpath::gh().await?;
    let mut command = Command::new(gh);
    crate::binpath::configure_command(&mut command);
    command
        .args(["auth", "token"])
        .stdin(Stdio::null())
        .stderr(Stdio::null());
    let output = command.output().await.ok()?;
    if !output.status.success() {
        return None;
    }

    let token = String::from_utf8_lossy(&output.stdout).trim().to_string();
    (!token.is_empty()).then_some(token)
}

/// Stops and removes a Cloud container. `docker run --rm` removes it after a
/// normal exit, but killing the Docker CLI directly can leave the container
/// behind, so cleanup is explicit on every session stop.
pub async fn remove_container(session_id: &str) {
    let name = container_name(session_id);
    let mut command = docker_command();
    let _ = command.args(["rm", "--force", &name]).output().await;
}

/// Deletes only the named volume for a Cloud session. The workspace is not a
/// host path and no Git repository can be affected by this operation.
pub async fn remove_volume(cloud_name: &str) {
    let volume = volume_name(cloud_name);
    let mut command = docker_command();
    let _ = command
        .args(["volume", "rm", "--force", &volume])
        .output()
        .await;
}

fn docker_command() -> Command {
    let mut command = Command::new("docker");
    crate::binpath::configure_command(&mut command);
    command.env("PATH", crate::binpath::agent_path());
    command
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cloud_resources_are_scoped_by_the_cloud_or_session_id() {
        assert_eq!(volume_name("abc"), "dray-cloud-abc");
        assert_eq!(container_name("abc"), "dray-cloud-abc");
        assert_eq!(container_name("a/b c"), "dray-cloud-abc");
    }

    #[tokio::test]
    async fn availability_requires_a_successful_command() {
        #[cfg(windows)]
        fn exit_command(code: u8) -> Command {
            let mut command = Command::new("cmd");
            command.args(["/C", &format!("exit {code}")]);
            command
        }

        #[cfg(not(windows))]
        fn exit_command(code: u8) -> Command {
            Command::new(if code == 0 { "true" } else { "false" })
        }

        assert!(command_succeeded(exit_command(0)).await);
        assert!(!command_succeeded(exit_command(1)).await);
        assert!(!command_succeeded(Command::new("dray-no-such-command")).await);
    }

    #[test]
    fn image_override_uses_the_environment() {
        // This test is intentionally not run in parallel with another env
        // reader in this module: it restores the process-global value before
        // returning so the rest of the test suite sees the caller's setting.
        let previous = std::env::var_os("DRAY_CLOUD_IMAGE");
        std::env::set_var("DRAY_CLOUD_IMAGE", "registry.example/dray:test");
        assert_eq!(image(), "registry.example/dray:test");
        match previous {
            Some(value) => std::env::set_var("DRAY_CLOUD_IMAGE", value),
            None => std::env::remove_var("DRAY_CLOUD_IMAGE"),
        }
    }
}
