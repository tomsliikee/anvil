//! Visual Studio's C# code snippets, expanded with Tab after the shortcut.
//! Placeholders (`$type$`) become linked tab stops with VS' defaults;
//! `$end$` is where the caret lands, `$selected$` takes the selection
//! (Surround With).

use std::ops::Range;

pub struct SnippetDef {
    pub shortcut: &'static str,
    pub title: &'static str,
    /// Lines joined with `\n`; indentation is four spaces per level and is
    /// re-based on the line the snippet is inserted into.
    pub body: &'static str,
    /// Placeholder defaults; `classname` is filled from the context.
    pub defaults: &'static [(&'static str, &'static str)],
    pub surrounds: bool,
}

macro_rules! snip {
    ($s:literal, $t:literal, $b:literal, [$(($k:literal, $v:literal)),*], $sur:literal) => {
        SnippetDef { shortcut: $s, title: $t, body: $b, defaults: &[$(($k, $v)),*], surrounds: $sur }
    };
}

pub const SNIPPETS: &[SnippetDef] = &[
    snip!("prop", "Property", "public $type$ $property$ { get; set; }$end$", [("type", "int"), ("property", "MyProperty")], false),
    snip!("propg", "Property with private set", "public $type$ $property$ { get; private set; }$end$", [("type", "int"), ("property", "MyProperty")], false),
    snip!("propfull", "Property with backing field", "private $type$ $field$;\n\npublic $type$ $property$\n{\n    get { return $field$; }\n    set { $field$ = value; }\n}\n$end$", [("type", "int"), ("field", "myVar"), ("property", "MyProperty")], false),
    snip!("ctor", "Constructor", "public $classname$()\n{\n    $end$\n}", [], false),
    snip!("~", "Destructor", "~$classname$()\n{\n    $end$\n}", [], false),
    snip!("for", "for loop", "for (int $index$ = 0; $index$ < $max$; $index$++)\n{\n    $selected$$end$\n}", [("index", "i"), ("max", "length")], true),
    snip!("forr", "Reverse for loop", "for (int $index$ = $max$ - 1; $index$ >= 0; $index$--)\n{\n    $selected$$end$\n}", [("index", "i"), ("max", "length")], true),
    snip!("foreach", "foreach loop", "foreach ($type$ $identifier$ in $collection$)\n{\n    $selected$$end$\n}", [("type", "var"), ("identifier", "item"), ("collection", "collection")], true),
    snip!("while", "while loop", "while ($expression$)\n{\n    $selected$$end$\n}", [("expression", "true")], true),
    snip!("do", "do...while loop", "do\n{\n    $selected$$end$\n} while ($expression$);", [("expression", "true")], true),
    snip!("if", "if statement", "if ($expression$)\n{\n    $selected$$end$\n}", [("expression", "true")], true),
    snip!("else", "else statement", "else\n{\n    $selected$$end$\n}", [], true),
    snip!("switch", "switch statement", "switch ($expression$)\n{\n    $cases$\n    default:\n        break;\n}$end$", [("expression", "switch_on"), ("cases", "")], false),
    snip!("try", "try catch", "try\n{\n    $selected$\n}\ncatch ($expression$)\n{\n    $end$\n    throw;\n}", [("expression", "Exception")], true),
    snip!("tryf", "try finally", "try\n{\n    $selected$\n}\nfinally\n{\n    $end$\n}", [], true),
    snip!("using", "using statement", "using ($resource$)\n{\n    $selected$$end$\n}", [("resource", "resource")], true),
    snip!("lock", "lock statement", "lock ($expression$)\n{\n    $selected$$end$\n}", [("expression", "this")], true),
    snip!("checked", "checked block", "checked\n{\n    $selected$$end$\n}", [], true),
    snip!("unchecked", "unchecked block", "unchecked\n{\n    $selected$$end$\n}", [], true),
    snip!("cw", "Console.WriteLine", "Console.WriteLine($selected$$end$);", [], false),
    snip!("svm", "static void Main", "static void Main(string[] args)\n{\n    $end$\n}", [], false),
    snip!("sim", "static int Main", "static int Main(string[] args)\n{\n    $end$\n    return 0;\n}", [], false),
    snip!("class", "class", "class $name$\n{\n    $selected$$end$\n}", [("name", "MyClass")], true),
    snip!("interface", "interface", "interface I$name$\n{\n    $selected$$end$\n}", [("name", "Interface")], true),
    snip!("enum", "enum", "enum $name$\n{\n    $selected$$end$\n}", [("name", "MyEnum")], true),
    snip!("struct", "struct", "struct $name$\n{\n    $selected$$end$\n}", [("name", "MyStruct")], true),
    snip!("namespace", "namespace", "namespace $name$\n{\n    $selected$$end$\n}", [("name", "MyNamespace")], true),
    snip!("indexer", "Indexer", "public $type$ this[$indextype$ index]\n{\n    get { $end$ /* return the specified index here */ }\n    set { /* set the specified index to value here */ }\n}", [("type", "object"), ("indextype", "int")], false),
    snip!("exception", "Exception", "[Serializable]\npublic class $newException$Exception : $base$\n{\n    public $newException$Exception() { }\n    public $newException$Exception(string message) : base(message) { }\n    public $newException$Exception(string message, Exception inner) : base(message, inner) { }\n}$end$", [("newException", "My"), ("base", "Exception")], false),
    snip!("testm", "MSTest test method", "[TestMethod]\npublic void $name$()\n{\n    $end$\n}", [("name", "MyTestMethod")], false),
    snip!("testc", "MSTest test class", "[TestClass]\npublic class $name$\n{\n    [TestMethod]\n    public void TestMethod1()\n    {\n        $end$\n    }\n}", [("name", "MyTestClass")], false),
];

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Stop {
    pub name: String,
    /// Every place the placeholder occurs (linked editing).
    pub ranges: Vec<Range<usize>>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Expansion {
    pub text: String,
    /// In order of first appearance.
    pub stops: Vec<Stop>,
    /// Caret position after the last stop.
    pub end: usize,
}

pub fn find(shortcut: &str) -> Option<&'static SnippetDef> {
    SNIPPETS.iter().find(|s| s.shortcut == shortcut)
}

