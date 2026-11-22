//! Test Explorer data: tests discovered from the structure model (fast,
//! before any build), `dotnet test` results from TRX files, and the
//! project → namespace → class → test tree.

use crate::csharp::{FileStructure, MemberKind};
use crate::xml::Doc;

const TEST_ATTRIBUTES: &[&str] = &["TestMethod", "DataTestMethod", "Fact", "Theory", "Test", "TestCase", "TestCaseSource"];

#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct TestId {
    pub namespace: String,
    pub class: String,
    pub method: String,
}

impl TestId {
    /// The name `dotnet test --filter FullyQualifiedName=…` takes.
    pub fn full_name(&self) -> String {
        if self.namespace.is_empty() { format!("{}.{}", self.class, self.method) } else { format!("{}.{}.{}", self.namespace, self.class, self.method) }
    }
}

/// Test methods of a file: methods carrying an MSTest, xUnit or NUnit test
/// attribute.
pub fn discover(file: &FileStructure) -> Vec<(TestId, usize)> {
    let mut out = Vec::new();
    for (outer, t) in file.all_types() {
        let class = if outer.is_empty() { t.name.clone() } else { format!("{}+{}", outer.join("+"), t.name) };
        for m in t.members.iter().filter(|m| m.kind == MemberKind::Method) {
            if m.attributes.iter().any(|a| TEST_ATTRIBUTES.contains(&a.as_str())) {
                out.push((TestId { namespace: t.namespace.clone(), class: class.clone(), method: m.name.clone() }, m.name_range.start));
            }
        }
    }
    out
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Outcome {
    Passed,
    Failed,
    Skipped,
    NotRun,
}

#[derive(Clone, Debug, PartialEq)]
pub struct TestResult {
    pub id: TestId,
    /// Display name with arguments for data-driven tests.
    pub display: String,
    pub outcome: Outcome,
    pub duration_ms: f64,
    pub message: Option<String>,
    pub stack: Option<String>,
    pub output: Option<String>,
}

fn duration_ms(s: &str) -> f64 {
    // hh:mm:ss.fffffff
    let parts: Vec<f64> = s.split(':').filter_map(|p| p.parse().ok()).collect();
    match parts.as_slice() {
        [h, m, sec] => (h * 3600.0 + m * 60.0 + sec) * 1000.0,
        _ => 0.0,
    }
}

/// Results of a VSTest TRX file (`dotnet test --logger trx`).
pub fn parse_trx(xml: &str) -> Vec<TestResult> {
    let Ok(doc) = Doc::parse(xml.trim_start_matches('\u{feff}')) else { return Vec::new() };
    let mut defs = std::collections::HashMap::new();
    for ut in doc.all("UnitTest") {
        if let (Some(id), Some(tm)) = (doc.attr(ut, "id"), doc.child(ut, "TestMethod")) {
            let class = doc.attr(tm, "className").unwrap_or_default();
            // className may carry ", Assembly" after the type.
            let class = class.split(',').next().unwrap_or(class).trim();
            let (ns, cls) = class.rsplit_once('.').unwrap_or(("", class));
            // NUnit names a TestCase "Div(4,2)"; the tree knows the method.
            let name = doc.attr(tm, "name").unwrap_or_default();
            let method = name.split('(').next().unwrap_or(name).trim();
            defs.insert(id.to_owned(), TestId { namespace: ns.to_owned(), class: cls.to_owned(), method: method.to_owned() });
        }
    }
    doc.all("UnitTestResult")
        .filter_map(|r| {
            let id = defs.get(doc.attr(r, "testId")?)?.clone();
            let outcome = match doc.attr(r, "outcome").unwrap_or("") {
                "Passed" => Outcome::Passed,
                "Failed" | "Error" | "Timeout" | "Aborted" => Outcome::Failed,
                "NotExecuted" | "Inconclusive" | "Skipped" => Outcome::Skipped,
                _ => Outcome::NotRun,
            };
            let output = doc.child(r, "Output");
            let err = output.and_then(|o| doc.child(o, "ErrorInfo"));
            let text = |parent: Option<usize>, name: &str| parent.and_then(|p| doc.child(p, name)).map(|x| doc.inner_text(x)).filter(|s| !s.trim().is_empty());
            Some(TestResult {
                display: doc.attr(r, "testName").unwrap_or(&id.method).to_owned(),
                id,
                outcome,
                duration_ms: doc.attr(r, "duration").map(duration_ms).unwrap_or(0.0),
                message: text(err, "Message"),
                stack: text(err, "StackTrace"),
                output: text(output, "StdOut"),
            })
        })
        .collect()
}

/// One result per test method: the cases of a data-driven test (xUnit
/// Theory, NUnit TestCase, MSTest DataRow) fold into one, failed when any
/// case failed, with every case's outcome listed in the message.
pub fn merge_cases(results: Vec<TestResult>) -> Vec<TestResult> {
    let mut out: Vec<TestResult> = Vec::new();
    let mut cases: std::collections::HashMap<TestId, Vec<TestResult>> = Default::default();
    for r in results {
        cases.entry(r.id.clone()).or_default().push(r);
    }
    let rank = |o: Outcome| match o {
        Outcome::Failed => 3,
        Outcome::Passed => 2,
        Outcome::Skipped => 1,
        Outcome::NotRun => 0,
    };
    for (_, mut v) in cases {
        if v.len() == 1 {
            out.extend(v.pop());
            continue;
        }
        v.sort_by(|a, b| a.display.cmp(&b.display));
        let outcome = v.iter().map(|r| r.outcome).max_by_key(|o| rank(*o)).unwrap_or(Outcome::NotRun);
        let mut message = String::new();
        let mut stack = String::new();
        for r in &v {
            let mark = match r.outcome {
                Outcome::Passed => "Passed",
                Outcome::Failed => "Failed",
                _ => "Skipped",
            };
            message.push_str(&format!("{mark}  {}\n", r.display));
            if let Some(m) = &r.message {
                message.push_str(&format!("    {}\n", m.trim().replace('\n', "\n    ")));
            }
            if let Some(st) = &r.stack {
                stack.push_str(&format!("{}:\n{}\n", r.display, st.trim_end()));
            }
        }
        let first = &v[0];
        out.push(TestResult {
            id: first.id.clone(),
            display: format!("{} ({} cases)", first.id.method, v.len()),
            outcome,
            duration_ms: v.iter().map(|r| r.duration_ms).sum(),
            message: Some(message.trim_end().to_owned()),
            stack: (!stack.is_empty()).then_some(stack),
            output: None,
        });
    }
    out.sort_by(|a, b| a.id.cmp(&b.id));
    out
}

/// A result line of VSTest's console logger (`--logger
/// "console;verbosity=normal"`), printed as each test ends: `  Passed
/// N.C.M [12 ms]`. xUnit prints the full name, NUnit and MSTest only the
/// test name, maybe with its arguments. Returns outcome, name without
/// arguments, and milliseconds.
pub fn parse_progress(line: &str) -> Option<(Outcome, String, f64)> {
    let l = line.strip_prefix("  ")?;
    let (outcome, rest) = if let Some(r) = l.strip_prefix("Passed ") {
        (Outcome::Passed, r)
    } else if let Some(r) = l.strip_prefix("Failed ") {
        (Outcome::Failed, r)
    } else {
        (Outcome::Skipped, l.strip_prefix("Skipped ")?)
    };
    let (name, time) = rest.rsplit_once(" [")?;
    let time = time.strip_suffix(']')?;
    let ms = if time.starts_with('<') {
        0.0
    } else if let Some(n) = time.strip_suffix(" ms") {
        n.trim().parse().ok()?
    } else if let Some(n) = time.strip_suffix(" s") {
        n.trim().parse::<f64>().ok()? * 1000.0
    } else {
        time.strip_suffix(" m")?.trim().parse::<f64>().ok()? * 60_000.0
    };
    let name = name.split('(').next().unwrap_or(name).trim();
    (!name.is_empty() && !name.contains(' ')).then(|| (outcome, name.to_owned(), ms))
}

/// A source location in a .NET stack frame: `at N.C.M() in /p/F.cs:line
/// 20` (VSTest) or `/p/F.cs(20,0): at …` (xUnit's own output).
pub fn stack_location(frame: &str) -> Option<(String, u32)> {
    if let Some((_, rest)) = frame.rsplit_once(" in ")
        && let Some((path, line)) = rest.rsplit_once(":line ")
    {
        return Some((path.trim().to_owned(), line.trim().parse().ok()?));
    }
    let t = frame.trim();
    let (head, _) = t.split_once("): at ")?;
    let (path, pos) = head.rsplit_once('(')?;
    Some((path.to_owned(), pos.split(',').next()?.parse().ok()?))
}

/// Whether a project runs its tests on Microsoft.Testing.Platform instead
/// of VSTest (changes the `dotnet test` arguments).
pub fn uses_testing_platform(csproj: &crate::csproj::Csproj) -> bool {
    let yes = |p: &str| csproj.property(p).is_some_and(|v| v.eq_ignore_ascii_case("true"));
    yes("TestingPlatformDotnetTestSupport") || yes("UseMicrosoftTestingPlatformRunner") || yes("EnableMSTestRunner") || yes("EnableNUnitRunner") || yes("UseTestingPlatformProtocol")
}

/// `dotnet test` arguments for running `filter` (all tests when empty)
/// with a TRX result file in `results_dir`.
pub fn test_args(project: &str, filter: &[TestId], results_dir: &str, mtp: bool) -> Vec<String> {
    let mut a = vec!["test".to_owned(), project.to_owned(), "--nologo".to_owned()];
    // NUnit's TestCases carry their arguments in the name ("N.C.M(1,2)"),
    // which `=` does not match; `~N.C.M\(` does, without catching "M2".
    let expr = filter.iter().map(|t| format!("FullyQualifiedName={0}|FullyQualifiedName~{0}\\(", t.full_name())).collect::<Vec<_>>().join("|");
    if mtp {
        a.extend(["--results-directory".into(), results_dir.into(), "--report-trx".into()]);
        if !expr.is_empty() {
            a.extend(["--filter".into(), expr]);
        }
    } else {
        // The console logger at normal verbosity prints a line per finished
        // test, which the Test Explorer turns into live progress.
        a.extend(["--logger".into(), "trx".into(), "--logger".into(), "console;verbosity=normal".into(), "--results-directory".into(), results_dir.into()]);
        if !expr.is_empty() {
            a.extend(["--filter".into(), expr]);
        }
    }
    a
}

#[cfg(test)]
mod tests {
    use super::*;

    const TRX: &str = r#"<?xml version="1.0" encoding="utf-8"?>
<TestRun id="1" xmlns="http://microsoft.com/schemas/VisualStudio/TeamTest/2010">
  <Results>
    <UnitTestResult executionId="e1" testId="t1" testName="AddLast_AppendsAtEnd" duration="00:00:00.0123000" outcome="Passed" />
    <UnitTestResult executionId="e2" testId="t2" testName="Remove_Missing" duration="00:00:01.5000000" outcome="Failed">
      <Output>
        <ErrorInfo>
          <Message>Assert.AreEqual failed. Expected:&lt;2&gt;. Actual:&lt;3&gt;.</Message>
          <StackTrace>   at Course.Tests.ListTests.Remove_Missing() in /x/ListTests.cs:line 20</StackTrace>
        </ErrorInfo>
      </Output>
    </UnitTestResult>
  </Results>
  <TestDefinitions>
    <UnitTest name="AddLast_AppendsAtEnd" id="t1"><TestMethod codeBase="x.dll" className="Course.Tests.ListTests" name="AddLast_AppendsAtEnd" /></UnitTest>
    <UnitTest name="Remove_Missing" id="t2"><TestMethod codeBase="x.dll" className="Course.Tests.ListTests, Course.Tests" name="Remove_Missing" /></UnitTest>
  </TestDefinitions>
</TestRun>"#;

    #[test]
    fn parses_trx() {
        let r = parse_trx(TRX);
        assert_eq!(r.len(), 2);
        assert_eq!(r[0].id.full_name(), "Course.Tests.ListTests.AddLast_AppendsAtEnd");
        assert_eq!(r[0].outcome, Outcome::Passed);
        assert!((r[0].duration_ms - 12.3).abs() < 1e-6);
        assert_eq!(r[1].outcome, Outcome::Failed);
        assert_eq!(r[1].id.class, "ListTests");
        assert_eq!(r[1].message.as_deref(), Some("Assert.AreEqual failed. Expected:<2>. Actual:<3>."));
        assert!(r[1].stack.as_deref().unwrap().contains("line 20"));
    }

    #[test]
    fn discovers_mstest_xunit_nunit() {
        let f = crate::csharp::parse("namespace T\n{\n    [TestClass]\n    public class A\n    {\n        [TestMethod]\n        public void One() {}\n        [Fact] public void Two() {}\n        [NUnit.Framework.Test] public void Three() {}\n        public void Helper() {}\n        [DataTestMethod, DataRow(1)] public void Four(int x) {}\n    }\n}");
        let names: Vec<String> = discover(&f).into_iter().map(|(t, _)| t.full_name()).collect();
        assert_eq!(names, ["T.A.One", "T.A.Two", "T.A.Three", "T.A.Four"]);
    }

    #[test]
    fn data_driven_cases_fold_into_their_method() {
        let trx = r#"<TestRun><Results>
<UnitTestResult testId="a" testName="Div_Cases(4,2,2)" duration="00:00:00.0040000" outcome="Passed" />
<UnitTestResult testId="b" testName="Div_Cases(9,3,4)" duration="00:00:00.0060000" outcome="Failed"><Output><ErrorInfo><Message>Expected: 4 But was: 3</Message><StackTrace>   at Lib.NUnitTests.DivTests.Div_Cases(Int32 a) in /p/UnitTest1.cs:line 21</StackTrace></ErrorInfo></Output></UnitTestResult>
<UnitTestResult testId="c" testName="Div_Works" duration="00:00:00.0010000" outcome="Passed" />
</Results><TestDefinitions>
<UnitTest id="a"><TestMethod className="Lib.NUnitTests.DivTests" name="Div_Cases(4,2,2)" /></UnitTest>
<UnitTest id="b"><TestMethod className="Lib.NUnitTests.DivTests" name="Div_Cases(9,3,4)" /></UnitTest>
<UnitTest id="c"><TestMethod className="Lib.NUnitTests.DivTests" name="Div_Works" /></UnitTest>
</TestDefinitions></TestRun>"#;
        let r = merge_cases(parse_trx(trx));
        assert_eq!(r.len(), 2);
        assert_eq!(r[0].id.full_name(), "Lib.NUnitTests.DivTests.Div_Cases");
        assert_eq!(r[0].outcome, Outcome::Failed);
        assert!((r[0].duration_ms - 10.0).abs() < 1e-6);
        assert!(r[0].message.as_deref().unwrap().contains("Failed  Div_Cases(9,3,4)"));
        assert!(r[0].stack.as_deref().unwrap().contains("line 21"));
        assert_eq!(r[1].outcome, Outcome::Passed);
    }

    #[test]
    fn console_progress_lines() {
        assert_eq!(parse_progress("  Passed Lib.XunitTests.CalcTests.Add_Works [< 1 ms]"), Some((Outcome::Passed, "Lib.XunitTests.CalcTests.Add_Works".into(), 0.0)));
        assert_eq!(parse_progress("  Failed Div_Cases(9,3,4) [67 ms]"), Some((Outcome::Failed, "Div_Cases".into(), 67.0)));
        assert_eq!(parse_progress("  Failed Lib.XunitTests.CalcTests.Add_Theory(a: 2, b: 2, sum: 5) [20 ms]"), Some((Outcome::Failed, "Lib.XunitTests.CalcTests.Add_Theory".into(), 20.0)));
        assert_eq!(parse_progress("  Passed Rows (1,2,3) [16 ms]"), Some((Outcome::Passed, "Rows".into(), 16.0)));
        assert_eq!(parse_progress("  Passed Lib.XunitTests.CalcTests.Slow [1 s]"), Some((Outcome::Passed, "Lib.XunitTests.CalcTests.Slow".into(), 1000.0)));
        assert_eq!(parse_progress("  Skipped Lib.XunitTests.CalcTests.Skipped [1 ms]"), Some((Outcome::Skipped, "Lib.XunitTests.CalcTests.Skipped".into(), 1.0)));
        assert_eq!(parse_progress("     Passed: 2"), None);
        assert_eq!(parse_progress("Passed!  - Failed: 0, Passed: 1"), None);
    }

    #[test]
    fn stack_locations() {
        assert_eq!(stack_location("   at Lib.Calc.Div(Int32 a, Int32 b) in /p/Lib/Class1.cs:line 7"), Some(("/p/Lib/Class1.cs".into(), 7)));
        assert_eq!(stack_location("/p/UnitTest1.cs(22,0): at Lib.XunitTests.CalcTests.Add_Theory(Int32 a)"), Some(("/p/UnitTest1.cs".into(), 22)));
        assert_eq!(stack_location("   at System.Reflection.MethodBaseInvoker.InvokeWithNoArgs(Object obj)"), None);
    }

    #[test]
    fn args_for_vstest_and_mtp() {
        let t = TestId { namespace: "N".into(), class: "C".into(), method: "M".into() };
        let a = test_args("p.csproj", std::slice::from_ref(&t), "/r", false);
        assert_eq!(a, ["test", "p.csproj", "--nologo", "--logger", "trx", "--logger", "console;verbosity=normal", "--results-directory", "/r", "--filter", "FullyQualifiedName=N.C.M|FullyQualifiedName~N.C.M\\("]);
        assert!(test_args("p.csproj", &[], "/r", true).contains(&"--report-trx".to_owned()));
    }
}
