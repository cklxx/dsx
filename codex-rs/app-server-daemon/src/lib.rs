mod backend;
mod client;
mod managed_install;

use std::path::Path;
use std::path::PathBuf;
use std::time::Duration;

use anyhow::Context;
use anyhow::Result;
use anyhow::anyhow;
pub use backend::BackendKind;
use backend::BackendPaths;
use codex_app_server_transport::app_server_control_socket_path;
use codex_utils_home_dir::find_codex_home;
use managed_install::managed_codex_bin;
#[cfg(unix)]
use managed_install::managed_codex_version;
use serde::Serialize;
use tokio::time::sleep;

const START_POLL_INTERVAL: Duration = Duration::from_millis(50);
const START_TIMEOUT: Duration = Duration::from_secs(10);
const OPERATION_LOCK_TIMEOUT: Duration = Duration::from_secs(75);
const PID_FILE_NAME: &str = "app-server.pid";
const OPERATION_LOCK_FILE_NAME: &str = "daemon.lock";
const STATE_DIR_NAME: &str = "app-server-daemon";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LifecycleCommand {
    Start,
    Restart,
    Stop,
    Version,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum LifecycleStatus {
    AlreadyRunning,
    Started,
    Restarted,
    Stopped,
    NotRunning,
    Running,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LifecycleOutput {
    pub status: LifecycleStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub backend: Option<BackendKind>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pid: Option<u32>,
    pub managed_codex_path: PathBuf,
    pub managed_codex_version: Option<String>,
    pub socket_path: PathBuf,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cli_version: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub app_server_version: Option<String>,
}

pub async fn probe_app_server_version(socket_path: &Path) -> Result<String> {
    Ok(client::probe(socket_path).await?.app_server_version)
}

pub async fn run(command: LifecycleCommand) -> Result<LifecycleOutput> {
    ensure_supported_platform()?;
    Daemon::from_environment()?.run(command).await
}

#[cfg(unix)]
fn ensure_supported_platform() -> Result<()> {
    Ok(())
}

#[cfg(not(unix))]
fn ensure_supported_platform() -> Result<()> {
    Err(anyhow!(
        "codex app-server daemon lifecycle is only supported on Unix platforms"
    ))
}

struct Daemon {
    socket_path: PathBuf,
    pid_file: PathBuf,
    operation_lock_file: PathBuf,
    managed_codex_bin: PathBuf,
}

impl Daemon {
    fn from_environment() -> Result<Self> {
        let codex_home = find_codex_home().context("failed to resolve DSX_HOME")?;
        let socket_path = app_server_control_socket_path(codex_home.as_path())?
            .as_path()
            .to_path_buf();
        let state_dir = codex_home.as_path().join(STATE_DIR_NAME);
        Ok(Self {
            socket_path,
            pid_file: state_dir.join(PID_FILE_NAME),
            operation_lock_file: state_dir.join(OPERATION_LOCK_FILE_NAME),
            managed_codex_bin: managed_codex_bin(codex_home.as_path()),
        })
    }

    async fn run(&self, command: LifecycleCommand) -> Result<LifecycleOutput> {
        match command {
            LifecycleCommand::Start => {
                let _operation_lock = self.acquire_operation_lock().await?;
                self.start().await
            }
            LifecycleCommand::Restart => {
                let _operation_lock = self.acquire_operation_lock().await?;
                self.restart().await
            }
            LifecycleCommand::Stop => {
                let _operation_lock = self.acquire_operation_lock().await?;
                self.stop().await
            }
            LifecycleCommand::Version => self.version().await,
        }
    }

    async fn start(&self) -> Result<LifecycleOutput> {
        if let Ok(info) = client::probe(&self.socket_path).await {
            return Ok(self
                .output(
                    LifecycleStatus::AlreadyRunning,
                    self.running_backend().await?,
                    /*pid*/ None,
                    Some(info.app_server_version),
                )
                .await);
        }

        if self.running_backend_instance().await?.is_some() {
            let info = self.wait_until_ready().await?;
            return Ok(self
                .output(
                    LifecycleStatus::AlreadyRunning,
                    Some(BackendKind::Pid),
                    /*pid*/ None,
                    Some(info.app_server_version),
                )
                .await);
        }

        self.ensure_managed_codex_bin()?;
        let pid = self.start_managed_backend().await?;
        let info = self.wait_until_ready().await?;
        Ok(self
            .output(
                LifecycleStatus::Started,
                Some(BackendKind::Pid),
                pid,
                Some(info.app_server_version),
            )
            .await)
    }

    async fn restart(&self) -> Result<LifecycleOutput> {
        if client::probe(&self.socket_path).await.is_ok() && self.running_backend().await?.is_none()
        {
            return Err(anyhow!(
                "app server is running but is not managed by codex app-server daemon"
            ));
        }

        self.ensure_managed_codex_bin()?;
        if let Some(backend) = self.running_backend_instance().await? {
            backend.stop().await?;
        }

        let pid = self.start_managed_backend().await?;
        let info = self.wait_until_ready().await?;
        Ok(self
            .output(
                LifecycleStatus::Restarted,
                Some(BackendKind::Pid),
                pid,
                Some(info.app_server_version),
            )
            .await)
    }

    async fn stop(&self) -> Result<LifecycleOutput> {
        if let Some(backend) = self.running_backend_instance().await? {
            backend.stop().await?;
            return Ok(self
                .output(
                    LifecycleStatus::Stopped,
                    Some(BackendKind::Pid),
                    /*pid*/ None,
                    /*app_server_version*/ None,
                )
                .await);
        }

        if client::probe(&self.socket_path).await.is_ok() {
            return Err(anyhow!(
                "app server is running but is not managed by codex app-server daemon"
            ));
        }

        Ok(self
            .output(
                LifecycleStatus::NotRunning,
                /*backend*/ None,
                /*pid*/ None,
                /*app_server_version*/ None,
            )
            .await)
    }

    async fn version(&self) -> Result<LifecycleOutput> {
        let info = client::probe(&self.socket_path).await?;
        Ok(self
            .output(
                LifecycleStatus::Running,
                self.running_backend().await?,
                /*pid*/ None,
                Some(info.app_server_version),
            )
            .await)
    }

    async fn wait_until_ready(&self) -> Result<client::ProbeInfo> {
        let deadline = tokio::time::Instant::now() + START_TIMEOUT;
        loop {
            match client::probe(&self.socket_path).await {
                Ok(info) => return Ok(info),
                Err(err) if tokio::time::Instant::now() < deadline => {
                    let _ = err;
                    sleep(START_POLL_INTERVAL).await;
                }
                Err(err) => {
                    let context = self.app_server_not_ready_context().await;
                    return Err(err).context(context);
                }
            }
        }
    }

    async fn app_server_not_ready_context(&self) -> String {
        let mut context = format!(
            "app server did not become ready on {}",
            self.socket_path.display()
        );
        self.append_daemon_app_server_context(&mut context).await;
        backend::append_stderr_log_tail_context(&self.pid_file, &mut context).await;
        context
    }

    async fn append_daemon_app_server_context(&self, context: &mut String) {
        let managed_codex_version = self
            .managed_codex_version_best_effort()
            .await
            .unwrap_or_else(|| "unknown".to_string());
        context.push_str(&format!(
            "\n\nDaemon used app-server:\n  path: {}\n  version: {managed_codex_version}",
            self.managed_codex_bin.display()
        ));
    }

    async fn running_backend(&self) -> Result<Option<BackendKind>> {
        Ok(self
            .running_backend_instance()
            .await?
            .map(|_| BackendKind::Pid))
    }

    async fn running_backend_instance(&self) -> Result<Option<backend::PidBackend>> {
        let backend = backend::pid_backend(self.backend_paths());
        if backend.is_starting_or_running().await? {
            return Ok(Some(backend));
        }
        Ok(None)
    }

    async fn start_managed_backend(&self) -> Result<Option<u32>> {
        self.start_managed_backend_with_bin(&self.managed_codex_bin)
            .await
    }

    async fn start_managed_backend_with_bin(
        &self,
        managed_codex_bin: &Path,
    ) -> Result<Option<u32>> {
        let backend = backend::pid_backend(self.backend_paths_with_bin(managed_codex_bin));
        backend.start().await
    }

    fn ensure_managed_codex_bin(&self) -> Result<()> {
        if self.managed_codex_bin.is_file() {
            return Ok(());
        }

        let managed_codex_path = self.managed_codex_bin.display();
        Err(anyhow!(
            "managed DSX executable not found at {managed_codex_path}\n\n\
             Install DSX at that path, then rerun the command."
        ))
    }

    #[cfg(unix)]
    async fn managed_codex_version_best_effort(&self) -> Option<String> {
        managed_codex_version(&self.managed_codex_bin).await.ok()
    }

    #[cfg(not(unix))]
    async fn managed_codex_version_best_effort(&self) -> Option<String> {
        None
    }

    fn backend_paths(&self) -> BackendPaths {
        self.backend_paths_with_bin(&self.managed_codex_bin)
    }

    fn backend_paths_with_bin(&self, managed_codex_bin: &Path) -> BackendPaths {
        BackendPaths {
            codex_bin: managed_codex_bin.to_path_buf(),
            pid_file: self.pid_file.clone(),
        }
    }

    async fn acquire_operation_lock(&self) -> Result<tokio::fs::File> {
        let operation_lock = self.open_operation_lock_file().await?;
        let deadline = tokio::time::Instant::now() + OPERATION_LOCK_TIMEOUT;
        while !try_lock_file(&operation_lock)? {
            if tokio::time::Instant::now() >= deadline {
                return Err(anyhow!(
                    "timed out waiting for daemon operation lock {}",
                    self.operation_lock_file.display()
                ));
            }
            sleep(START_POLL_INTERVAL).await;
        }
        Ok(operation_lock)
    }

    async fn open_operation_lock_file(&self) -> Result<tokio::fs::File> {
        if let Some(parent) = self.operation_lock_file.parent() {
            tokio::fs::create_dir_all(parent).await.with_context(|| {
                format!(
                    "failed to create daemon state directory {}",
                    parent.display()
                )
            })?;
        }
        tokio::fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(&self.operation_lock_file)
            .await
            .with_context(|| {
                format!(
                    "failed to open daemon operation lock {}",
                    self.operation_lock_file.display()
                )
            })
    }

    async fn output(
        &self,
        status: LifecycleStatus,
        backend: Option<BackendKind>,
        pid: Option<u32>,
        app_server_version: Option<String>,
    ) -> LifecycleOutput {
        let managed_codex_version = self.managed_codex_version_best_effort().await;
        LifecycleOutput {
            status,
            backend,
            pid,
            managed_codex_path: self.managed_codex_bin.clone(),
            managed_codex_version,
            socket_path: self.socket_path.clone(),
            cli_version: Some(env!("CARGO_PKG_VERSION").to_string()),
            app_server_version,
        }
    }
}

#[cfg(unix)]
fn try_lock_file(file: &tokio::fs::File) -> Result<bool> {
    use std::os::fd::AsRawFd;

    let result = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
    if result == 0 {
        return Ok(true);
    }

    let err = std::io::Error::last_os_error();
    if err.raw_os_error() == Some(libc::EWOULDBLOCK) {
        return Ok(false);
    }
    Err(err).context("failed to lock daemon operation")
}

#[cfg(not(unix))]
fn try_lock_file(_file: &tokio::fs::File) -> Result<bool> {
    Ok(true)
}
