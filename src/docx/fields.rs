use docx_rs::*;

/// Add a complex field (`w:fldChar` and `w:instrText` runs) to `p`: the
/// field begin, the instruction `instr`, the field separator, the cached
/// result `cached` and the field end.
/// Each part is in a separate run. An empty `cached` gives no result run.
pub(super) fn add_field(p: Paragraph, instr: InstrText, cached: &str) -> Paragraph {
    let mut p = p
        .add_run(Run::new().add_field_char(FieldCharType::Begin, false))
        .add_run(Run::new().add_instr_text(instr))
        .add_run(Run::new().add_field_char(FieldCharType::Separate, false));
    if !cached.is_empty() {
        p = p.add_run(Run::new().add_text(cached));
    }
    p.add_run(Run::new().add_field_char(FieldCharType::End, false))
}
