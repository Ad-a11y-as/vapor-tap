use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use tokio::sync::{mpsc, oneshot};
use tokio::task::JoinHandle;
use tokio::time::timeout;
use tokio_tungstenite::connect_async;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::http::header::{
    AUTHORIZATION, HeaderValue, SEC_WEBSOCKET_PROTOCOL,
};

use crate::audio::Pcm16Chunk;
use crate::{Error, Result};

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum AsrMode {
    Online,
    Offline,
    #[default]
    TwoPass,
}

impl AsrMode {
    fn as_protocol_str(self) -> &'static str {
        match self {
            Self::Online => "online",
            Self::Offline => "offline",
            Self::TwoPass => "2pass",
        }
    }
}

/// Connection and protocol settings for a remote FunASR WebSocket service.
#[derive(Clone)]
pub struct FunAsrConfig {
    pub url: String,
    pub mode: AsrMode,
    pub wav_name: String,
    /// Optional JSON hotword object, for example `{"Vapor Tap": 20}`.
    pub hotwords: Option<String>,
    pub use_itn: bool,
    pub bearer_token: Option<String>,
    pub audio_queue_capacity: usize,
    pub connect_timeout: Duration,
    pub final_result_timeout: Duration,
}

impl FunAsrConfig {
    pub fn new(url: impl Into<String>) -> Self {
        Self {
            url: url.into(),
            mode: AsrMode::TwoPass,
            wav_name: "vapor-tap".into(),
            hotwords: None,
            use_itn: true,
            bearer_token: None,
            audio_queue_capacity: 128,
            connect_timeout: Duration::from_secs(10),
            final_result_timeout: Duration::from_secs(30),
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum TranscriptEvent {
    Partial {
        text: String,
    },
    Final {
        text: String,
        timestamp: Option<String>,
    },
    ServerError {
        message: String,
    },
    End,
    Disconnected {
        reason: String,
    },
}

pub type FunAsrEventReceiver = mpsc::UnboundedReceiver<TranscriptEvent>;

enum InputMessage {
    Audio(Pcm16Chunk),
    Finish,
}

/// Cloneable, bounded input side of a FunASR connection.
#[derive(Clone)]
pub struct FunAsrInput {
    sender: mpsc::Sender<InputMessage>,
}

impl FunAsrInput {
    /// Non-blocking send used by the audio conversion worker. Queue overflow is
    /// reported explicitly because dropping speech corrupts a transcript.
    pub fn try_send(&self, chunk: Pcm16Chunk) -> Result<()> {
        self.sender
            .try_send(InputMessage::Audio(chunk))
            .map_err(|error| match error {
                mpsc::error::TrySendError::Full(_) => Error::AudioQueueFull,
                mpsc::error::TrySendError::Closed(_) => {
                    Error::Network("FunASR connection is closed".into())
                }
            })
    }

    pub async fn send(&self, chunk: Pcm16Chunk) -> Result<()> {
        self.sender
            .send(InputMessage::Audio(chunk))
            .await
            .map_err(|_| Error::Network("FunASR connection is closed".into()))
    }
}

/// A running remote FunASR session.
pub struct FunAsrClient {
    input: FunAsrInput,
    events: Option<FunAsrEventReceiver>,
    task: Option<JoinHandle<Result<()>>>,
}

impl FunAsrClient {
    pub async fn connect(config: FunAsrConfig) -> Result<Self> {
        if config.audio_queue_capacity == 0 {
            return Err(Error::InvalidArgument(
                "FunASR audio queue capacity must be non-zero",
            ));
        }
        if config.url.trim().is_empty() {
            return Err(Error::InvalidArgument("FunASR URL must be non-empty"));
        }

        let (input_sender, input_receiver) = mpsc::channel(config.audio_queue_capacity);
        let (event_sender, event_receiver) = mpsc::unbounded_channel();
        let (ready_sender, ready_receiver) = oneshot::channel();
        let task = tokio::spawn(async move {
            let mut ready_sender = Some(ready_sender);
            let result =
                run_connection(config, input_receiver, event_sender, &mut ready_sender).await;
            if let Some(sender) = ready_sender {
                let message = result
                    .as_ref()
                    .err()
                    .map(ToString::to_string)
                    .unwrap_or_else(|| "FunASR connection ended during startup".into());
                let _ = sender.send(Err(message));
            }
            result
        });
        match ready_receiver.await {
            Ok(Ok(())) => {}
            Ok(Err(error)) => {
                let _ = task.await;
                return Err(Error::Network(error));
            }
            Err(_) => {
                let _ = task.await;
                return Err(Error::Network(
                    "FunASR connection task ended during startup".into(),
                ));
            }
        }
        let input = FunAsrInput {
            sender: input_sender,
        };
        Ok(Self {
            input,
            events: Some(event_receiver),
            task: Some(task),
        })
    }

    pub fn input(&self) -> FunAsrInput {
        self.input.clone()
    }

    pub fn take_events(&mut self) -> Result<FunAsrEventReceiver> {
        self.events
            .take()
            .ok_or(Error::InvalidArgument("FunASR events were already taken"))
    }

    /// Signals end of speech and waits for the server's final acknowledgement.
    pub async fn finish(mut self) -> Result<()> {
        let _ = self.input.sender.send(InputMessage::Finish).await;
        if let Some(task) = self.task.take() {
            task.await
                .map_err(|error| Error::Network(format!("FunASR task failed: {error}")))??;
        }
        Ok(())
    }
}

impl Drop for FunAsrClient {
    fn drop(&mut self) {
        if let Some(task) = self.task.take() {
            task.abort();
        }
    }
}

#[derive(Serialize)]
struct StartMessage<'a> {
    mode: &'a str,
    wav_name: &'a str,
    wav_format: &'static str,
    audio_fs: u32,
    chunk_size: [u32; 3],
    chunk_interval: u32,
    is_speaking: bool,
    itn: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    hotwords: Option<&'a str>,
}

#[derive(Serialize)]
struct FinishMessage {
    is_speaking: bool,
    is_end: bool,
}

fn deserialize_timestamp<'de, D>(deserializer: D) -> std::result::Result<Option<String>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let value = Option::<serde_json::Value>::deserialize(deserializer)?;
    Ok(match value {
        None | Some(serde_json::Value::Null) => None,
        Some(serde_json::Value::String(timestamp)) => Some(timestamp),
        Some(timestamp) => Some(timestamp.to_string()),
    })
}

#[derive(Debug, Deserialize)]
struct ServerMessage {
    #[serde(default)]
    mode: String,
    #[serde(default)]
    text: String,
    #[serde(default, deserialize_with = "deserialize_timestamp")]
    timestamp: Option<String>,
    #[serde(default)]
    is_final: bool,
    #[serde(default)]
    is_end: bool,
    #[serde(default)]
    error: Option<String>,
}

async fn run_connection(
    config: FunAsrConfig,
    mut input: mpsc::Receiver<InputMessage>,
    events: mpsc::UnboundedSender<TranscriptEvent>,
    ready: &mut Option<oneshot::Sender<std::result::Result<(), String>>>,
) -> Result<()> {
    let mut request = config
        .url
        .as_str()
        .into_client_request()
        .map_err(|error| Error::Network(format!("invalid FunASR URL: {error}")))?;
    request
        .headers_mut()
        .insert(SEC_WEBSOCKET_PROTOCOL, HeaderValue::from_static("binary"));
    if let Some(token) = config.bearer_token.as_deref() {
        let value = HeaderValue::from_str(&format!("Bearer {token}"))
            .map_err(|error| Error::Network(format!("invalid bearer token: {error}")))?;
        request.headers_mut().insert(AUTHORIZATION, value);
    }

    let (mut socket, _) = timeout(config.connect_timeout, connect_async(request))
        .await
        .map_err(|_| Error::Network("FunASR connection timed out".into()))?
        .map_err(|error| Error::Network(error.to_string()))?;

    let start = StartMessage {
        mode: config.mode.as_protocol_str(),
        wav_name: &config.wav_name,
        wav_format: "pcm",
        audio_fs: 16_000,
        chunk_size: [5, 10, 5],
        chunk_interval: 10,
        is_speaking: true,
        itn: config.use_itn,
        hotwords: config.hotwords.as_deref(),
    };
    send_json(&mut socket, &start).await?;
    if let Some(sender) = ready.take() {
        let _ = sender.send(Ok(()));
    }

    loop {
        tokio::select! {
            command = input.recv() => {
                match command {
                    Some(InputMessage::Audio(chunk)) => {
                        socket.send(Message::Binary(chunk.bytes.into())).await
                            .map_err(|error| disconnected(&events, error.to_string()))?;
                    }
                    Some(InputMessage::Finish) | None => {
                        send_json(
                            &mut socket,
                            &FinishMessage {
                                is_speaking: false,
                                is_end: true,
                            },
                        )
                        .await?;
                        break;
                    }
                }
            }
            incoming = socket.next() => {
                if handle_incoming(incoming, &events)? {
                    return Ok(());
                }
            }
        }
    }

    timeout(config.final_result_timeout, async {
        loop {
            let incoming = socket.next().await;
            if handle_incoming(incoming, &events)? {
                return Ok::<(), Error>(());
            }
        }
    })
    .await
    .map_err(|_| Error::Network("timed out waiting for final FunASR result".into()))??;
    let _ = socket.close(None).await;
    Ok(())
}

async fn send_json<S, T>(
    socket: &mut tokio_tungstenite::WebSocketStream<S>,
    value: &T,
) -> Result<()>
where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
    T: Serialize,
{
    let json = serde_json::to_string(value)
        .map_err(|error| Error::Protocol(format!("serialize request: {error}")))?;
    socket
        .send(Message::Text(json.into()))
        .await
        .map_err(|error| Error::Network(error.to_string()))
}

fn handle_incoming(
    incoming: Option<std::result::Result<Message, tokio_tungstenite::tungstenite::Error>>,
    events: &mpsc::UnboundedSender<TranscriptEvent>,
) -> Result<bool> {
    match incoming {
        Some(Ok(Message::Text(text))) => {
            let message: ServerMessage = serde_json::from_str(text.as_ref())
                .map_err(|error| Error::Protocol(format!("{error}; message={text}")))?;
            emit_server_message(message, events)
        }
        Some(Ok(Message::Close(frame))) => {
            let reason = frame
                .map(|frame| frame.reason.to_string())
                .unwrap_or_else(|| "server closed the WebSocket".into());
            Err(disconnected(events, reason))
        }
        Some(Ok(Message::Ping(_) | Message::Pong(_) | Message::Binary(_) | Message::Frame(_))) => {
            Ok(false)
        }
        Some(Err(error)) => Err(disconnected(events, error.to_string())),
        None => Err(disconnected(events, "FunASR stream ended")),
    }
}

fn emit_server_message(
    message: ServerMessage,
    events: &mpsc::UnboundedSender<TranscriptEvent>,
) -> Result<bool> {
    if let Some(error) = message.error.filter(|error| !error.is_empty()) {
        let _ = events.send(TranscriptEvent::ServerError {
            message: error.clone(),
        });
        return Err(Error::Protocol(format!("FunASR server error: {error}")));
    }
    if !message.text.is_empty() {
        let is_final =
            message.is_final || message.mode == "offline" || message.mode == "2pass-offline";
        let event = if is_final {
            TranscriptEvent::Final {
                text: message.text,
                timestamp: message.timestamp,
            }
        } else {
            TranscriptEvent::Partial { text: message.text }
        };
        let _ = events.send(event);
    }
    if message.is_end {
        if !message.is_final {
            let message = "FunASR ended the session without a final acknowledgement".to_owned();
            let _ = events.send(TranscriptEvent::ServerError {
                message: message.clone(),
            });
            return Err(Error::Protocol(message));
        }
        let _ = events.send(TranscriptEvent::End);
        return Ok(true);
    }
    Ok(false)
}

fn disconnected(
    events: &mpsc::UnboundedSender<TranscriptEvent>,
    reason: impl Into<String>,
) -> Error {
    let reason = reason.into();
    let _ = events.send(TranscriptEvent::Disconnected {
        reason: reason.clone(),
    });
    Error::Network(reason)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::net::{TcpListener, TcpStream};
    use tokio_tungstenite::WebSocketStream;
    use tokio_tungstenite::accept_hdr_async;
    use tokio_tungstenite::tungstenite::handshake::server::{Request, Response};

    #[allow(clippy::result_large_err)]
    async fn accept_funasr(stream: TcpStream) -> WebSocketStream<TcpStream> {
        accept_hdr_async(stream, |request: &Request, mut response: Response| {
            assert_eq!(
                request.headers().get(SEC_WEBSOCKET_PROTOCOL),
                Some(&HeaderValue::from_static("binary"))
            );
            response
                .headers_mut()
                .insert(SEC_WEBSOCKET_PROTOCOL, HeaderValue::from_static("binary"));
            Ok(response)
        })
        .await
        .unwrap()
    }

    #[test]
    fn maps_online_and_offline_messages() {
        let (sender, mut receiver) = mpsc::unbounded_channel();
        emit_server_message(
            ServerMessage {
                mode: "2pass-online".into(),
                text: "临时".into(),
                timestamp: None,
                is_final: false,
                is_end: false,
                error: None,
            },
            &sender,
        )
        .unwrap();
        emit_server_message(
            ServerMessage {
                mode: "2pass-offline".into(),
                text: "最终。".into(),
                timestamp: Some("[[0,100]]".into()),
                is_final: true,
                is_end: true,
                error: None,
            },
            &sender,
        )
        .unwrap();

        assert_eq!(
            receiver.try_recv().unwrap(),
            TranscriptEvent::Partial {
                text: "临时".into()
            }
        );
        assert!(matches!(
            receiver.try_recv().unwrap(),
            TranscriptEvent::Final { .. }
        ));
        assert_eq!(receiver.try_recv().unwrap(), TranscriptEvent::End);
    }

    #[tokio::test]
    async fn streams_pcm_to_a_remote_websocket_and_receives_text() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let mut socket = accept_funasr(stream).await;

            let Message::Text(start) = socket.next().await.unwrap().unwrap() else {
                panic!("expected start JSON");
            };
            let start: serde_json::Value = serde_json::from_str(start.as_ref()).unwrap();
            assert_eq!(start["mode"], "2pass");
            assert_eq!(start["audio_fs"], 16_000);
            assert_eq!(start["wav_format"], "pcm");

            socket
                .send(Message::Text(
                    r#"{"mode":"2pass-online","text":"hello"}"#.into(),
                ))
                .await
                .unwrap();
            let Message::Binary(audio) = socket.next().await.unwrap().unwrap() else {
                panic!("expected binary PCM");
            };
            assert_eq!(audio.len(), 1_920);

            let Message::Text(finish) = socket.next().await.unwrap().unwrap() else {
                panic!("expected finish JSON");
            };
            let finish: serde_json::Value = serde_json::from_str(finish.as_ref()).unwrap();
            assert_eq!(finish["is_speaking"], false);
            assert_eq!(finish["is_end"], true);
            socket
                .send(Message::Text(
                    r#"{"mode":"2pass-offline","text":"hello.","timestamp":[[0,100]],"is_final":true,"is_end":true}"#.into(),
                ))
                .await
                .unwrap();
        });

        let mut config = FunAsrConfig::new(format!("ws://{address}"));
        config.connect_timeout = Duration::from_secs(2);
        config.final_result_timeout = Duration::from_secs(2);
        let mut client = FunAsrClient::connect(config).await.unwrap();
        let input = client.input();
        let mut events = client.take_events().unwrap();
        input
            .send(Pcm16Chunk {
                bytes: vec![0; 1_920],
                samples: 960,
            })
            .await
            .unwrap();
        client.finish().await.unwrap();
        server.await.unwrap();

        assert!(matches!(
            events.recv().await.unwrap(),
            TranscriptEvent::Partial { .. }
        ));
        assert_eq!(
            events.recv().await.unwrap(),
            TranscriptEvent::Final {
                text: "hello.".into(),
                timestamp: Some("[[0,100]]".into()),
            }
        );
        assert_eq!(events.recv().await.unwrap(), TranscriptEvent::End);
    }

    #[tokio::test]
    async fn rejects_a_failed_end_of_input_acknowledgement() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let mut socket = accept_funasr(stream).await;

            let Message::Text(_) = socket.next().await.unwrap().unwrap() else {
                panic!("expected start JSON");
            };
            let Message::Text(finish) = socket.next().await.unwrap().unwrap() else {
                panic!("expected finish JSON");
            };
            let finish: serde_json::Value = serde_json::from_str(finish.as_ref()).unwrap();
            assert_eq!(finish["is_speaking"], false);
            assert_eq!(finish["is_end"], true);

            socket
                .send(Message::Text(
                    r#"{"is_end":true,"is_final":false,"error":"model failed"}"#.into(),
                ))
                .await
                .unwrap();
        });

        let mut config = FunAsrConfig::new(format!("ws://{address}"));
        config.connect_timeout = Duration::from_secs(2);
        config.final_result_timeout = Duration::from_secs(2);
        let mut client = FunAsrClient::connect(config).await.unwrap();
        let mut events = client.take_events().unwrap();

        let error = client.finish().await.unwrap_err();
        server.await.unwrap();

        assert!(matches!(error, Error::Protocol(message) if message.contains("model failed")));
        assert_eq!(
            events.recv().await.unwrap(),
            TranscriptEvent::ServerError {
                message: "model failed".into()
            }
        );
        assert!(events.recv().await.is_none());
    }
}
