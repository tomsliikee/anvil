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
            defs.insert(id.to_owned(), TestId { namespace: ns.to_owned(), class: cls.to_owned(), method: doc.attr(tm, "name").unwrap_or_default().to_owned() });
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
    let expr = filter.iter().map(|t| format!("FullyQualifiedName={}", t.full_name())).collect::<Vec<_>>().join("|");
    if mtp {
        a.extend(["--results-directory".into(), results_dir.into(), "--report-trx".into()]);
        if !expr.is_empty() {
            a.extend(["--filter".into(), expr]);
        }
    } else {
        a.extend(["--logger".into(), "trx".into(), "--results-directory".into(), results_dir.into()]);
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
    fn args_for_vstest_and_mtp() {
        let t = TestId { namespace: "N".into(), class: "C".into(), method: "M".into() };
        let a = test_args("p.csproj", std::slice::from_ref(&t), "/r", false);
        assert_eq!(a, ["test", "p.csproj", "--nologo", "--logger", "trx", "--results-directory", "/r", "--filter", "FullyQualifiedName=N.C.M"]);
        assert!(test_args("p.csproj", &[], "/r", true).contains(&"--report-trx".to_owned()));
    }
}