/// Expands a snippet for insertion on a line indented with `indent`.
/// `classname` is the enclosing type; `selected` the selection for Surround
/// With (re-indented to the snippet's body).
pub fn expand(def: &SnippetDef, indent: &str, classname: &str, selected: &str, newline: &str) -> Expansion {
    let mut text = String::new();
    let mut stops: Vec<Stop> = Vec::new();
    let mut end = None;
    let mut rest = def.body;
    while let Some(i) = rest.find('$') {
        push_indented(&mut text, &rest[..i], indent, newline);
        let after = &rest[i + 1..];
        let Some(j) = after.find('$') else {
            push_indented(&mut text, &rest[i..], indent, newline);
            rest = "";
            break;
        };
        let name = &after[..j];
        match name {
            "end" => end = Some(text.len()),
            "selected" => {
                let line_indent = current_indent(&text);
                let sel = selected.trim_end_matches('\n');
                let mut first = true;
                for line in sel.lines() {
                    if !first {
                        text.push_str(newline);
                        text.push_str(&line_indent);
                    }
                    text.push_str(line.trim_start_matches(&*common_indent(sel)));
                    first = false;
                }
            }
            _ => {
                let value = if name == "classname" { classname } else { def.defaults.iter().find(|(k, _)| *k == name).map_or(name, |(_, v)| v) };
                let start = text.len();
                text.push_str(value);
                if name != "classname" {
                    match stops.iter_mut().find(|s| s.name == name) {
                        Some(s) => s.ranges.push(start..text.len()),
                        None => stops.push(Stop { name: name.to_owned(), ranges: std::iter::once(start..text.len()).collect() }),
                    }
                }
            }
        }
        rest = &after[j + 1..];
    }
    push_indented(&mut text, rest, indent, newline);
    let end = end.unwrap_or(text.len());
    Expansion { text, stops, end }
}

fn push_indented(out: &mut String, s: &str, indent: &str, newline: &str) {
    let mut first = true;
    for line in s.split('\n') {
        if !first {
            out.push_str(newline);
            if !line.is_empty() {
                out.push_str(indent);
            }
        }
        out.push_str(line);
        first = false;
    }
}

fn current_indent(text: &str) -> String {
    let start = text.rfind('\n').map_or(0, |i| i + 1);
    text[start..].chars().take_while(|c| c.is_whitespace()).collect()
}

fn common_indent(s: &str) -> String {
    s.lines().filter(|l| !l.trim().is_empty()).map(|l| l.chars().take_while(|c| *c == ' ' || *c == '\t').collect::<String>()).min_by_key(String::len).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prop_has_two_stops() {
        let e = expand(find("prop").unwrap(), "        ", "Person", "", "\n");
        assert_eq!(e.text, "public int MyProperty { get; set; }");
        assert_eq!(e.stops.len(), 2);
        assert_eq!(&e.text[e.stops[1].ranges[0].clone()], "MyProperty");
        assert_eq!(e.end, e.text.len());
    }

    #[test]
    fn ctor_uses_class_name_and_indent() {
        let e = expand(find("ctor").unwrap(), "        ", "Person", "", "\n");
        assert_eq!(e.text, "public Person()\n        {\n            \n        }");
        assert_eq!(e.end, "public Person()\n        {\n            ".len());
        assert!(e.stops.is_empty());
    }

    #[test]
    fn for_links_index_and_surrounds() {
        let e = expand(find("for").unwrap(), "    ", "C", "a();\nb();\n", "\n");
        assert_eq!(e.text, "for (int i = 0; i < length; i++)\n    {\n        a();\n        b();\n    }");
        assert_eq!(e.stops[0].ranges.len(), 3);
    }

    #[test]
    fn crlf_newlines() {
        let e = expand(find("svm").unwrap(), "", "P", "", "\r\n");
        assert_eq!(e.text, "static void Main(string[] args)\r\n{\r\n    \r\n}");
    }
}
