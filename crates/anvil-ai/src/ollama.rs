//! Ollama's HTTP API: installed models, one-shot generation with a
//! `suffix` (fill-in-the-middle) and streamed chat.

use futures_util::StreamExt;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::time::Duration;
use tokio::sync::mpsc;

pub const DEFAULT_URL: &str = "http://localhost:11434";

#[derive(Clone, Debug, PartialEq)]
pub enum Error {
    /// Ollama does not answer (not running, wrong port).
    Unreachable(String),
    /// Ollama answered with an error.
    Server(String),
    /// The model is not installed (`ollama pull` it).
    ModelMissing(String),
    /// No model that fills in the middle is installed.
    NoModel,
    /// The model did not answer in time (too big for this machine).
    Timeout,
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::Unreachable(e) => write!(f, "Ollama is not reachable: {e}"),
            Error::Server(e) => write!(f, "Ollama: {e}"),
            Error::ModelMissing(m) => write!(f, "Ollama has no model `{m}`"),
            Error::NoModel => write!(f, "Ollama has no coder model"),
            Error::Timeout => write!(f, "Ollama did not answer in time"),
        }
    }
}

impl std::error::Error for Error {}

#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct Model {
    pub name: String,
    #[serde(default)]
    pub size: u64,
    #[serde(default)]
    pub capabilities: Vec<String>,
}

impl Model {
    /// Coder models are trained for fill-in-the-middle.
    pub fn is_coder(&self) -> bool {
        let n = self.name.to_lowercase();
        n.contains("coder") || n.contains("starcoder") || n.contains("codellama") || n.contains("codestral") || n.contains("deepseek-coder")
    }

    /// Ollama lists `insert` for models whose template takes a suffix;
    /// older servers list no capabilities, then the name decides.
    pub fn supports_fim(&self) -> bool {
        if self.capabilities.is_empty() { self.is_coder() } else { self.capabilities.iter().any(|c| c == "insert") }
    }
}

