//! Diagnostics from `dotnet build` / MSBuild console output
//! (`file(line,col): warning SA1633: message [project]`).

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Severity {
    Error,
    Warning,
    Info,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Diagnostic {
    pub file: String,
    /// 1-based, as MSBuild and the Error List show them.
    pub line: u32,
    pub col: u32,
    pub severity: Severity,
    pub code: String,
    pub message: String,
    pub project: Option<String>,
}

impl Diagnostic {
    /// StyleCop rules (`SA…`, `SX…`) for the Error List filter.
    pub fn is_stylecop(&self) -> bool {
        (self.code.starts_with("SA") || self.code.starts_with("SX")) && self.code[2..].chars().all(|c| c.is_ascii_digit())
    }

    /// The rule's documentation link, cut from StyleCop's message.
    pub fn help_link(&self) -> Option<&str> {
        let start = self.message.rfind("(https://")?;
        self.message[start + 1..].strip_suffix(')')
    }

    /// The message without the trailing documentation link.
    pub fn text(&self) -> &str {
        match self.message.rfind(" (https://") {
            Some(i) if self.message.ends_with(')') => &self.message[..i],
            _ => &self.message,
        }
    }
}

/// Parses one line; `None` for lines that are not diagnostics.
pub fn parse_line(line: &str) -> Option<Diagnostic> {
    let line = line.trim();
    // The project in brackets at the end.
    let (body, project) = match line.rfind(" [") {
        Some(i) if line.ends_with(']') => (&line[..i], Some(line[i + 2..line.len() - 1].to_owned())),
        _ => (line, None),
    };
    for (marker, severity) in [(": error ", Severity::Error), (": warning ", Severity::Warning), (": info ", Severity::Info), (": message ", Severity::Info)] {
        let Some(i) = body.find(marker) else { continue };
        let location = &body[..i];
        let rest = &body[i + marker.len()..];
        let (code, message) = rest.split_once(": ").unwrap_or(("", rest));
        let (file, line_no, col) = match location.rfind('(') {
            Some(p) if location.ends_with(')') => {
                let nums: Vec<u32> = location[p + 1..location.len() - 1].split(',').filter_map(|n| n.trim().parse().ok()).collect();
                (location[..p].trim().to_owned(), nums.first().copied().unwrap_or(0), nums.get(1).copied().unwrap_or(0))
            }
            _ => (location.trim().to_owned(), 0, 0),
        };
        if code.contains(' ') {
            return None;
        }
        return Some(Diagnostic { file, line: line_no, col, severity, code: code.to_owned(), message: message.to_owned(), project });
    }
    None
}

/// All diagnostics of a build log, without the repetitions MSBuild prints
/// in its summary, in first-seen order.
pub fn parse_log(log: &str) -> Vec<Diagnostic> {
    let mut seen = std::collections::HashSet::new();
    log.lines().filter_map(parse_line).filter(|d| seen.insert(d.clone())).collect()
}

/// A place in a file named in output text: MSBuild's `File.cs(12,5)` and
/// .NET stack traces' `in /w/File.cs:line 12`. `start..end` are byte
/// offsets of the link text in the line.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Location {
    pub start: usize,
    pub end: usize,
    pub file: String,
    pub line: u32,
    pub col: u32,
}

/// File names a link may point at; anything else in parentheses (a
/// method call in a log message) is not a place.
const SOURCE_EXTENSIONS: &[&str] = &[".cs", ".csproj", ".props", ".targets", ".razor", ".cshtml", ".xaml", ".resx", ".slnx", ".sln", ".json", ".config", ".vb", ".fs"];

fn is_path_char(c: char) -> bool {
    c.is_alphanumeric() || matches!(c, '/' | '\\' | '.' | '_' | '-' | '+' | '~' | ':' | '@')
}

