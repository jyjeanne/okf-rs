//! PDF export: a single, paginated PDF document with one section per
//! concept, grouped by kind — the same content `generate_markdown`
//! renders as one consolidated Markdown file, here as a single file
//! meant to be printed, archived, or shared as-is rather than opened in
//! an editor or browser.
//!
//! Built directly on `printpdf`'s low-level page/layer/text API (not a
//! higher-level PDF layout crate) so this doesn't need to embed a font:
//! `printpdf`'s built-in Helvetica/Courier fonts are part of the PDF
//! spec's standard 14 fonts, present in every conformant PDF reader,
//! which keeps the `okf-rs` binary's dependency footprint and size down
//! — consistent with `okf-generator`/`okf-docs`'s HTML/Markdown
//! renderers, which likewise hand-build their output rather than pull in
//! a templating engine.
//!
//! **Known limitation:** `printpdf`'s built-in fonts don't expose glyph
//! width metrics, so line-wrapping here estimates each character's width
//! as a fixed fraction of the font size ([`AVG_CHAR_WIDTH_EM`]) rather
//! than measuring real glyph widths — close enough for a readable,
//! utilitarian export, not pixel/point-perfect typesetting. It's tuned
//! to wrap a little early rather than let a line overflow the page.

use crate::RELATIONSHIP_HEADINGS;
use anyhow::{anyhow, Result};
use okf_parser::Concept;
use okf_render::{capitalize, group_by_kind_dir};
use printpdf::{
    BuiltinFont, IndirectFontRef, Mm, PdfDocument, PdfDocumentReference, PdfLayerIndex,
    PdfPageIndex,
};

const PAGE_WIDTH_MM: f32 = 210.0;
const PAGE_HEIGHT_MM: f32 = 297.0;
const MARGIN_MM: f32 = 20.0;
const USABLE_WIDTH_MM: f32 = PAGE_WIDTH_MM - 2.0 * MARGIN_MM;

const TITLE_SIZE: f32 = 22.0;
const HEADING1_SIZE: f32 = 15.0;
const HEADING2_SIZE: f32 = 12.0;
const BODY_SIZE: f32 = 10.0;
const MONO_SIZE: f32 = 9.0;

/// Line-height multiplier over font size (a typical single-spaced ratio).
const LINE_HEIGHT_FACTOR: f32 = 1.35;
/// Extra vertical gap after a whole entry, in mm.
const ENTRY_GAP_MM: f32 = 4.0;
/// Points-to-millimeters conversion (1 pt = 1/72 inch = 25.4/72 mm).
const PT_TO_MM: f32 = 25.4 / 72.0;
/// Estimated average glyph width as a fraction of font size, for
/// Helvetica/Courier-family text — see the module docs' known-limitation
/// note on why this is an estimate, not a measurement.
const AVG_CHAR_WIDTH_EM: f32 = 0.5;

/// Renders `concepts` as a single paginated PDF document (A4, one
/// section per concept grouped by kind, with a PDF outline/bookmark per
/// concept for navigation) and returns its bytes — the caller decides
/// where to write them, the same way [`crate::generate_markdown`]
/// returns a `String` rather than writing a file itself.
pub fn generate_pdf(concepts: &[Concept]) -> Result<Vec<u8>> {
    let by_dir = group_by_kind_dir(concepts);

    let mut writer = PdfWriter::new();
    writer.title("Documentation");
    writer.body(&format!("{} concepts", concepts.len()));
    writer.gap();

    for (dir, entries) in &by_dir {
        writer.heading1(&format!("{} ({})", capitalize(dir), entries.len()));
        for concept in entries {
            writer.bookmark(&concept.name);
            writer.heading2(&concept.name);
            writer.body(&format!(
                "{} — {}",
                concept.frontmatter_type(),
                concept.location
            ));
            if !concept.is_public {
                writer.body("private");
            }
            if let Some(signature) = &concept.signature {
                writer.mono(signature);
            }
            if let Some(description) = &concept.description {
                writer.body(description);
            }
            if !concept.tags.is_empty() {
                writer.body(&format!("Tags: {}", concept.tags.join(", ")));
            }
            for (kind, heading) in RELATIONSHIP_HEADINGS {
                let targets: Vec<&str> = concept
                    .relationships
                    .iter()
                    .filter(|r| r.kind == kind)
                    .map(|r| r.target_display.as_str())
                    .collect();
                if targets.is_empty() {
                    continue;
                }
                writer.body(&format!("{heading}: {}", targets.join(", ")));
            }
            writer.gap();
        }
    }

    writer.finish()
}

