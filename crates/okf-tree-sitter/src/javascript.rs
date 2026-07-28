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
}
