//! Processes on the host. In the Flatpak everything external runs through
//! `flatpak-spawn --host` (see `anvil_core::host`); outside it directly.

use std::path::{Path, PathBuf};
use std::process::Stdio;

use anvil_core::host::{self, Command};
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::sync::mpsc;

pub fn in_flatpak() -> bool {
    Path::new("/.flatpak-info").exists()
}

pub fn home() -> PathBuf {
    gtk::glib::home_dir()
}

/// A Tokio command for `cmd`, wrapped for the host. Must be spawned on
/// the I/O runtime.
pub fn tokio_command(cmd: &Command) -> tokio::process::Command {
    let argv = host::wrap(cmd, in_flatpak());
    let mut c = tokio::process::Command::new(&argv[0]);
    c.args(&argv[1..]);
    if !in_flatpak() {
        if let Some(d) = &cmd.cwd {
            c.current_dir(d);
        }
        for (k, v) in &cmd.env {
            c.env(k, v);
        }
    }
    c.kill_on_drop(true);
    c
}

#[derive(Clone, Debug)]
pub struct Output {
    pub status: Option<i32>,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
}

impl Output {
    pub fn ok(&self) -> bool {
        self.status == Some(0)
    }

    pub fn stdout_text(&self) -> String {
        String::from_utf8_lossy(&self.stdout).into_owned()
    }

    pub fn stderr_text(&self) -> String {
        String::from_utf8_lossy(&self.stderr).into_owned()
    }
}

/// Runs to completion, optionally feeding `stdin`, and collects output.
pub async fn run(cmd: Command, stdin: Option<Vec<u8>>) -> std::io::Result<Output> {
    crate::runtime::run(async move {
        let mut c = tokio_command(&cmd);
        c.stdout(Stdio::piped()).stderr(Stdio::piped()).stdin(if stdin.is_some() { Stdio::piped() } else { Stdio::null() });
        let mut child = c.spawn()?;
        if let (Some(data), Some(mut sin)) = (stdin, child.stdin.take()) {
            use tokio::io::AsyncWriteExt;
            sin.write_all(&data).await?;
            drop(sin);
        }
        let out = child.wait_with_output().await?;
        Ok(Output { status: out.status.code(), stdout: out.stdout, stderr: out.stderr })
    })
    .await
}

pub enum Line {
    Out(String),
    Err(String),
    /// The process ended (exit code, `None` when killed or not started).
    Done(Option<i32>),
}

/// A running process whose lines arrive on a channel the UI awaits.
pub struct Streaming {
    pub lines: mpsc::UnboundedReceiver<Line>,
    kill: Option<tokio::sync::oneshot::Sender<()>>,
}

impl Streaming {
    pub fn kill(&mut self) {
        if let Some(k) = self.kill.take() {
            let _ = k.send(());
        }
    }
}

pub fn stream(cmd: Command) -> Streaming {
    let (tx, rx) = mpsc::unbounded_channel();
    let (kill_tx, kill_rx) = tokio::sync::oneshot::channel::<()>();
    crate::runtime::handle().spawn(async move {
        let mut c = tokio_command(&cmd);
        c.stdout(Stdio::piped()).stderr(Stdio::piped()).stdin(Stdio::null());
        let mut child = match c.spawn() {
            Ok(ch) => ch,
            Err(e) => {
                let _ = tx.send(Line::Err(format!("{}: {e}", cmd.program)));
                let _ = tx.send(Line::Done(None));
                return;
            }
        };
        let out = child.stdout.take().map(|o| BufReader::new(o).lines());
        let err = child.stderr.take().map(|e| BufReader::new(e).lines());
        let tx_out = tx.clone();
        let t1 = tokio::spawn(async move {
            if let Some(mut l) = out {
                while let Ok(Some(line)) = l.next_line().await {
                    let _ = tx_out.send(Line::Out(line));
                }
            }
        });
        let tx_err = tx.clone();
        let t2 = tokio::spawn(async move {
            if let Some(mut l) = err {
                while let Ok(Some(line)) = l.next_line().await {
                    let _ = tx_err.send(Line::Err(line));
                }
            }
        });
        let status = tokio::select! {
            s = child.wait() => s.ok().and_then(|s| s.code()),
            _ = kill_rx => {
                let _ = child.kill().await;
                None
            }
        };
        let _ = t1.await;
        let _ = t2.await;
        let _ = tx.send(Line::Done(status));
    });
    Streaming { lines: rx, kill: Some(kill_tx) }
}

/// Whether a program exists on the host (`dotnet`, `git`).
pub async fn has(program: &str) -> bool {
    run(Command::new("sh").args(["-c", &format!("command -v {}", host::sh_quote(program))]), None).await.is_ok_and(|o| o.ok())
}