/// Tracks the current page/cursor position across a flowing document,
/// starting a new page whenever the next line wouldn't fit — the PDF
/// equivalent of what a word processor's pagination does automatically,
/// hand-rolled here since `printpdf` only exposes page/text primitives,
/// not layout.
struct PdfWriter {
    doc: PdfDocumentReference,
    page: PdfPageIndex,
    layer: PdfLayerIndex,
    font: IndirectFontRef,
    font_bold: IndirectFontRef,
    font_mono: IndirectFontRef,
    /// Current cursor height from the bottom of the page, in mm.
    y: f32,
}

impl PdfWriter {
    fn new() -> Self {
        let (doc, page, layer) = PdfDocument::new(
            "okf-rs documentation",
            Mm(PAGE_WIDTH_MM),
            Mm(PAGE_HEIGHT_MM),
            "Layer 1",
        );
        // `add_builtin_font` only fails for a `BuiltinFont` variant that
        // isn't actually one of the standard 14 -- unreachable with the
        // fixed variants used here.
        let font = doc
            .add_builtin_font(BuiltinFont::Helvetica)
            .expect("Helvetica is one of the standard 14 PDF fonts");
        let font_bold = doc
            .add_builtin_font(BuiltinFont::HelveticaBold)
            .expect("Helvetica-Bold is one of the standard 14 PDF fonts");
        let font_mono = doc
            .add_builtin_font(BuiltinFont::Courier)
            .expect("Courier is one of the standard 14 PDF fonts");
        PdfWriter {
            doc,
            page,
            layer,
            font,
            font_bold,
            font_mono,
            y: PAGE_HEIGHT_MM - MARGIN_MM,
        }
    }

    fn line_height_mm(font_size: f32) -> f32 {
        font_size * LINE_HEIGHT_FACTOR * PT_TO_MM
    }

    /// Starts a new page (and resets the cursor to the top) if the next
    /// line wouldn't fit above the bottom margin.
    fn ensure_space(&mut self, height_needed_mm: f32) {
        if self.y - height_needed_mm >= MARGIN_MM {
            return;
        }
        let (page, layer) = self
            .doc
            .add_page(Mm(PAGE_WIDTH_MM), Mm(PAGE_HEIGHT_MM), "Layer 1");
        self.page = page;
        self.layer = layer;
        self.y = PAGE_HEIGHT_MM - MARGIN_MM;
    }

    /// Greedily word-wraps `text` to [`USABLE_WIDTH_MM`] at `font_size`
    /// (an estimate — see the module docs) and writes each resulting
    /// line, paginating as needed.
    fn write_wrapped(&mut self, text: &str, font_size: f32, font: &IndirectFontRef) {
        let avg_char_width_mm = font_size * AVG_CHAR_WIDTH_EM * PT_TO_MM;
        let max_chars = ((USABLE_WIDTH_MM / avg_char_width_mm).floor() as usize).max(1);
        let line_height = Self::line_height_mm(font_size);

        for line in wrap_text(text, max_chars) {
            self.ensure_space(line_height);
            let layer_ref = self.doc.get_page(self.page).get_layer(self.layer);
            layer_ref.use_text(&line, font_size, Mm(MARGIN_MM), Mm(self.y), font);
            self.y -= line_height;
        }
    }

    fn title(&mut self, text: &str) {
        self.write_wrapped(text, TITLE_SIZE, &self.font_bold.clone());
    }

    fn heading1(&mut self, text: &str) {
        self.y -= ENTRY_GAP_MM / 2.0;
        self.write_wrapped(text, HEADING1_SIZE, &self.font_bold.clone());
    }

    fn heading2(&mut self, text: &str) {
        self.write_wrapped(text, HEADING2_SIZE, &self.font_bold.clone());
    }

    fn body(&mut self, text: &str) {
        self.write_wrapped(text, BODY_SIZE, &self.font.clone());
    }

    fn mono(&mut self, text: &str) {
        self.write_wrapped(text, MONO_SIZE, &self.font_mono.clone());
    }

    fn gap(&mut self) {
        self.y -= ENTRY_GAP_MM;
    }

    fn bookmark(&mut self, title: &str) {
        self.doc.add_bookmark(title, self.page);
    }

    fn finish(self) -> Result<Vec<u8>> {
        self.doc
            .save_to_bytes()
            .map_err(|e| anyhow!("failed to render PDF: {e:?}"))
    }
}

