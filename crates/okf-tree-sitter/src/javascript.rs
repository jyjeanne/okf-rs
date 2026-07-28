use crate::jsish;
use crate::FileExtraction;
use anyhow::Result;
use okf_parser::Language;

pub fn extract(source: &str, relative_path: &str) -> Result<FileExtraction> {
    let ts_lang: tree_sitter::Language = tree_sitter_javascript::LANGUAGE.into();
    jsish::extract(source, relative_path, Language::JavaScript, ts_lang, false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use okf_parser::ConceptKind;

    #[test]
    fn extracts_class_method_and_calls() {
        let src = r#"
import { Foo } from "./foo";

class Bar {
    baz() {
        qux();
    }
}

function qux() {}
"#;
        let extraction = extract(src, "src/bar.js").unwrap();
        let names: Vec<_> = extraction
            .concepts
            .iter()
            .map(|c| c.name.as_str())
            .collect();
        assert!(names.contains(&"Bar"));
        assert!(names.contains(&"baz"));
        assert!(names.contains(&"qux"));

        let baz = extraction
            .concepts
            .iter()
            .find(|c| c.name == "baz")
            .unwrap();
        assert_eq!(baz.kind, ConceptKind::Method);

        assert_eq!(extraction.calls.len(), 1);
        assert_eq!(extraction.calls[0].callee_name, "qux");
    }

    #[test]
    fn object_literal_shorthand_methods_are_skipped_not_collided() {
        let src = r#"
function greet() { console.log("hi"); }
const handlers = { greet() { console.log("obj greet"); } };
"#;
        let extraction = extract(src, "src/foo.js").unwrap();
        let greet_fns: Vec<_> = extraction
            .concepts
            .iter()
            .filter(|c| c.name == "greet")
            .collect();
        // Only the top-level `function greet()` is a concept; the object
        // literal's shorthand method has no stable container, so it's
        // skipped instead of colliding with it on id.
        assert_eq!(greet_fns.len(), 1);
        assert_eq!(greet_fns[0].kind, ConceptKind::Function);
    }

    #[test]
    fn captures_member_expression_calls() {
        let src = r#"
class Bar {
    baz() {
        this.helper();
    }
    helper() {}
}
"#;
        let extraction = extract(src, "src/bar.js").unwrap();
        assert!(extraction.calls.iter().any(|c| c.callee_name == "helper"));
    }

    #[test]
    fn detects_underscore_convention_visibility() {
        let src = r#"
function publicFn() {}
function _privateFn() {}
"#;
        let extraction = extract(src, "src/foo.js").unwrap();
        let find = |name: &str| extraction.concepts.iter().find(|c| c.name == name).unwrap();

        assert!(find("publicFn").is_public);
        assert!(!find("_privateFn").is_public);
    }
}
