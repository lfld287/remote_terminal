use std::io::{Read, Write};
use std::sync::Arc;

use anyhow::{Context, Result};
use portable_pty::{native_pty_system, CommandBuilder, MasterPty, PtySize};
use tokio::sync::{broadcast, Mutex};

struct PtySession {
    master: Box<dyn MasterPty + Send>,
    writer: Box<dyn Write + Send>,
    #[allow(dead_code)]
    output_tx: broadcast::Sender<Vec<u8>>,
    id: String,
}

pub struct PtyManager {
    sessions: Arc<Mutex<Vec<Arc<Mutex<PtySession>>>>>,
}

impl PtyManager {
    pub fn new() -> Self {
        Self {
            sessions: Arc::new(Mutex::new(Vec::new())),
        }
    }

    pub async fn create_session(
        &self,
        cols: u16,
        rows: u16,
        shell: Option<String>,
    ) -> Result<(String, broadcast::Sender<Vec<u8>>)> {
        let pty_system = native_pty_system();

        let pair = pty_system
            .openpty(PtySize {
                rows,
                cols,
                pixel_width: 0,
                pixel_height: 0,
            })
            .context("Failed to open PTY")?;

        let shell_cmd = shell.unwrap_or_else(default_shell);
        let cmd = CommandBuilder::new(shell_cmd);

        let _child = pair
            .slave
            .spawn_command(cmd)
            .context("Failed to spawn shell")?;

        let reader = pair
            .master
            .try_clone_reader()
            .context("Failed to clone PTY reader")?;

        let writer = pair
            .master
            .take_writer()
            .context("Failed to get PTY writer")?;

        let id = uuid::Uuid::new_v4().to_string();
        let (output_tx, _) = broadcast::channel(4096);

        let session = PtySession {
            master: pair.master,
            writer,
            output_tx: output_tx.clone(),
            id: id.clone(),
        };

        let session = Arc::new(Mutex::new(session));

        {
            let mut sessions = self.sessions.lock().await;
            sessions.push(session.clone());
        }

        spawn_reader(reader, output_tx.clone());

        Ok((id, output_tx))
    }

    pub async fn write_to_session(&self, id: &str, data: &[u8]) -> Result<()> {
        let sessions = self.sessions.lock().await;
        for session in sessions.iter() {
            let mut s = session.lock().await;
            if s.id == id {
                s.writer.write_all(data)?;
                s.writer.flush()?;
                return Ok(());
            }
        }
        anyhow::bail!("Session {} not found", id)
    }

    pub async fn resize_session(&self, id: &str, cols: u16, rows: u16) -> Result<()> {
        let sessions = self.sessions.lock().await;
        for session in sessions.iter() {
            let s = session.lock().await;
            if s.id == id {
                s.master
                    .resize(PtySize {
                        rows,
                        cols,
                        pixel_width: 0,
                        pixel_height: 0,
                    })
                    .context("Failed to resize PTY")?;
                return Ok(());
            }
        }
        anyhow::bail!("Session {} not found", id)
    }

    pub async fn kill_session(&self, id: &str) -> Result<()> {
        let mut sessions = self.sessions.lock().await;
        let mut found_idx = None;
        for (i, session) in sessions.iter().enumerate() {
            let s = session.lock().await;
            if s.id == id {
                found_idx = Some(i);
                break;
            }
        }
        if let Some(i) = found_idx {
            sessions.remove(i);
            Ok(())
        } else {
            anyhow::bail!("Session {} not found", id)
        }
    }
}

fn spawn_reader(mut reader: Box<dyn Read + Send>, tx: broadcast::Sender<Vec<u8>>) {
    std::thread::spawn(move || {
        let mut buf = [0u8; 8192];
        loop {
            match reader.read(&mut buf) {
                Ok(0) => {
                    let _ = tx.send(b"[Process exited]\r\n".to_vec());
                    break;
                }
                Ok(n) => {
                    let _ = tx.send(buf[..n].to_vec());
                }
                Err(_) => break,
            }
        }
    });
}

fn default_shell() -> String {
    if cfg!(target_os = "windows") {
        "cmd.exe".to_string()
    } else {
        std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".to_string())
    }
}
