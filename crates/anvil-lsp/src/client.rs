//! The request/response machinery: ids, pending replies, cancellation,
//! server-to-client requests and notifications.

use std::collections::HashMap;
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::{Arc, Mutex};

use serde::de::DeserializeOwned;
use serde::Serialize;
use serde_json::{json, Value};
use tokio::io::{AsyncRead, AsyncWrite, BufReader};
use tokio::sync::{mpsc, oneshot};

use crate::rpc;

#[derive(Clone, Debug, PartialEq)]
pub enum Error {
    /// The server answered with an error object.
    Rpc { code: i64, message: String },
    /// The server went away before answering.
    Closed,
    /// The answer did not have the expected shape.
    Decode(String),
    /// No answer in time; the request was cancelled.
    Timeout,
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::Rpc { code, message } => write!(f, "{message} ({code})"),
            Error::Closed => f.write_str("the language server stopped"),
            Error::Decode(e) => write!(f, "unexpected reply: {e}"),
            Error::Timeout => f.write_str("the language server did not answer in time"),
        }
    }
}

impl std::error::Error for Error {}

/// LSP's `RequestCancelled`: a stale request we cancelled ourselves.
pub const REQUEST_CANCELLED: i64 = -32800;

/// What the server sends without being asked.
#[derive(Clone, Debug)]
pub enum Incoming {
    Notification { method: String, params: Value },
    /// A request the app must answer with [`Client::respond`].
    Request { id: Value, method: String, params: Value },
    /// The server's stdout closed.
    Exited,
}

type Pending = Arc<Mutex<HashMap<i64, oneshot::Sender<Result<Value, Error>>>>>;

#[derive(Clone)]
pub struct Client {
    out: mpsc::UnboundedSender<Value>,
    pending: Pending,
    next: Arc<AtomicI64>,
}

impl Client {
    /// Starts reading and writing on the given pipes (on the current Tokio
    /// runtime). Server requests that need no app decision are answered
    /// here: `workspace/configuration` with Anvil's few settings and nulls (defaults) otherwise, capability
    /// registration and progress creation with success.
    pub fn start<R, W>(reader: R, writer: W) -> (Client, mpsc::UnboundedReceiver<Incoming>)
    where
        R: AsyncRead + Unpin + Send + 'static,
        W: AsyncWrite + Unpin + Send + 'static,
    {
        let (out_tx, mut out_rx) = mpsc::unbounded_channel::<Value>();
        let (in_tx, in_rx) = mpsc::unbounded_channel();
        let pending: Pending = Arc::default();
        let client = Client { out: out_tx.clone(), pending: pending.clone(), next: Arc::new(AtomicI64::new(1)) };

        tokio::spawn(async move {
            let mut w = writer;
            while let Some(msg) = out_rx.recv().await {
                if rpc::write_message(&mut w, &msg).await.is_err() {
                    break;
                }
            }
        });

        tokio::spawn(async move {
            let mut r = BufReader::new(reader);
            while let Ok(Some(msg)) = rpc::read_message(&mut r).await {
                let method = msg.get("method").and_then(Value::as_str).map(str::to_owned);
                let id = msg.get("id").cloned();
                match (method, id) {
                    (Some(method), Some(id)) => {
                        let params = msg.get("params").cloned().unwrap_or(Value::Null);
                        let auto = match method.as_str() {
                            "workspace/configuration" => {
                                let items = params.get("items").and_then(Value::as_array).cloned().unwrap_or_default();
                                Some(Value::Array(items.iter().map(|i| crate::roslyn::configuration_value(i.get("section").and_then(Value::as_str).unwrap_or(""))).collect()))
                            }
                            "client/registerCapability" | "client/unregisterCapability" | "window/workDoneProgress/create" => Some(Value::Null),
                            _ => None,
                        };
                        match auto {
                            Some(result) => {
                                let _ = out_tx.send(json!({"jsonrpc": "2.0", "id": id, "result": result}));
                                // The app still learns about registrations
                                // (pull-diagnostic identifiers live there).
                                if method.ends_with("registerCapability") {
                                    let _ = in_tx.send(Incoming::Notification { method, params });
                                }
                            }
                            None => {
                                let _ = in_tx.send(Incoming::Request { id, method, params });
                            }
                        }
                    }
                    (Some(method), None) => {
                        let _ = in_tx.send(Incoming::Notification { method, params: msg.get("params").cloned().unwrap_or(Value::Null) });
                    }
                    (None, Some(id)) => {
                        let Some(id) = id.as_i64() else { continue };
                        let Some(tx) = pending.lock().expect("pending lock").remove(&id) else { continue };
                        let result = match msg.get("error") {
                            Some(e) => Err(Error::Rpc {
                                code: e.get("code").and_then(Value::as_i64).unwrap_or(0),
                                message: e.get("message").and_then(Value::as_str).unwrap_or("").to_owned(),
                            }),
                            None => Ok(msg.get("result").cloned().unwrap_or(Value::Null)),
                        };
                        let _ = tx.send(result);
                    }
                    (None, None) => {}
                }
            }
            // Fail everything still waiting.
            for (_, tx) in pending.lock().expect("pending lock").drain() {
                let _ = tx.send(Err(Error::Closed));
            }
            let _ = in_tx.send(Incoming::Exited);
        });

        (client, in_rx)
    }

    /// Sends a request; returns its id (for [`Client::cancel`]) and the
    /// future reply.
    pub fn send_request(&self, method: &str, params: Value) -> (i64, oneshot::Receiver<Result<Value, Error>>) {
        let id = self.next.fetch_add(1, Ordering::Relaxed);
        let (tx, rx) = oneshot::channel();
        self.pending.lock().expect("pending lock").insert(id, tx);
        if self.out.send(message(Some(id), method, params)).is_err() {
            self.pending.lock().expect("pending lock").remove(&id);
        }
        (id, rx)
    }

