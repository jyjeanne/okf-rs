use crate::jsish;
use crate::FileExtraction;
use anyhow::Result;
use okf_parser::Language;

pub fn extract(source: &str, relative_path: &str) -> Result<FileExtraction> {
    let ts_lang: tree_sitter::Language = if relative_path.ends_with(".tsx") {
        tree_sitter_typescript::LANGUAGE_TSX.into()
    } else {
        tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into()
    };
    jsish::extract(source, relative_path, Language::TypeScript, ts_lang, true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use okf_parser::ConceptKind;

    #[test]
    fn extracts_interface_class_method_and_calls() {
        let src = r#"
import { Foo } from "./foo";

export interface Greeter {
    hello(): void;
}

export class Bar {
    baz() {
        qux();
    }
}

export function qux(): void {}
"#;
        let extraction = extract(src, "src/bar.ts").unwrap();
        let names: Vec<_> = extraction
            .concepts
            .iter()
            .map(|c| c.name.as_str())
            .collect();
        assert!(names.contains(&"Greeter"));
        assert!(names.contains(&"Bar"));
        assert!(names.contains(&"baz"));
        assert!(names.contains(&"qux"));

        let baz = extraction
            .concepts
            .iter()
            .find(|c| c.name == "baz")
            .unwrap();
        assert_eq!(baz.kind, ConceptKind::Method);
        assert_eq!(baz.qualified_name, "src.bar.Bar.baz");

        let module = extraction
            .concepts
            .iter()
            .find(|c| c.name == "bar")
            .unwrap();
        assert_eq!(module.relationships.len(), 1);
        assert_eq!(module.relationships[0].target_display, "./foo");

        assert_eq!(extraction.calls.len(), 1);
        assert_eq!(extraction.calls[0].callee_name, "qux");
    }
}
