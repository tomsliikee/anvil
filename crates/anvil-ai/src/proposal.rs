//! Edit proposals from a chat answer: every ```diff block becomes one
//! patch per file, which the app shows hunk by hunk; nothing is applied
//! without the user's review.

use anvil_core::diff::{self, FilePatch};

/// The patches in an answer, in order. Blocks that are not unified diffs
/// are ignored.
pub fn patches(answer: &str) -> Vec<FilePatch> {
    let mut out = Vec::new();
    let mut rest = answer;
    while let Some(i) = rest.find("```diff") {
        let body = &rest[i + 7..];
        let body = body.strip_prefix('\n').unwrap_or(body);
        let end = body.find("```").unwrap_or(body.len());
        out.extend(diff::parse_unified(&body[..end]).into_iter().filter(|p| !p.hunks.is_empty()));
        rest = &body[end.min(body.len())..];
        if let Some(r) = rest.strip_prefix("```") {
            rest = r;
        }
    }
    out
}

/// The answer's prose without the diff blocks (shown above the review).
pub fn prose(answer: &str) -> String {
    let mut out = String::new();
    let mut rest = answer;
    while let Some(i) = rest.find("```diff") {
        out.push_str(&rest[..i]);
        let after = &rest[i + 7..];
        rest = after.find("```").map_or("", |e| &after[e + 3..]);
    }
    out.push_str(rest);
    out.trim().to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_patches_and_prose() {
        let answer = "Rename the field:\n\n```diff\n--- a/Person.cs\n+++ b/Person.cs\n@@ -5,1 +5,1 @@\n-        private string firstName;\n+        private string givenName;\n```\n\nDone.";
        let p = patches(answer);
        assert_eq!(p.len(), 1);
        assert_eq!(p[0].path(), Some("Person.cs"));
        assert_eq!(prose(answer), "Rename the field:\n\n\n\nDone.");
        assert!(patches("no code here").is_empty());
    }
}