/// The model for ghost text: the configured one if it is installed,
/// otherwise (empty setting) the smallest model that fills in the middle.
/// Ghost text has to come within a second or two, and on a machine
/// without a GPU only a small model does that (qwen2.5-coder, warm, on a
/// laptop CPU: 1.5b about 1.5 s, 7b about 8 s).
pub fn pick_inline_model(models: &[Model], configured: &str) -> Result<String, Error> {
    let configured = configured.trim();
    if !configured.is_empty() {
        // `qwen2.5-coder` means `qwen2.5-coder:latest`.
        let found = models.iter().any(|m| m.name == configured || m.name.strip_suffix(":latest") == Some(configured));
        return if found { Ok(configured.to_owned()) } else { Err(Error::ModelMissing(configured.to_owned())) };
    }
    models.iter().filter(|m| m.supports_fim()).min_by_key(|m| m.size).map(|m| m.name.clone()).ok_or(Error::NoModel)
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct ChatMessage {
    pub role: String,
    pub content: String,
}

impl ChatMessage {
    pub fn new(role: &str, content: impl Into<String>) -> Self {
        ChatMessage { role: role.to_owned(), content: content.into() }
    }
}

#[derive(Clone)]
pub struct Ollama {
    http: reqwest::Client,
    base: String,
}

fn unreachable(e: reqwest::Error) -> Error {
    if e.is_timeout() { Error::Timeout } else { Error::Unreachable(e.to_string()) }
}

/// An error body from Ollama (`{"error": "model 'x' not found"}`).
fn server_error(model: &str, text: &str) -> Error {
    let msg = serde_json::from_str::<Value>(text).ok().and_then(|v| v.get("error").and_then(Value::as_str).map(str::to_owned)).unwrap_or_else(|| text.to_owned());
    if msg.contains("not found") { Error::ModelMissing(model.to_owned()) } else { Error::Server(msg) }
}

impl Ollama {
    /// Only loopback URLs are accepted: the AI must never talk to anything
    /// but the local machine.
    pub fn new(base: &str) -> Option<Ollama> {
        let host = base.strip_prefix("http://")?.split(['/', ':']).next()?;
        if !matches!(host, "localhost" | "127.0.0.1" | "[::1]") {
            return None;
        }
        let http = reqwest::Client::builder().no_proxy().connect_timeout(Duration::from_secs(2)).build().ok()?;
        Some(Ollama { http, base: base.trim_end_matches('/').to_owned() })
    }

    pub fn url(&self) -> &str {
        &self.base
    }

    pub async fn models(&self) -> Result<Vec<Model>, Error> {
        #[derive(Deserialize)]
        struct Tags {
            models: Vec<Model>,
        }
        let r = self.http.get(format!("{}/api/tags", self.base)).timeout(Duration::from_secs(3)).send().await.map_err(unreachable)?;
        let tags: Tags = r.json().await.map_err(|e| Error::Server(e.to_string()))?;
        Ok(tags.models)
    }

    /// Whether `model` is in memory now (`/api/ps`).
    pub async fn is_loaded(&self, model: &str) -> Result<bool, Error> {
        #[derive(Deserialize)]
        struct Ps {
            models: Vec<Model>,
        }
        let r = self.http.get(format!("{}/api/ps", self.base)).timeout(Duration::from_secs(3)).send().await.map_err(unreachable)?;
        let ps: Ps = r.json().await.map_err(|e| Error::Server(e.to_string()))?;
        Ok(ps.models.iter().any(|m| m.name == model || m.name.strip_suffix(":latest") == Some(model)))
    }

    /// Loads `model` without generating. Ollama stops loading when the
    /// request is dropped, so callers that abort on typing run this apart
    /// from the request they abort.
    pub async fn load(&self, model: &str, timeout: Duration) -> Result<(), Error> {
        let body = json!({ "model": model, "keep_alive": "30m" });
        let r = self.http.post(format!("{}/api/generate", self.base)).json(&body).timeout(timeout).send().await.map_err(unreachable)?;
        if r.status().is_success() { Ok(()) } else { Err(server_error(model, &r.text().await.unwrap_or_default())) }
    }

    /// Fill-in-the-middle: the text the model puts between `prefix` and
    /// `suffix`. Short and greedy, as ghost text must be; a blank line
    /// ends it. Dropping the future closes the connection and Ollama stops
    /// generating, so a stale request never queues ahead of a new one.
    pub async fn complete(&self, model: &str, prefix: &str, suffix: &str, max_tokens: u32, timeout: Duration) -> Result<String, Error> {
        let body = json!({
            "model": model,
            "prompt": prefix,
            "suffix": suffix,
            "stream": false,
            "keep_alive": "30m",
            "options": { "temperature": 0.1, "num_predict": max_tokens, "stop": ["\n\n"] }
        });
        let r = self.http.post(format!("{}/api/generate", self.base)).json(&body).timeout(timeout).send().await.map_err(unreachable)?;
        if !r.status().is_success() {
            let text = r.text().await.unwrap_or_default();
            return Err(server_error(model, &text));
        }
        let v: Value = r.json().await.map_err(|e| if e.is_timeout() { Error::Timeout } else { Error::Server(e.to_string()) })?;
        if let Some(e) = v.get("error").and_then(Value::as_str) {
            return Err(server_error(model, &json!({ "error": e }).to_string()));
        }
        Ok(v.get("response").and_then(Value::as_str).unwrap_or("").to_owned())
    }

    /// Streams a chat answer piece by piece into `tx`; returns the whole
    /// answer. Dropping the receiver stops the stream.
    pub async fn chat(&self, model: &str, messages: &[ChatMessage], tx: mpsc::UnboundedSender<String>) -> Result<String, Error> {
        let body = json!({ "model": model, "messages": messages, "stream": true, "keep_alive": "30m", "options": { "temperature": 0.2 } });
        let r = self.http.post(format!("{}/api/chat", self.base)).json(&body).send().await.map_err(unreachable)?;
        if !r.status().is_success() {
            let text = r.text().await.unwrap_or_default();
            return Err(server_error(model, &text));
        }
        let mut stream = r.bytes_stream();
        let mut buf = Vec::new();
        let mut all = String::new();
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(unreachable)?;
            buf.extend_from_slice(&chunk);
            while let Some(nl) = buf.iter().position(|&b| b == b'\n') {
                let line: Vec<u8> = buf.drain(..=nl).collect();
                let Ok(v) = serde_json::from_slice::<Value>(&line) else { continue };
                if let Some(e) = v.get("error").and_then(Value::as_str) {
                    return Err(Error::Server(e.to_owned()));
                }
                if let Some(piece) = v.pointer("/message/content").and_then(Value::as_str) {
                    all.push_str(piece);
                    if tx.send(piece.to_owned()).is_err() {
                        return Ok(all);
                    }
                }
            }
        }
        Ok(all)
    }
}

