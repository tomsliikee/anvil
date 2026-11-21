//! How Anvil starts a program under the debugger: attach-with-hold.
//!
//! netcoredbg's DAP mode never forwards input to a program it launches
//! (only its command-line mode does), so `Console.ReadLine` would hang
//! forever, and its `--server` mode listens on every network interface.
//! Anvil therefore runs the program itself in the terminal panel, where it
//! has a real console, with a .NET startup hook ([`HOOK_SOURCE`]) that
//! writes the process id to a file and waits until a debugger is attached.
//! netcoredbg then runs over stdin/stdout, attaches, receives the
//! breakpoints, and the hook lets `Main` run. Tested against netcoredbg
//! 3.2.0-1092: breakpoints in `Main` hit and the program reads its input.

use std::path::{Path, PathBuf};

use serde_json::{json, Value};

pub const NETCOREDBG_VERSION: &str = "3.2.0-1092";

pub fn download_url(version: &str) -> String {
    format!("https://github.com/Samsung/netcoredbg/releases/download/{version}/netcoredbg-linux-amd64.tar.gz")
}

/// `~/.local/share/anvil/netcoredbg/<version>/netcoredbg/netcoredbg`.
pub fn binary(home: &Path, version: &str) -> PathBuf {
    install_dir(home, version).join("netcoredbg/netcoredbg")
}

pub fn install_dir(home: &Path, version: &str) -> PathBuf {
    home.join(".local/share/anvil/netcoredbg").join(version)
}

/// Raised when [`HOOK_SOURCE`] changes, so the hook is built again.
pub const HOOK_VERSION: &str = "1";

/// The startup hook's project. `netstandard2.0` loads into every .NET
/// (Core) runtime a course project may target; a Debug build keeps
/// netcoredbg from warning about optimised code.
pub const HOOK_PROJECT: &str = r#"<Project Sdk="Microsoft.NET.Sdk">
  <PropertyGroup>
    <TargetFramework>netstandard2.0</TargetFramework>
    <AssemblyName>AnvilStartupHook</AssemblyName>
    <RootNamespace></RootNamespace>
  </PropertyGroup>
</Project>
"#;

pub const HOOK_SOURCE: &str = r#"using System;
using System.Diagnostics;
using System.IO;
using System.Threading;

/// Started by Anvil's F5: tells Anvil the process id, then waits until the
/// debugger has attached so breakpoints in Main are set before it runs.
internal class StartupHook
{
    public static void Initialize()
    {
        var file = Environment.GetEnvironmentVariable("ANVIL_DEBUG_PID_FILE");
        Environment.SetEnvironmentVariable("DOTNET_STARTUP_HOOKS", null);
        Environment.SetEnvironmentVariable("ANVIL_DEBUG_PID_FILE", null);
        if (string.IsNullOrEmpty(file)) return;
        File.WriteAllText(file + ".tmp", Process.GetCurrentProcess().Id.ToString());
        File.Move(file + ".tmp", file);
        var deadline = DateTime.UtcNow.AddSeconds(30);
        while (!Debugger.IsAttached && DateTime.UtcNow < deadline) Thread.Sleep(10);
    }
}
"#;

/// `~/.local/share/anvil/debug-hook/<HOOK_VERSION>`; the project is built
/// there into `out/`.
pub fn hook_dir(home: &Path) -> PathBuf {
    home.join(".local/share/anvil/debug-hook").join(HOOK_VERSION)
}

pub fn hook_dll(home: &Path) -> PathBuf {
    hook_dir(home).join("out/AnvilStartupHook.dll")
}

/// Where the hook writes the process id for one run.
pub fn pid_file(home: &Path, token: &str) -> PathBuf {
    home.join(".local/share/anvil/run").join(format!("{token}.pid"))
}

/// The environment of the program under F5.
pub fn program_env(hook: &Path, pid_file: &Path) -> Vec<(String, String)> {
    vec![("DOTNET_STARTUP_HOOKS".into(), hook.to_string_lossy().into_owned()), ("ANVIL_DEBUG_PID_FILE".into(), pid_file.to_string_lossy().into_owned())]
}

/// netcoredbg over stdin/stdout.
pub fn adapter_args() -> Vec<String> {
    vec!["--interpreter=vscode".into()]
}

/// DAP `attach` arguments.
pub fn attach_args(pid: u32) -> Value {
    json!({"name": ".NET Attach", "type": "coreclr", "request": "attach", "processId": pid, "justMyCode": true})
}

/// Function breakpoints that stop at the start of the program for F10 and
/// F11 (`<Main>$` is the method of top-level statements).
pub const ENTRY_FUNCTIONS: [&str; 2] = ["Main", "<Main>$"];

/// The built program of an SDK-style project:
/// `bin/Debug/<tfm>/<AssemblyName>.dll`. Used when MSBuild cannot be asked
/// for `TargetPath`.
pub fn output_dll(project_dir: &Path, assembly: &str, tfm: &str) -> PathBuf {
    project_dir.join("bin").join("Debug").join(tfm).join(format!("{assembly}.dll"))
}

