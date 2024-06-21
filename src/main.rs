use anyhow::anyhow;
use async_trait::async_trait;
use ratatui::{
    backend::CrosstermBackend,
    layout::{Constraint, Layout, Rect},
    text::Line,
    widgets::{Block, Paragraph, Wrap},
    Terminal,
};
use russh::{server::*, Channel, ChannelId, MethodSet};
use russh_keys::key::PublicKey;
use std::{collections::HashMap, io::ErrorKind, ops::Neg, sync::Arc};
use strip_ansi_escapes::strip;
use tokio::sync::{Mutex, RwLock};

#[derive(Clone)]
struct Client {
    terminal: Terminal<CrosstermBackend<TerminalHandle>>,
    input: String,
    user: String,
    fingerprint: String,
    scroll: i32,
}

impl Client {
    fn render(&mut self, history: &[String]) -> std::io::Result<()> {
        self.terminal.draw(|frame| {
            let rects = Layout::vertical([Constraint::Percentage(90), Constraint::Fill(1)])
                .split(frame.size());
            let para = Paragraph::new(
                history
                    .iter()
                    .map(|s| Line::from(s.as_str()))
                    .collect::<Vec<_>>(),
            )
            .wrap(Wrap { trim: true });
            let line_count = para.line_count(rects[0].width);
            let mut scroll_offset = if line_count > rects[0].height as usize {
                (line_count - rects[0].height as usize + 4) as u16
            } else {
                0
            };

            self.scroll = self.scroll.clamp((scroll_offset as i32).neg(), 0);

            if self.scroll < 0 {
                scroll_offset = scroll_offset.saturating_sub(self.scroll.unsigned_abs() as u16);
            } else {
                scroll_offset += self.scroll.unsigned_abs() as u16;
            }

            frame.render_widget(
                para.scroll((scroll_offset, 0))
                    .block(Block::bordered().title("Chat History")),
                rects[0],
            );
            frame.render_widget(
                Paragraph::new(self.input.as_str())
                    .block(Block::bordered().title("Message Input"))
                    .wrap(Wrap { trim: true }),
                rects[1],
            );
        })?;
        Ok(())
    }

    fn new(
        user: String,
        fingerprint: String,
        history: &[String],
        handle: TerminalHandle,
    ) -> std::io::Result<Self> {
        let mut terminal = Terminal::new(CrosstermBackend::new(handle))?;
        terminal.clear()?;
        let mut client = Self {
            terminal,
            input: "".into(),
            user,
            fingerprint,
            scroll: 0,
        };
        client.render(history)?;
        Ok(client)
    }
}

#[derive(Clone)]
struct TerminalHandle {
    handle: Handle,
    sink: Vec<u8>,
    channel_id: ChannelId,
}

impl std::io::Write for TerminalHandle {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.sink.extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        let handle = self.handle.clone();
        let channel_id = self.channel_id;
        let data = self.sink.clone().into();
        futures::executor::block_on(async move {
            let result = handle.data(channel_id, data).await;
            if result.is_err() {
                eprintln!("Failed to send data: {:?}", result);
            }
        });

        self.sink.clear();
        Ok(())
    }
}

#[derive(Clone, Default)]
struct AppServer {
    clients: Arc<Mutex<HashMap<usize, Client>>>,
    history: Arc<RwLock<Vec<String>>>,
    keys: Arc<Mutex<HashMap<usize, (String, PublicKey)>>>,
    id: usize,
}

impl AppServer {
    pub fn new() -> Self {
        Self::default()
    }

    pub async fn run(&mut self) -> Result<(), anyhow::Error> {
        let config = Config {
            inactivity_timeout: Some(std::time::Duration::from_secs(3600)),
            auth_rejection_time: std::time::Duration::from_secs(3),
            auth_rejection_time_initial: Some(std::time::Duration::from_secs(0)),
            keys: vec![russh_keys::key::KeyPair::generate_ed25519().unwrap()],
            methods: MethodSet::PUBLICKEY,
            ..Default::default()
        };

        self.run_on_address(Arc::new(config), ("0.0.0.0", 2222))
            .await?;
        Ok(())
    }
}

