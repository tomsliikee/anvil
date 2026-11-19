//! A Language Server Protocol client for Microsoft's Roslyn C# server,
//! over the server's stdin and stdout. The app spawns the process (on the
//! host) and hands the pipes to [`client::Client::start`].

pub mod client;
pub mod position;
pub mod roslyn;
pub mod rpc;

pub use lsp_types;