/// The last non-empty line of `dotnet msbuild -getProperty:TargetPath`.
pub fn parse_target_path(stdout: &str) -> Option<PathBuf> {
    let line = stdout.lines().map(str::trim).rfind(|l| !l.is_empty())?;
    line.ends_with(".dll").then(|| PathBuf::from(line))
}

/// The MSBuild properties F5 asks for in one call.
pub const PROGRAM_PROPERTIES: [&str; 3] = ["TargetPath", "OutputType", "TargetFrameworks"];

/// What MSBuild says about the startup project's program.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ProgramInfo {
    /// Empty for a multi-targeting project unless one framework is given.
    pub target_path: Option<PathBuf>,
    pub output_type: String,
    pub frameworks: Vec<String>,
}

impl ProgramInfo {
    /// Only an executable can be started; VS refuses a class library
    /// ("A project with an Output Type of Class Library cannot be started
    /// directly").
    pub fn is_startable(&self) -> bool {
        self.output_type.eq_ignore_ascii_case("exe") || self.output_type.eq_ignore_ascii_case("winexe")
    }
}

/// The JSON `dotnet msbuild -getProperty:A -getProperty:B` prints (lines
/// before it, such as warnings, are skipped).
pub fn parse_program_info(stdout: &str) -> Option<ProgramInfo> {
    let json: Value = serde_json::from_str(stdout.get(stdout.find('{')?..)?.trim()).ok()?;
    let props = json.get("Properties")?;
    let s = |k: &str| props.get(k).and_then(Value::as_str).map(str::trim).unwrap_or("").to_owned();
    let target = s("TargetPath");
    Some(ProgramInfo {
        target_path: (!target.is_empty()).then(|| PathBuf::from(target)),
        output_type: s("OutputType"),
        frameworks: s("TargetFrameworks").split(';').map(str::trim).filter(|f| !f.is_empty()).map(str::to_owned).collect(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shapes() {
        let home = Path::new("/h");
        assert_eq!(binary(home, NETCOREDBG_VERSION), Path::new("/h/.local/share/anvil/netcoredbg/3.2.0-1092/netcoredbg/netcoredbg"));
        assert!(download_url(NETCOREDBG_VERSION).ends_with("/3.2.0-1092/netcoredbg-linux-amd64.tar.gz"));
        assert_eq!(hook_dll(home), Path::new("/h/.local/share/anvil/debug-hook/1/out/AnvilStartupHook.dll"));
        let env = program_env(&hook_dll(home), &pid_file(home, "t"));
        assert_eq!(env[0].0, "DOTNET_STARTUP_HOOKS");
        assert_eq!(env[1].1, "/h/.local/share/anvil/run/t.pid");
        assert_eq!(attach_args(42)["processId"], 42);
        assert_eq!(adapter_args(), ["--interpreter=vscode"]);
        assert_eq!(output_dll(Path::new("/p"), "App", "net10.0"), Path::new("/p/bin/Debug/net10.0/App.dll"));
    }

    #[test]
    fn target_path_from_msbuild() {
        assert_eq!(parse_target_path("/p/bin/Debug/net10.0/App.dll\n"), Some(PathBuf::from("/p/bin/Debug/net10.0/App.dll")));
        assert_eq!(parse_target_path("warning: x\n  /p/App.dll  \n\n"), Some(PathBuf::from("/p/App.dll")));
        assert_eq!(parse_target_path("MSBUILD : error MSB1009\n"), None);
    }

    #[test]
    fn program_info_from_msbuild() {
        // As the .NET 10 SDK prints it for a class library and for a
        // multi-targeting console app.
        let lib = "{\n  \"Properties\": {\n    \"TargetPath\": \"/s/\\u00DCbung 1/Lib/bin/Debug/net10.0/Lib.dll\",\n    \"OutputType\": \"Library\",\n    \"TargetFrameworks\": \"\"\n  }\n}\n";
        let info = parse_program_info(lib).unwrap();
        assert_eq!(info.target_path, Some(PathBuf::from("/s/Übung 1/Lib/bin/Debug/net10.0/Lib.dll")));
        assert!(!info.is_startable());
        assert!(info.frameworks.is_empty());
        let multi = "warning: x\n{\"Properties\": {\"TargetPath\": \"\", \"OutputType\": \"Exe\", \"TargetFrameworks\": \"net9.0;net10.0\"}}";
        let info = parse_program_info(multi).unwrap();
        assert_eq!(info.target_path, None);
        assert!(info.is_startable());
        assert_eq!(info.frameworks, ["net9.0", "net10.0"]);
        assert!(ProgramInfo { output_type: "WinExe".into(), ..Default::default() }.is_startable());
        assert_eq!(parse_program_info("MSBUILD : error MSB1009"), None);
    }
}
