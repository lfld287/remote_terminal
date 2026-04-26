use std::collections::HashMap;
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
}

pub struct PtyManager {
    sessions: Arc<Mutex<HashMap<String, Arc<Mutex<PtySession>>>>>,
}

impl PtyManager {
    pub fn new() -> Self {
        Self {
            sessions: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    pub async fn get_or_create_session(
        &self,
        token: &str,
        cols: u16,
        rows: u16,
        shell: Option<String>,
    ) -> Result<(broadcast::Sender<Vec<u8>>, bool)> {
        let mut sessions = self.sessions.lock().await;

        if let Some(session) = sessions.get(token) {
            let s = session.lock().await;
            return Ok((s.output_tx.clone(), true));
        }

        let session = Self::create_pty_session(cols, rows, shell)?;
        let tx = {
            let s = session.lock().await;
            s.output_tx.clone()
        };
        sessions.insert(token.to_string(), session);

        Ok((tx, false))
    }

    fn create_pty_session(
        cols: u16,
        rows: u16,
        shell: Option<String>,
    ) -> Result<Arc<Mutex<PtySession>>> {
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

        let (output_tx, _) = broadcast::channel(4096);

        let session = PtySession {
            master: pair.master,
            writer,
            output_tx: output_tx.clone(),
        };

        spawn_reader(reader, output_tx.clone());

        Ok(Arc::new(Mutex::new(session)))
    }

    pub async fn write_to_session(&self, token: &str, data: &[u8]) -> Result<()> {
        let sessions = self.sessions.lock().await;
        let session = sessions
            .get(token)
            .context(format!("Session for token {} not found", &token[..4]))?;
        let mut s = session.lock().await;
        s.writer.write_all(data)?;
        s.writer.flush()?;
        Ok(())
    }

    pub async fn resize_session(&self, token: &str, cols: u16, rows: u16) -> Result<()> {
        let sessions = self.sessions.lock().await;
        let session = sessions
            .get(token)
            .context(format!("Session for token {} not found", &token[..4]))?;
        let s = session.lock().await;
        s.master
            .resize(PtySize {
                rows,
                cols,
                pixel_width: 0,
                pixel_height: 0,
            })
            .context("Failed to resize PTY")?;
        Ok(())
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
