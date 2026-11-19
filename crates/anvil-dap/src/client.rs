//! Requests, responses and events of the Debug Adapter Protocol, with the
//! typed helpers the debugger panels use.

use std::collections::HashMap;
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::{Arc, Mutex};

use serde::Deserialize;
use serde_json::{json, Value};
use tokio::io::{AsyncRead, AsyncWrite, BufReader};
use tokio::sync::{mpsc, oneshot};

use crate::rpc;

#[derive(Clone, Debug, PartialEq)]
pub enum Error {
    Failed(String),
    Closed,
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::Failed(m) => f.write_str(m),
            Error::Closed => f.write_str("the debugger stopped"),
        }
    }
}

impl std::error::Error for Error {}

#[derive(Clone, Debug, PartialEq)]
pub enum Event {
    Initialized,
    Stopped { reason: String, thread_id: Option<i64>, text: Option<String> },
    Continued,
    Output { category: String, output: String },
    Exited { code: i64 },
    Terminated,
    Breakpoint(BreakpointState),
    Other { name: String, body: Value },
    /// The adapter's connection closed.
    Closed,
}

type Pending = Arc<Mutex<HashMap<i64, oneshot::Sender<Result<Value, Error>>>>>;

#[derive(Clone)]
pub struct Client {
    out: mpsc::UnboundedSender<Value>,
    pending: Pending,
    seq: Arc<AtomicI64>,
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct StackFrame {
    pub id: i64,
    pub name: String,
    #[serde(default)]
    pub source: Option<Source>,
    #[serde(default)]
    pub line: i64,
    #[serde(default)]
    pub column: i64,
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct Source {
    #[serde(default)]
    pub path: Option<String>,
    #[serde(default)]
    pub name: Option<String>,
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct Scope {
    pub name: String,
    #[serde(rename = "variablesReference")]
    pub variables_reference: i64,
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct Variable {
    pub name: String,
    pub value: String,
    #[serde(default, rename = "type")]
    pub ty: Option<String>,
    #[serde(default, rename = "variablesReference")]
    pub variables_reference: i64,
    /// The expression that reads this variable (`names._items`).
    #[serde(default, rename = "evaluateName")]
    pub evaluate_name: Option<String>,
}

/// What the adapter says about a breakpoint: in the `setBreakpoints`
/// reply (in request order) and in `breakpoint` events (by id).
#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
pub struct BreakpointState {
    #[serde(default)]
    pub id: Option<i64>,
    #[serde(default)]
    pub verified: bool,
    #[serde(default)]
    pub line: Option<i64>,
    #[serde(default)]
    pub message: Option<String>,
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct Thread {
    pub id: i64,
    pub name: String,
}

#[derive(Clone, Debug, PartialEq)]
pub struct SourceBreakpoint {
    pub line: i64,
    pub condition: Option<String>,
}

impl Client {
    pub fn start<R, W>(reader: R, writer: W) -> (Client, mpsc::UnboundedReceiver<Event>)
    where
        R: AsyncRead + Unpin + Send + 'static,
        W: AsyncWrite + Unpin + Send + 'static,
    {
        let (out_tx, mut out_rx) = mpsc::unbounded_channel::<Value>();
        let (ev_tx, ev_rx) = mpsc::unbounded_channel();
        let pending: Pending = Arc::default();
        let client = Client { out: out_tx.clone(), pending: pending.clone(), seq: Arc::new(AtomicI64::new(1)) };
        tokio::spawn(async move {
            let mut w = writer;
            while let Some(m) = out_rx.recv().await {
                if rpc::write_message(&mut w, &m).await.is_err() {
                    break;
                }
            }
        });
        tokio::spawn(async move {
            let mut r = BufReader::new(reader);
            while let Ok(Some(msg)) = rpc::read_message(&mut r).await {
                match msg.get("type").and_then(Value::as_str) {
                    Some("response") => {
                        let Some(seq) = msg.get("request_seq").and_then(Value::as_i64) else { continue };
                        let Some(tx) = pending.lock().expect("lock").remove(&seq) else { continue };
                        let ok = msg.get("success").and_then(Value::as_bool).unwrap_or(false);
                        let _ = tx.send(if ok {
                            Ok(msg.get("body").cloned().unwrap_or(Value::Null))
                        } else {
                            Err(Error::Failed(msg.get("message").and_then(Value::as_str).unwrap_or("request failed").to_owned()))
                        });
                    }
                    Some("event") => {
                        let body = msg.get("body").cloned().unwrap_or(Value::Null);
                        let s = |k: &str| body.get(k).and_then(Value::as_str).map(str::to_owned);
                        let ev = match msg.get("event").and_then(Value::as_str).unwrap_or("") {
                            "initialized" => Event::Initialized,
                            "stopped" => Event::Stopped { reason: s("reason").unwrap_or_default(), thread_id: body.get("threadId").and_then(Value::as_i64), text: s("text").or_else(|| s("description")) },
                            "continued" => Event::Continued,
                            "output" => Event::Output { category: s("category").unwrap_or_else(|| "console".into()), output: s("output").unwrap_or_default() },
                            "exited" => Event::Exited { code: body.get("exitCode").and_then(Value::as_i64).unwrap_or(0) },
                            "terminated" => Event::Terminated,
                            "breakpoint" => match body.get("breakpoint").cloned().map(serde_json::from_value) {
                                Some(Ok(b)) => Event::Breakpoint(b),
                                _ => Event::Other { name: "breakpoint".into(), body },
                            },
                            other => Event::Other { name: other.to_owned(), body },
                        };
                        let _ = ev_tx.send(ev);
                    }
                    Some("request") => {
                        // Reverse requests (runInTerminal): netcoredbg sends
                        // none; refuse politely so it does not wait.
                        let seq = msg.get("seq").cloned().unwrap_or(Value::Null);
                        let cmd = msg.get("command").cloned().unwrap_or(Value::Null);
                        let _ = out_tx.send(json!({"type": "response", "seq": 0, "request_seq": seq, "command": cmd, "success": false, "message": "not supported"}));
                    }
                    _ => {}
                }
            }
            for (_, tx) in pending.lock().expect("lock").drain() {
                let _ = tx.send(Err(Error::Closed));
            }
            let _ = ev_tx.send(Event::Closed);
        });
        (client, ev_rx)
    }

    pub async fn request(&self, command: &str, arguments: Value) -> Result<Value, Error> {
        let seq = self.seq.fetch_add(1, Ordering::Relaxed);
        let (tx, rx) = oneshot::channel();
        self.pending.lock().expect("lock").insert(seq, tx);
        if self.out.send(json!({"seq": seq, "type": "request", "command": command, "arguments": arguments})).is_err() {
            return Err(Error::Closed);
        }
        rx.await.unwrap_or(Err(Error::Closed))
    }

    pub async fn initialize(&self) -> Result<Value, Error> {
        self.request(
            "initialize",
            json!({"clientID": "anvil", "clientName": "Anvil", "adapterID": "coreclr", "pathFormat": "path", "linesStartAt1": true, "columnsStartAt1": true, "supportsVariableType": true, "supportsRunInTerminalRequest": false, "locale": "en-us"}),
        )
        .await
    }

    /// Hit counts are not sent: netcoredbg 3.2 ignores `hitCondition`
    /// (tested), so Anvil counts hits itself.
    pub async fn set_breakpoints(&self, path: &str, bps: &[SourceBreakpoint]) -> Result<Vec<BreakpointState>, Error> {
        let list: Vec<Value> = bps
            .iter()
            .map(|b| {
                let mut v = json!({"line": b.line});
                if let Some(c) = &b.condition {
                    v["condition"] = json!(c);
                }
                v
            })
            .collect();
        let b = self.request("setBreakpoints", json!({"source": {"path": path}, "breakpoints": list})).await?;
        Ok(serde_json::from_value(b.get("breakpoints").cloned().unwrap_or_default()).unwrap_or_default())
    }

    /// Breakpoints on methods by name (`Main` for F10/F11 at the start).
    pub async fn set_function_breakpoints(&self, names: &[&str]) -> Result<Value, Error> {
        let list: Vec<Value> = names.iter().map(|n| json!({"name": n})).collect();
        self.request("setFunctionBreakpoints", json!({"breakpoints": list})).await
    }

    /// VS' Exception Settings: break when thrown (`all`) or only unhandled.
    pub async fn set_exception_breakpoints(&self, thrown: bool) -> Result<Value, Error> {
        let filters = if thrown { json!(["all", "user-unhandled"]) } else { json!(["user-unhandled"]) };
        self.request("setExceptionBreakpoints", json!({"filters": filters})).await
    }

    pub async fn configuration_done(&self) -> Result<Value, Error> {
        self.request("configurationDone", json!({})).await
    }

    pub async fn threads(&self) -> Result<Vec<Thread>, Error> {
        let b = self.request("threads", json!({})).await?;
        Ok(serde_json::from_value(b.get("threads").cloned().unwrap_or_default()).unwrap_or_default())
    }

    pub async fn stack_trace(&self, thread: i64) -> Result<Vec<StackFrame>, Error> {
        let b = self.request("stackTrace", json!({"threadId": thread, "startFrame": 0, "levels": 200})).await?;
        Ok(serde_json::from_value(b.get("stackFrames").cloned().unwrap_or_default()).unwrap_or_default())
    }

    pub async fn scopes(&self, frame: i64) -> Result<Vec<Scope>, Error> {
        let b = self.request("scopes", json!({"frameId": frame})).await?;
        Ok(serde_json::from_value(b.get("scopes").cloned().unwrap_or_default()).unwrap_or_default())
    }

    pub async fn variables(&self, reference: i64) -> Result<Vec<Variable>, Error> {
        let b = self.request("variables", json!({"variablesReference": reference})).await?;
        Ok(serde_json::from_value(b.get("variables").cloned().unwrap_or_default()).unwrap_or_default())
    }

    /// Immediate window, Watch and hover (`context` `repl`, `watch`,
    /// `hover`).
    pub async fn evaluate(&self, expression: &str, frame: Option<i64>, context: &str) -> Result<Variable, Error> {
        let mut args = json!({"expression": expression, "context": context});
        if let Some(f) = frame {
            args["frameId"] = json!(f);
        }
        let b = self.request("evaluate", args).await?;
        Ok(Variable {
            name: expression.to_owned(),
            value: b.get("result").and_then(Value::as_str).unwrap_or("").to_owned(),
            ty: b.get("type").and_then(Value::as_str).map(str::to_owned),
            variables_reference: b.get("variablesReference").and_then(Value::as_i64).unwrap_or(0),
            evaluate_name: Some(expression.to_owned()),
        })
    }

    pub async fn continue_(&self, thread: i64) -> Result<Value, Error> {
        self.request("continue", json!({"threadId": thread})).await
    }

    pub async fn next(&self, thread: i64) -> Result<Value, Error> {
        self.request("next", json!({"threadId": thread})).await
    }

    pub async fn step_in(&self, thread: i64) -> Result<Value, Error> {
        self.request("stepIn", json!({"threadId": thread})).await
    }

    pub async fn step_out(&self, thread: i64) -> Result<Value, Error> {
        self.request("stepOut", json!({"threadId": thread})).await
    }

    pub async fn pause(&self, thread: i64) -> Result<Value, Error> {
        self.request("pause", json!({"threadId": thread})).await
    }

    pub async fn disconnect(&self, terminate: bool) -> Result<Value, Error> {
        self.request("disconnect", json!({"terminateDebuggee": terminate})).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rpc::{read_message, write_message};

    #[tokio::test]
    async fn session_against_a_scripted_adapter() {
        let (cr, mut sw) = tokio::io::duplex(1 << 16);
        let (sr, cw) = tokio::io::duplex(1 << 16);
        let (client, mut events) = Client::start(cr, cw);
        let mut sr = BufReader::new(sr);

        let c = client.clone();
        let init = tokio::spawn(async move { c.initialize().await });
        let req = read_message(&mut sr).await.unwrap().unwrap();
        assert_eq!(req["command"], "initialize");
        write_message(&mut sw, &json!({"type": "response", "request_seq": req["seq"], "success": true, "command": "initialize", "body": {"supportsConfigurationDoneRequest": true}})).await.unwrap();
        assert_eq!(init.await.unwrap().unwrap()["supportsConfigurationDoneRequest"], true);

        let c = client.clone();
        let set = tokio::spawn(async move { c.set_breakpoints("/a/Program.cs", &[SourceBreakpoint { line: 11, condition: Some("i == 3".into()) }]).await });
        let req = read_message(&mut sr).await.unwrap().unwrap();
        assert_eq!(req["arguments"]["breakpoints"], json!([{"line": 11, "condition": "i == 3"}]));
        // As netcoredbg answers before the program has loaded.
        write_message(&mut sw, &json!({"type": "response", "request_seq": req["seq"], "success": true, "command": "setBreakpoints", "body": {"breakpoints": [{"id": 1, "line": 11, "verified": false, "message": "The breakpoint is pending"}]}})).await.unwrap();
        assert_eq!(set.await.unwrap().unwrap(), [BreakpointState { id: Some(1), verified: false, line: Some(11), message: Some("The breakpoint is pending".into()) }]);
        write_message(&mut sw, &json!({"type": "event", "event": "breakpoint", "body": {"reason": "changed", "breakpoint": {"id": 1, "line": 11, "verified": true}}})).await.unwrap();
        assert_eq!(events.recv().await.unwrap(), Event::Breakpoint(BreakpointState { id: Some(1), verified: true, line: Some(11), message: None }));

        write_message(&mut sw, &json!({"type": "event", "event": "stopped", "body": {"reason": "breakpoint", "threadId": 7}})).await.unwrap();
        assert_eq!(events.recv().await.unwrap(), Event::Stopped { reason: "breakpoint".into(), thread_id: Some(7), text: None });

        let c = client.clone();
        let st = tokio::spawn(async move { c.stack_trace(7).await });
        let req = read_message(&mut sr).await.unwrap().unwrap();
        write_message(&mut sw, &json!({"type": "response", "request_seq": req["seq"], "success": true, "command": "stackTrace", "body": {"stackFrames": [{"id": 1, "name": "Program.Main()", "line": 12, "column": 13, "source": {"path": "/a/Program.cs"}}]}})).await.unwrap();
        let frames = st.await.unwrap().unwrap();
        assert_eq!(frames[0].line, 12);
        assert_eq!(frames[0].source.as_ref().unwrap().path.as_deref(), Some("/a/Program.cs"));

        let c = client.clone();
        let bad = tokio::spawn(async move { c.evaluate("x +", Some(1), "repl").await });
        let req = read_message(&mut sr).await.unwrap().unwrap();
        write_message(&mut sw, &json!({"type": "response", "request_seq": req["seq"], "success": false, "command": "evaluate", "message": "error CS1525"})).await.unwrap();
        assert_eq!(bad.await.unwrap(), Err(Error::Failed("error CS1525".into())));
        drop(sw);
        assert_eq!(events.recv().await.unwrap(), Event::Closed);
    }
}