/// Cleans a completion for ghost text: no trailing blank lines, nothing
/// from the line of a `}` that closes a block the completion did not open
/// (models like to repeat the closing lines after the cursor), and no line
/// the text after the cursor already starts with.
pub fn clean_completion(raw: &str, suffix: &str) -> String {
    let mut depth = 0i32;
    let mut cut = raw.len();
    for (i, c) in raw.char_indices() {
        match c {
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth < 0 {
                    cut = raw[..i].rfind('\n').unwrap_or(i);
                    break;
                }
            }
            _ => {}
        }
    }
    let s = raw[..cut].trim_end_matches(['\n', '\r', ' ', '\t']);
    let next_line = suffix.lines().find(|l| !l.trim().is_empty()).map(str::trim).unwrap_or("");
    let mut keep = s.len();
    if !next_line.is_empty() {
        let (mut at, mut depth) = (0, 0i32);
        for (n, line) in s.split('\n').enumerate() {
            // Only outside blocks the completion opened itself.
            if n > 0 && depth == 0 && line.trim() == next_line {
                keep = at;
                break;
            }
            depth += line.matches('{').count() as i32 - line.matches('}').count() as i32;
            at += line.len() + 1;
        }
    }
    s[..keep.min(s.len())].trim_end().to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_loopback() {
        assert!(Ollama::new("http://localhost:11434").is_some());
        assert!(Ollama::new("http://127.0.0.1:11434/").is_some());
        assert!(Ollama::new("http://example.com:11434").is_none());
        assert!(Ollama::new("https://localhost").is_none());
    }

    #[test]
    fn cleans_ghost_text() {
        assert_eq!(clean_completion("return this.p;\n        }\n\n", "\n        }\n"), "return this.p;");
        assert_eq!(clean_completion("x + 1;\n\n", ""), "x + 1;");
        // What qwen2.5-coder answered after `Console.` in a method.
        let raw = "WriteLine(p.Name);\n            Console.WriteLine(p.Age);\n        }\n    }";
        assert_eq!(clean_completion(raw, "\n        }\n    }\n}\n"), "WriteLine(p.Name);\n            Console.WriteLine(p.Age);");
        // Blocks the completion opens itself stay whole.
        assert_eq!(clean_completion("if (x)\n{\n    y();\n}", "\n}"), "if (x)\n{\n    y();\n}");
        assert_eq!(clean_completion("a();\nb();", "b();\n"), "a();");
        assert_eq!(clean_completion("}", "\n}"), "");
    }

    fn model(name: &str, size: u64, caps: &[&str]) -> Model {
        Model { name: name.into(), size, capabilities: caps.iter().map(|c| c.to_string()).collect() }
    }

    #[test]
    fn picks_the_smallest_fim_model() {
        let models = [model("qwen2.5:3b", 2, &["completion"]), model("qwen2.5-coder:7b", 7, &["completion", "insert"]), model("qwen2.5-coder:1.5b", 1, &["completion", "insert"])];
        assert_eq!(pick_inline_model(&models, ""), Ok("qwen2.5-coder:1.5b".into()));
        assert_eq!(pick_inline_model(&models, "qwen2.5-coder:7b"), Ok("qwen2.5-coder:7b".into()));
        assert_eq!(pick_inline_model(&models, "codellama"), Err(Error::ModelMissing("codellama".into())));
        assert_eq!(pick_inline_model(&models[..1], ""), Err(Error::NoModel));
        assert_eq!(pick_inline_model(&[model("starcoder2:latest", 3, &[])], "starcoder2"), Ok("starcoder2".into()));
    }

    #[test]
    fn maps_server_errors() {
        assert_eq!(server_error("x", r#"{"error":"model 'x' not found"}"#), Error::ModelMissing("x".into()));
        assert_eq!(server_error("x", "boom"), Error::Server("boom".into()));
    }

    /// Against the real local Ollama when it runs (skipped otherwise).
    #[tokio::test]
    #[ignore = "needs a local Ollama with qwen2.5-coder"]
    async fn live_fim() {
        let o = Ollama::new(DEFAULT_URL).unwrap();
        let models = o.models().await.unwrap();
        let m = pick_inline_model(&models, "").expect("a coder model");
        o.load(&m, Duration::from_secs(120)).await.unwrap();
        assert!(o.is_loaded(&m).await.unwrap());
        let out = o.complete(&m, "static int Add(int a, int b)\n{\n    return ", ";\n}\n", 16, Duration::from_secs(120)).await.unwrap();
        assert!(out.contains('a') && out.contains('b'), "{out}");
        let missing = o.complete("no-such-model:1b", "a", "b", 4, Duration::from_secs(10)).await;
        assert_eq!(missing, Err(Error::ModelMissing("no-such-model:1b".into())));
        assert_eq!(Ollama::new("http://127.0.0.1:1").unwrap().models().await.map(|_| ()).map_err(|e| matches!(e, Error::Unreachable(_))), Err(true));
        let (tx, mut rx) = mpsc::unbounded_channel();
        let all = o.chat(&m, &[ChatMessage::new("user", "Reply with the single word: ok")], tx).await.unwrap();
        assert!(rx.recv().await.is_some());
        assert!(all.to_lowercase().contains("ok"));
    }
}
