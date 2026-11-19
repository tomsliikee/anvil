//! How the variable windows show values, as Visual Studio does: a
//! collection shows `Count = 3` instead of its type name, and a value that
//! changed since the last stop is marked.

use std::collections::HashMap;

use crate::client::Variable;

/// The expression whose value replaces an object's `{Type}` text: the
/// count of a generic or `System.Collections` collection.
pub fn count_expression(v: &Variable) -> Option<String> {
    let ty = v.ty.as_deref()?;
    let expr = v.evaluate_name.as_deref()?;
    let is_object_text = v.value == format!("{{{ty}}}");
    let collection_like = ty.contains('<') || ty.starts_with("System.Collections.");
    (v.variables_reference > 0 && is_object_text && collection_like).then(|| format!("({expr}).Count"))
}

/// The text shown for a counted collection.
pub fn with_count(count: &str) -> String {
    format!("Count = {count}")
}

/// Values of the last stop by expression, to mark what changed.
#[derive(Clone, Debug, Default)]
pub struct Snapshot(HashMap<String, String>);

impl Snapshot {
    /// Records `value` for `key` and says whether it differs from the
    /// value at the previous stop (a new variable is not "changed").
    pub fn update(&mut self, key: &str, value: &str) -> bool {
        match self.0.insert(key.to_owned(), value.to_owned()) {
            Some(old) => old != value,
            None => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn var(name: &str, value: &str, ty: &str, reference: i64) -> Variable {
        Variable { name: name.into(), value: value.into(), ty: Some(ty.into()), variables_reference: reference, evaluate_name: Some(name.into()) }
    }

    #[test]
    fn collections_are_counted() {
        // As netcoredbg 3.2 reports a List<string> local.
        let list = var("names", "{System.Collections.Generic.List<string>}", "System.Collections.Generic.List<string>", 2);
        assert_eq!(count_expression(&list).as_deref(), Some("(names).Count"));
        // Arrays already show their length; plain objects keep their type.
        assert_eq!(count_expression(&var("args", "{string[0]}", "string[]", 0)), None);
        assert_eq!(count_expression(&var("p", "{Demo.Person}", "Demo.Person", 3)), None);
        assert_eq!(count_expression(&var("n", "3", "int", 0)), None);
        assert_eq!(with_count("5"), "Count = 5");
    }

    #[test]
    fn changes_between_stops() {
        let mut s = Snapshot::default();
        assert!(!s.update("total", "0"));
        assert!(!s.update("total", "0"));
        assert!(s.update("total", "3"));
        assert!(!s.update("i", "1"));
    }
}
