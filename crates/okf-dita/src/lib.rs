//! DITA XML ⇄ OKF converter: [`export_dita`] renders a bundle's concepts
//! as DITA topics (the "DITA (planned)" documentation-generation format
//! named alongside Markdown/HTML/PDF in the original spec), and
//! [`import_dita`] reads an existing DITA corpus back into `Document`
//! concepts — so a technical-writing team's existing DITA content joins
//! the same bundle/CLI/MCP/graph queries every code-derived concept
//! already works with, rather than living in a separate, disconnected
//! toolchain.
//!
//! Both directions are deterministic and offline — no AI dependency, no
//! DITA-OT (the Java-based DITA Open Toolkit) or any other external
//! processor required, just a hand-templated writer (export) and a
//! lightweight, dependency-free XML parser ([`roxmltree`], import).

mod export;
mod import;

pub use export::export_dita;
pub use import::import_dita;

#[cfg(test)]
mod tests {
    use super::*;
    use okf_parser::{Concept, ConceptKind, Language, Location};

    /// Exporting a bundle's concepts to DITA and re-importing that same
    /// output must round-trip: `export_dita`'s own `<!DOCTYPE ...>`
    /// prolog and topic shape must be exactly what `import_dita` accepts
    /// — the two halves of this crate are only useful together if they
    /// actually agree on the format between them, not just each in
    /// isolation against a hand-written fixture.
    #[test]
    fn exporting_then_importing_round_trips_title_and_description() {
        let original = Concept {
            id: Concept::make_id(ConceptKind::Function, "auth.verify_token"),
            kind: ConceptKind::Function,
            language: Language::Rust,
            name: "verify_token".to_string(),
            qualified_name: "auth.verify_token".to_string(),
            description: Some("Verifies a signed authentication token.".to_string()),
            location: Location {
                file: "src/auth.rs".to_string(),
                start_line: 1,
                end_line: 3,
            },
            signature: Some("fn verify_token(token: &str) -> bool".to_string()),
            tags: Vec::new(),
            is_public: true,
            generated_at: None,
            relationships: Vec::new(),
        };

        let dir = tempfile::tempdir().unwrap();
        export_dita(std::slice::from_ref(&original), dir.path()).unwrap();

        let (imported, skipped) = import_dita(dir.path()).unwrap();
        assert!(skipped.is_empty(), "unexpected skips: {skipped:?}");
        // `root.ditamap`'s extension isn't `.dita`/`.xml`, so it's never
        // even attempted as a topic -- only the one real exported topic
        // file should have imported.
        assert_eq!(imported.len(), 1);

        let round_tripped = &imported[0];
        assert_eq!(round_tripped.kind, ConceptKind::Document);
        assert_eq!(round_tripped.name, original.name);
        assert_eq!(round_tripped.description, original.description);
    }
}