    pub async fn request_raw(&self, method: &str, params: Value) -> Result<Value, Error> {
        let (_, rx) = self.send_request(method, params);
        rx.await.unwrap_or(Err(Error::Closed))
    }

    /// Gives up on a request the caller stopped waiting for (a timeout):
    /// its pending slot is freed and the server gets `$/cancelRequest`.
    pub fn abandon(&self, id: i64) {
        self.pending.lock().expect("pending lock").remove(&id);
        self.cancel(id);
    }

    /// A typed request.
    pub async fn request<R>(&self, params: R::Params) -> Result<R::Result, Error>
    where
        R: lsp_types::request::Request,
        R::Params: Serialize,
        R::Result: DeserializeOwned,
    {
        let params = serde_json::to_value(params).map_err(|e| Error::Decode(e.to_string()))?;
        let v = self.request_raw(R::METHOD, params).await?;
        serde_json::from_value(v).map_err(|e| Error::Decode(e.to_string()))
    }

    pub fn notify_raw(&self, method: &str, params: Value) {
        let _ = self.out.send(message(None, method, params));
    }

    pub fn notify<N>(&self, params: N::Params)
    where
        N: lsp_types::notification::Notification,
        N::Params: Serialize,
    {
        if let Ok(p) = serde_json::to_value(params) {
            self.notify_raw(N::METHOD, p);
        }
    }

    /// `$/cancelRequest`; the reply then arrives as an error with
    /// [`REQUEST_CANCELLED`] and is dropped by whoever waits.
    pub fn cancel(&self, id: i64) {
        self.notify_raw("$/cancelRequest", json!({ "id": id }));
    }

    pub fn respond(&self, id: Value, result: Value) {
        let _ = self.out.send(json!({"jsonrpc": "2.0", "id": id, "result": result}));
    }

    pub fn respond_error(&self, id: Value, code: i64, message: &str) {
        let _ = self.out.send(json!({"jsonrpc": "2.0", "id": id, "error": {"code": code, "message": message}}));
    }
}

/// A request (with `id`) or notification. `Null` params are left out:
/// Roslyn's StreamJsonRpc throws on `"params": null` (as `shutdown` and
/// `exit` sent it) and the server dies with a core dump.
fn message(id: Option<i64>, method: &str, params: Value) -> Value {
    let mut m = json!({"jsonrpc": "2.0", "method": method});
    if let Some(id) = id {
        m["id"] = json!(id);
    }
    if !params.is_null() {
        m["params"] = params;
    }
    m
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rpc::{read_message, write_message};

    #[test]
    fn null_params_are_left_out() {
        assert_eq!(message(Some(3), "shutdown", Value::Null), json!({"jsonrpc": "2.0", "id": 3, "method": "shutdown"}));
        assert_eq!(message(None, "exit", Value::Null), json!({"jsonrpc": "2.0", "method": "exit"}));
        assert_eq!(message(None, "x", json!({"a": 1}))["params"], json!({"a": 1}));
    }

    /// A scripted server on the other end of two pipes.
    #[tokio::test]
    async fn request_reply_notification_and_auto_answers() {
        let (client_r, mut server_w) = tokio::io::duplex(1 << 16);
        let (server_r, client_w) = tokio::io::duplex(1 << 16);
        let (client, mut incoming) = Client::start(client_r, client_w);
        let mut server_r = BufReader::new(server_r);

        let c2 = client.clone();
        let reply = tokio::spawn(async move { c2.request_raw("textDocument/hover", json!({"x": 1})).await });
        let req = read_message(&mut server_r).await.unwrap().unwrap();
        assert_eq!(req["method"], "textDocument/hover");
        write_message(&mut server_w, &json!({"jsonrpc": "2.0", "id": req["id"], "result": {"contents": "int x"}})).await.unwrap();
        assert_eq!(reply.await.unwrap().unwrap()["contents"], "int x");

        // workspace/configuration is answered without the app.
        write_message(&mut server_w, &json!({"jsonrpc": "2.0", "id": 99, "method": "workspace/configuration", "params": {"items": [{}, {}]}})).await.unwrap();
        let ans = read_message(&mut server_r).await.unwrap().unwrap();
        assert_eq!(ans["id"], 99);
        assert_eq!(ans["result"], json!([null, null]));

        write_message(&mut server_w, &json!({"jsonrpc": "2.0", "method": "workspace/projectInitializationComplete"})).await.unwrap();
        match incoming.recv().await.unwrap() {
            Incoming::Notification { method, .. } => assert_eq!(method, "workspace/projectInitializationComplete"),
            other => panic!("{other:?}"),
        }

        // An error reply, then the server goes away.
        let c3 = client.clone();
        let failing = tokio::spawn(async move { c3.request_raw("x", Value::Null).await });
        let req = read_message(&mut server_r).await.unwrap().unwrap();
        write_message(&mut server_w, &json!({"jsonrpc": "2.0", "id": req["id"], "error": {"code": REQUEST_CANCELLED, "message": "cancelled"}})).await.unwrap();
        assert_eq!(failing.await.unwrap(), Err(Error::Rpc { code: REQUEST_CANCELLED, message: "cancelled".into() }));
        let c4 = client.clone();
        let orphan = tokio::spawn(async move { c4.request_raw("y", Value::Null).await });
        let _ = read_message(&mut server_r).await;
        drop(server_w);
        assert_eq!(orphan.await.unwrap(), Err(Error::Closed));
        assert!(matches!(incoming.recv().await.unwrap(), Incoming::Exited));
    }
}