impl Server for AppServer {
    type Handler = Self;
    fn new_client(&mut self, _: Option<std::net::SocketAddr>) -> Self {
        let s = self.clone();
        self.id += 1;
        s
    }
}

#[async_trait]
impl Handler for AppServer {
    type Error = anyhow::Error;

    async fn channel_open_session(
        &mut self,
        channel: Channel<Msg>,
        session: &mut Session,
    ) -> Result<bool, Self::Error> {
        {
            let mut clients = self.clients.lock().await;
            let terminal_handle = TerminalHandle {
                handle: session.handle(),
                sink: Vec::new(),
                channel_id: channel.id(),
            };

            let (user, fingerprint) = {
                let keys = self.keys.lock().await;
                let (user, key) = keys.get(&self.id).ok_or(anyhow!(ErrorKind::NotFound))?;
                (user.clone(), key.fingerprint())
            };

            clients.insert(
                self.id,
                Client::new(
                    user,
                    fingerprint,
                    &self.history.read().await,
                    terminal_handle.clone(),
                )?,
            );
        }
        Ok(true)
    }

    async fn auth_publickey(&mut self, user: &str, key: &PublicKey) -> Result<Auth, Self::Error> {
        {
            let mut keys = self.keys.lock().await;
            keys.insert(self.id, (user.into(), key.clone()));
        }
        Ok(Auth::Accept)
    }

    async fn data(
        &mut self,
        channel: ChannelId,
        data: &[u8],
        session: &mut Session,
    ) -> Result<(), Self::Error> {
        {
            let history = self.history.clone();
            let mut clients = self.clients.lock().await;
            let client = clients
                .get_mut(&self.id)
                .ok_or(anyhow!(ErrorKind::NotFound))?;
            match data {
                [3] => {
                    clients.remove(&self.id);
                    session.close(channel);
                }
                [13] => {
                    {
                        let mut history = history.write().await;
                        history.push(client.fingerprint.clone());
                        history.push(format!("{}: {}", client.user, client.input));
                        history.push("".into());
                    }
                    client.input = "".into();
                    for (_, client) in clients.iter_mut() {
                        client.render(&history.read().await)?;
                    }
                }
                [127] | [8] => {
                    client.input.pop();
                    client.render(&history.read().await)?;
                }
                [27, 91, 65] => {
                    client.scroll -= 1;
                    client.render(&history.read().await)?;
                }
                [27, 91, 66] => {
                    client.scroll += 1;
                    client.render(&history.read().await)?;
                }
                [27, 91, 53, 126] => {
                    client.scroll -= 10;
                    client.render(&history.read().await)?;
                }
                [27, 91, 54, 126] => {
                    client.scroll += 10;
                    client.render(&history.read().await)?;
                }
                text => {
                    client
                        .input
                        .push_str(&String::from_utf8_lossy(strip(text).as_slice()));
                    client.render(&history.read().await)?;
                }
            }
        }
        Ok(())
    }

    async fn window_change_request(
        &mut self,
        _: ChannelId,
        col_width: u32,
        row_height: u32,
        _: u32,
        _: u32,
        _: &mut Session,
    ) -> Result<(), Self::Error> {
        let mut clients = self.clients.lock().await;
        let client = clients.get_mut(&self.id).unwrap();
        let rect = Rect {
            x: 0,
            y: 0,
            width: col_width as u16,
            height: row_height as u16,
        };
        client.terminal.resize(rect)?;
        client.render(&self.history.read().await)?;
        Ok(())
    }
}

#[tokio::main]
async fn main() {
    env_logger::builder()
        .filter_level(log::LevelFilter::Trace)
        .init();
    let mut server = AppServer::new();
    server.run().await.expect("Failed running server");
}