/// Greedy word-wrap to at most `max_chars` per line. Falls back to
/// hard-splitting a single word longer than `max_chars` on its own
/// (rather than looping forever or silently overflowing), which a real
/// signature/URL with no spaces can hit.
fn wrap_text(text: &str, max_chars: usize) -> Vec<String> {
    let mut lines = Vec::new();
    let mut current = String::new();

    for word in text.split_whitespace() {
        let candidate_len = if current.is_empty() {
            word.chars().count()
        } else {
            current.chars().count() + 1 + word.chars().count()
        };

        if candidate_len <= max_chars {
            if !current.is_empty() {
                current.push(' ');
            }
            current.push_str(word);
            continue;
        }

        if !current.is_empty() {
            lines.push(std::mem::take(&mut current));
        }

        if word.chars().count() <= max_chars {
            current = word.to_string();
        } else {
            for chunk in hard_split(word, max_chars) {
                lines.push(chunk);
            }
        }
    }
    if !current.is_empty() {
        lines.push(current);
    }
    if lines.is_empty() {
        lines.push(String::new());
    }
    lines
}

fn hard_split(word: &str, max_chars: usize) -> Vec<String> {
    word.chars()
        .collect::<Vec<_>>()
        .chunks(max_chars)
        .map(|chunk| chunk.iter().collect())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wraps_long_text_into_multiple_lines_within_the_limit() {
        let lines = wrap_text("the quick brown fox jumps over the lazy dog", 12);
        assert!(lines.len() > 1);
        for line in &lines {
            assert!(line.chars().count() <= 12, "line too long: {line:?}");
        }
        assert_eq!(lines.join(" "), "the quick brown fox jumps over the lazy dog");
    }

    #[test]
    fn short_text_stays_on_one_line() {
        assert_eq!(wrap_text("short text", 40), vec!["short text".to_string()]);
    }

    #[test]
    fn a_single_word_longer_than_the_limit_is_hard_split() {
        let lines = wrap_text("supercalifragilisticexpialidocious", 10);
        assert!(lines.len() > 1);
        for line in &lines {
            assert!(line.chars().count() <= 10);
        }
        assert_eq!(lines.join(""), "supercalifragilisticexpialidocious");
    }

    #[test]
    fn empty_text_produces_one_empty_line_not_zero_lines() {
        assert_eq!(wrap_text("", 10), vec![String::new()]);
    }

    #[test]
    fn generate_pdf_produces_a_well_formed_pdf() {
        use okf_parser::{ConceptKind, Language, Location};

        let concept = Concept {
            id: Concept::make_id(ConceptKind::Function, "auth.verify_token"),
            kind: ConceptKind::Function,
            language: Language::Rust,
            name: "verify_token".to_string(),
            qualified_name: "auth.verify_token".to_string(),
            description: Some("Verifies a signed token.".to_string()),
            location: Location {
                file: "src/auth.rs".to_string(),
                start_line: 1,
                end_line: 3,
            },
            signature: Some("fn verify_token(token: &str) -> bool".to_string()),
            tags: vec!["auth".to_string()],
            is_public: true,
            generated_at: None,
            relationships: Vec::new(),
        };

        let bytes = generate_pdf(&[concept]).unwrap();
        assert!(bytes.starts_with(b"%PDF-"));
        assert!(bytes.len() > 200, "suspiciously small PDF output");
    }

    #[test]
    fn generate_pdf_paginates_a_large_number_of_concepts() {
        use okf_parser::{ConceptKind, Language, Location};

        let concepts: Vec<Concept> = (0..200)
            .map(|i| Concept {
                id: Concept::make_id(ConceptKind::Function, &format!("m.f{i}")),
                kind: ConceptKind::Function,
                language: Language::Rust,
                name: format!("f{i}"),
                qualified_name: format!("m.f{i}"),
                description: Some("A function that does something.".to_string()),
                location: Location {
                    file: "src/lib.rs".to_string(),
                    start_line: i,
                    end_line: i,
                },
                signature: Some(format!("fn f{i}()")),
                tags: Vec::new(),
                is_public: true,
                generated_at: None,
                relationships: Vec::new(),
            })
            .collect();

        // Just needs to not panic/error across many forced page breaks;
        // the resulting bytes being a well-formed multi-page PDF is
        // exercised indirectly (printpdf itself would error on a
        // malformed page/layer reference during `save_to_bytes`).
        let bytes = generate_pdf(&concepts).unwrap();
        assert!(bytes.starts_with(b"%PDF-"));
    }
}