/// Every place in `line`, in order.
pub fn locations(line: &str) -> Vec<Location> {
    let mut out = Vec::new();
    // `path(line,col)` and `path(line)`.
    for (open, _) in line.match_indices('(') {
        let Some(close) = line[open..].find(')').map(|c| open + c) else { continue };
        let nums: Vec<Option<u32>> = line[open + 1..close].split(',').map(|n| n.trim().parse().ok()).collect();
        if nums.is_empty() || nums.len() > 4 || nums.iter().any(Option::is_none) {
            continue;
        }
        let start = line[..open].rfind(|c: char| !is_path_char(c)).map_or(0, |i| i + line[i..].chars().next().map_or(1, char::len_utf8));
        let file = &line[start..open];
        if !SOURCE_EXTENSIONS.iter().any(|e| file.to_ascii_lowercase().ends_with(e)) {
            continue;
        }
        out.push(Location { start, end: close + 1, file: file.to_owned(), line: nums[0].unwrap_or(0), col: nums.get(1).copied().flatten().unwrap_or(0) });
    }
    // ` in /w/File.cs:line 12` (exceptions and failed tests).
    for (at, _) in line.match_indices(":line ") {
        let start = line[..at].rfind(" in ").map(|i| i + 4);
        let digits: String = line[at + 6..].chars().take_while(char::is_ascii_digit).collect();
        if let (Some(start), Ok(n)) = (start, digits.parse::<u32>()) {
            let file = &line[start..at];
            if SOURCE_EXTENSIONS.iter().any(|e| file.to_ascii_lowercase().ends_with(e)) {
                out.push(Location { start, end: at + 6 + digits.len(), file: file.to_owned(), line: n, col: 0 });
            }
        }
    }
    out.sort_by_key(|l| l.start);
    out
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Counts {
    pub errors: usize,
    pub warnings: usize,
    pub messages: usize,
}

pub fn counts(diags: &[Diagnostic]) -> Counts {
    let mut c = Counts::default();
    for d in diags {
        match d.severity {
            Severity::Error => c.errors += 1,
            Severity::Warning => c.warnings += 1,
            Severity::Info => c.messages += 1,
        }
    }
    c
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(name: &str) -> String {
        std::fs::read_to_string(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/dotnet").join(name)).unwrap()
    }

    #[test]
    fn parses_stylecop_warning() {
        let d = parse_line("/work/course/doublyLinkedList/ListNode.cs(1,1): warning SA1633: The file header is missing or not located at the top of the file. (https://github.com/DotNetAnalyzers/StyleCopAnalyzers/blob/master/documentation/SA1633.md) [/work/course/doublyLinkedList/doublyLinkedList.csproj]").unwrap();
        assert_eq!(d.file, "/work/course/doublyLinkedList/ListNode.cs");
        assert_eq!((d.line, d.col), (1, 1));
        assert_eq!(d.severity, Severity::Warning);
        assert_eq!(d.code, "SA1633");
        assert!(d.is_stylecop());
        assert_eq!(d.text(), "The file header is missing or not located at the top of the file.");
        assert_eq!(d.help_link(), Some("https://github.com/DotNetAnalyzers/StyleCopAnalyzers/blob/master/documentation/SA1633.md"));
        assert_eq!(d.project.as_deref(), Some("/work/course/doublyLinkedList/doublyLinkedList.csproj"));
    }

    #[test]
    fn course_build_counts() {
        let diags = parse_log(&fixture("build-warnings.txt"));
        let c = counts(&diags);
        assert_eq!(c.errors, 0);
        assert_eq!(c.warnings, 45, "each warning once although MSBuild prints it twice");
        assert_eq!(diags.iter().filter(|d| d.code == "SA1633").count(), 5);
        assert_eq!(diags.iter().filter(|d| d.is_stylecop()).count(), 40);
        let errors = parse_log(&fixture("build-errors.txt"));
        assert_eq!(counts(&errors).errors, 6);
        assert_eq!(errors[0].code, "CS1061");
    }

    #[test]
    fn ignores_other_lines() {
        assert!(parse_line("  Determining projects to restore...").is_none());
        assert!(parse_line("Time Elapsed 00:00:04.01").is_none());
        let d = parse_line("CSC : error CS2001: Source file 'x.cs' could not be found.").unwrap();
        assert_eq!(d.file, "CSC");
        assert_eq!(d.line, 0);
    }

    #[test]
    fn finds_locations() {
        let l = locations("/w/App/Program.cs(12,5): error CS1002: ; expected [/w/App/App.csproj]");
        assert_eq!(l.len(), 1);
        assert_eq!((l[0].file.as_str(), l[0].line, l[0].col, l[0].start), ("/w/App/Program.cs", 12, 5, 0));
        assert_eq!(&"/w/App/Program.cs(12,5): x"[l[0].start..l[0].end], "/w/App/Program.cs(12,5)");
        let t = locations("   at App.Program.Main(String[] args) in /w/App/Program.cs:line 7");
        assert_eq!(t.len(), 1, "Main(String[] args) is not a place");
        assert_eq!((t[0].file.as_str(), t[0].line), ("/w/App/Program.cs", 7));
        assert!(locations("Console.WriteLine(42)").is_empty());
        assert!(locations("Time Elapsed 00:00:04.01").is_empty());
        let rel = locations("  Gen1.cs(3) : warning");
        assert_eq!((rel[0].file.as_str(), rel[0].line, rel[0].col), ("Gen1.cs", 3, 0));
    }
}
