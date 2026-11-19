//! `Content-Length` framing, as in LSP.

use serde_json::Value;
use tokio::io::{AsyncBufRead, AsyncBufReadExt, AsyncReadExt, AsyncWrite, AsyncWriteExt};

pub async fn read_message<R: AsyncBufRead + Unpin>(r: &mut R) -> std::io::Result<Option<Value>> {
    let mut length: Option<usize> = None;
    loop {
        let mut line = String::new();
        if r.read_line(&mut line).await? == 0 {
            return Ok(None);
        }
        let line = line.trim_end();
        if line.is_empty() {
            if length.is_some() {
                break;
            }
            continue;
        }
        if let Some((k, v)) = line.split_once(':')
            && k.trim().eq_ignore_ascii_case("content-length")
        {
            length = v.trim().parse().ok();
        }
    }
    let mut buf = vec![0u8; length.unwrap_or(0)];
    r.read_exact(&mut buf).await?;
    serde_json::from_slice(&buf).map(Some).map_err(std::io::Error::other)
}

pub async fn write_message<W: AsyncWrite + Unpin>(w: &mut W, msg: &Value) -> std::io::Result<()> {
    let body = serde_json::to_vec(msg).map_err(std::io::Error::other)?;
    w.write_all(format!("Content-Length: {}\r\n\r\n", body.len()).as_bytes()).await?;
    w.write_all(&body).await?;
    w.flush().await
}
