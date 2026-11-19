//! The pure core of Anvil: every file format Visual Studio writes, the C#
//! structure model, the diagram-to-code planner, templates, the keymap and
//! the hand-in ZIP. Nothing here touches GTK or a process, so all of it is
//! tested on the host with `cargo test -p anvil-core`.

pub mod diagnostics;
pub mod diff;
pub mod edit;
pub mod handin;
pub mod hashcode;
pub mod highlight;
pub mod host;
pub mod ignore;
pub mod keymap;
pub mod launch;
pub mod layout;
pub mod nuget;
pub mod planner;
pub mod session;
pub mod text;
pub mod tokens;
pub mod xml;
pub mod cd;
pub mod commands;
pub mod csharp;
pub mod csproj;
pub mod sln;
pub mod slnx;
pub mod snippets;
pub mod spring;
pub mod solution;
pub mod templates;
pub mod testing;
