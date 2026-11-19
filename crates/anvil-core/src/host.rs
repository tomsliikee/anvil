//! Command lines for the host. Inside the Flatpak every external program
//! (dotnet, Roslyn, netcoredbg, git, the user's program) runs on the host
//! through `flatpak-spawn --host`, so the user's SDK, NuGet cache and git
//! configuration are the ones the terminal uses.

use std::path::PathBuf;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Command {
    pub program: String,
    pub args: Vec<String>,
    pub cwd: Option<PathBuf>,
    pub env: Vec<(String, String)>,
}

impl Command {
    pub fn new(program: impl Into<String>) -> Self {
        Command { program: program.into(), ..Default::default() }
    }

    pub fn arg(mut self, a: impl Into<String>) -> Self {
        self.args.push(a.into());
        self
    }

    pub fn args<I, S>(mut self, a: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.args.extend(a.into_iter().map(Into::into));
        self
    }

    pub fn cwd(mut self, dir: impl Into<PathBuf>) -> Self {
        self.cwd = Some(dir.into());
        self
    }

    pub fn env(mut self, k: impl Into<String>, v: impl Into<String>) -> Self {
        self.env.push((k.into(), v.into()));
        self
    }

    /// `dotnet` with the environment Anvil always sets (no telemetry
    /// banner, no logo, English messages so the parsers understand them).
    pub fn dotnet() -> Self {
        Command::new("dotnet").env("DOTNET_CLI_TELEMETRY_OPTOUT", "1").env("DOTNET_NOLOGO", "1").env("DOTNET_CLI_UI_LANGUAGE", "en")
    }
}

/// The argv to execute. In the sandbox the program is wrapped in
/// `flatpak-spawn --host`, which takes the folder and the environment as
/// options; every argument stays one argument (no shell in between).
pub fn wrap(cmd: &Command, in_flatpak: bool) -> Vec<String> {
    if !in_flatpak {
        let mut v = vec![cmd.program.clone()];
        v.extend(cmd.args.iter().cloned());
        return v;
    }
    let mut v = vec!["flatpak-spawn".to_owned(), "--host".to_owned(), "--watch-bus".to_owned()];
    if let Some(dir) = &cmd.cwd {
        v.push(format!("--directory={}", dir.display()));
    }
    for (k, val) in &cmd.env {
        v.push(format!("--env={k}={val}"));
    }
    v.push(cmd.program.clone());
    v.extend(cmd.args.iter().cloned());
    v
}

/// Quotes one word for a POSIX shell (for the terminal panel, which runs
/// a command line through the user's shell).
pub fn sh_quote(s: &str) -> String {
    if !s.is_empty() && s.chars().all(|c| c.is_ascii_alphanumeric() || "-_./=:,+@%".contains(c)) {
        s.to_owned()
    } else {
        format!("'{}'", s.replace('\'', "'\\''"))
    }
}

pub fn sh_line(cmd: &Command) -> String {
    let mut parts: Vec<String> = cmd.env.iter().map(|(k, v)| format!("{k}={}", sh_quote(v))).collect();
    parts.push(sh_quote(&cmd.program));
    parts.extend(cmd.args.iter().map(|a| sh_quote(a)));
    parts.join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn host_command_passes_args_unquoted() {
        let c = Command::new("dotnet").args(["build", "/home/t/Übung 1/A.slnx"]).cwd("/home/t/Übung 1").env("A", "b c");
        assert_eq!(wrap(&c, false), ["dotnet", "build", "/home/t/Übung 1/A.slnx"]);
        assert_eq!(
            wrap(&c, true),
            ["flatpak-spawn", "--host", "--watch-bus", "--directory=/home/t/Übung 1", "--env=A=b c", "dotnet", "build", "/home/t/Übung 1/A.slnx"]
        );
    }

    #[test]
    fn shell_quoting() {
        assert_eq!(sh_quote("abc"), "abc");
        assert_eq!(sh_quote("a b"), "'a b'");
        assert_eq!(sh_quote("it's"), "'it'\\''s'");
        assert_eq!(sh_line(&Command::new("dotnet").arg("run").env("X", "1")), "X=1 dotnet run");
    }
}
