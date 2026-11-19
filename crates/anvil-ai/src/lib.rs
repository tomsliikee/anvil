//! Local AI through Ollama (localhost only; nothing leaves the machine):
//! fill-in-the-middle completion for ghost text and a chat whose edit
//! proposals come back as patches the user reviews.

pub mod context;
pub mod ollama;
pub mod proposal;
