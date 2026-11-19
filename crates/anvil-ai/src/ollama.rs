//! Ollama's HTTP API: installed models, one-shot generation with a
//! `suffix` (fill-in-the-middle) and streamed chat.

use futures_util::StreamExt;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tokio::sync::mpsc;

pub const DEFAULT_URL: &str = "http://localhost:11434";

#[derive(Clone, Debug, PartialEq)]
pub enum Error {
    /// Ollama does not answer (not running, wrong port).
    Unreachable(String),
    /// Ollama answered with an error (model missing, …).
    Server(String),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::Unreachable(e) => write!(f, "Ollama is not reachable: {e}"),
            Error::Server(e) => write!(f, "Ollama: {e}"),
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
    Error::Unreachable(e.to_string())
}

impl Ollama {
    /// Only loopback URLs are accepted: the AI must never talk to anything
    /// but the local machine.
    pub fn new(base: &str) -> Option<Ollama> {
        let host = base.strip_prefix("http://")?.split(['/', ':']).next()?;
        if !matches!(host, "localhost" | "127.0.0.1" | "[::1]") {
            return None;
        }
        let http = reqwest::Client::builder().no_proxy().build().ok()?;
        Some(Ollama { http, base: base.trim_end_matches('/').to_owned() })
    }

    pub async fn models(&self) -> Result<Vec<Model>, Error> {
        #[derive(Deserialize)]
        struct Tags {
            models: Vec<Model>,
        }
        let r = self.http.get(format!("{}/api/tags", self.base)).timeout(std::time::Duration::from_secs(3)).send().await.map_err(unreachable)?;
        let tags: Tags = r.json().await.map_err(|e| Error::Server(e.to_string()))?;
        Ok(tags.models)
    }

    /// Fill-in-the-middle: the text the model puts between `prefix` and
    /// `suffix`. Short and greedy, as ghost text must be.
    pub async fn complete(&self, model: &str, prefix: &str, suffix: &str, max_tokens: u32) -> Result<String, Error> {
        let body = json!({
            "model": model,
            "prompt": prefix,
            "suffix": suffix,
            "stream": false,
            "keep_alive": "30m",
            "options": { "temperature": 0.1, "num_predict": max_tokens, "stop": ["\n\n\n"] }
        });
        let r = self.http.post(format!("{}/api/generate", self.base)).json(&body).send().await.map_err(unreachable)?;
        let v: Value = r.json().await.map_err(|e| Error::Server(e.to_string()))?;
        if let Some(e) = v.get("error").and_then(Value::as_str) {
            return Err(Error::Server(e.to_owned()));
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
            let msg = serde_json::from_str::<Value>(&text).ok().and_then(|v| v.get("error").and_then(Value::as_str).map(str::to_owned)).unwrap_or(text);
            return Err(Error::Server(msg));
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

/// Cleans a completion for ghost text: no repeat of the text after the
/// cursor, no trailing blank lines.
pub fn clean_completion(raw: &str, suffix: &str) -> String {
    let mut s = raw.trim_end_matches(['\n', '\r', ' ']).to_owned();
    // Models sometimes repeat the start of the suffix.
    let next_line = suffix.lines().find(|l| !l.trim().is_empty()).map(str::trim).unwrap_or("");
    if !next_line.is_empty()
        && let Some(i) = s.find(next_line)
    {
        s.truncate(i);
        s = s.trim_end().to_owned();
    }
    s
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
    }

    /// Against the real local Ollama when it runs (skipped otherwise).
    #[tokio::test]
    #[ignore = "needs a local Ollama with qwen2.5-coder"]
    async fn live_fim() {
        let o = Ollama::new(DEFAULT_URL).unwrap();
        let models = o.models().await.unwrap();
        let m = models.iter().find(|m| m.is_coder()).expect("a coder model");
        let out = o.complete(&m.name, "static int Add(int a, int b)\n{\n    return ", ";\n}\n", 16).await.unwrap();
        assert!(out.contains('a') && out.contains('b'), "{out}");
        let (tx, mut rx) = mpsc::unbounded_channel();
        let all = o.chat(&m.name, &[ChatMessage::new("user", "Reply with the single word: ok")], tx).await.unwrap();
        assert!(rx.recv().await.is_some());
        assert!(all.to_lowercase().contains("ok"));
    }
}
