use docx_rs::*;

/// Numbering ID for unordered (bullet) lists.
pub(super) const BULLET_NUMBERING_ID: usize = 1;
/// Numbering ID for ordered (decimal) lists.
pub(super) const ORDERED_NUMBERING_ID: usize = 2;

/// Add the bullet and ordered list numbering definitions (levels 0 to 2).
pub(super) fn add_list_numbering(docx: Docx) -> Docx {
    docx.add_abstract_numbering(
        AbstractNumbering::new(BULLET_NUMBERING_ID)
            .add_level(
                Level::new(
                    0,
                    Start::new(1),
                    NumberFormat::new("bullet"),
                    LevelText::new("\u{2022}"),
                    LevelJc::new("left"),
                )
                .indent(
                    Some(720),
                    Some(SpecialIndentType::Hanging(360)),
                    None,
                    None,
                ),
            )
            .add_level(
                Level::new(
                    1,
                    Start::new(1),
                    NumberFormat::new("bullet"),
                    LevelText::new("\u{25E6}"),
                    LevelJc::new("left"),
                )
                .indent(
                    Some(1440),
                    Some(SpecialIndentType::Hanging(360)),
                    None,
                    None,
                ),
            )
            .add_level(
                Level::new(
                    2,
                    Start::new(1),
                    NumberFormat::new("bullet"),
                    LevelText::new("\u{25AA}"),
                    LevelJc::new("left"),
                )
                .indent(
                    Some(2160),
                    Some(SpecialIndentType::Hanging(360)),
                    None,
                    None,
                ),
            ),
    )
    .add_abstract_numbering(
        AbstractNumbering::new(ORDERED_NUMBERING_ID)
            .add_level(
                Level::new(
                    0,
                    Start::new(1),
                    NumberFormat::new("decimal"),
                    LevelText::new("%1."),
                    LevelJc::new("left"),
                )
                .indent(
                    Some(720),
                    Some(SpecialIndentType::Hanging(360)),
                    None,
                    None,
                ),
            )
            .add_level(
                Level::new(
                    1,
                    Start::new(1),
                    NumberFormat::new("lowerLetter"),
                    LevelText::new("%2."),
                    LevelJc::new("left"),
                )
                .indent(
                    Some(1440),
                    Some(SpecialIndentType::Hanging(360)),
                    None,
                    None,
                ),
            )
            .add_level(
                Level::new(
                    2,
                    Start::new(1),
                    NumberFormat::new("lowerRoman"),
                    LevelText::new("%3."),
                    LevelJc::new("left"),
                )
                .indent(
                    Some(2160),
                    Some(SpecialIndentType::Hanging(360)),
                    None,
                    None,
                ),
            ),
    )
    .add_numbering(Numbering::new(BULLET_NUMBERING_ID, BULLET_NUMBERING_ID))
    .add_numbering(Numbering::new(ORDERED_NUMBERING_ID, ORDERED_NUMBERING_ID))
}
