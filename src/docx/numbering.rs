use docx_rs::*;

use super::params::Lists;

/// The number of list levels in each numbering definition.
pub(super) const LIST_LEVELS: usize = 6;

/// The `w:numId` of the first list. docx-rs writes its own `w:num` with
/// ID 1.
pub(super) const FIRST_LIST_NUM_ID: usize = 2;

/// A list numbering definition (`w:abstractNum`). The key of a definition
/// in the renderer and in `docx.yaml` is `BulletList`, `OrderedList` or
/// `TaskList`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum ListKind {
    /// `BulletList`: a bullet character on each level.
    Bullet,
    /// `OrderedList`: a number on each level.
    Ordered,
    /// `TaskList`: no marker. The item text starts with a box character.
    Task,
}

impl ListKind {
    const ALL: [ListKind; 3] = [ListKind::Bullet, ListKind::Ordered, ListKind::Task];

    /// The fixed `w:abstractNumId`. docx-rs writes its own `w:abstractNum`
    /// with ID 1.
    pub(super) fn abstract_num_id(self) -> usize {
        match self {
            ListKind::Bullet => 2,
            ListKind::Ordered => 3,
            ListKind::Task => 4,
        }
    }
}

/// Add the `BulletList`, `OrderedList` and `TaskList` numbering
/// definitions from `lists`.
pub(super) fn add_list_numbering(docx: Docx, lists: &Lists) -> Docx {
    ListKind::ALL.into_iter().fold(docx, |docx, kind| {
        docx.add_abstract_numbering(abstract_numbering(kind, lists))
    })
}

fn abstract_numbering(kind: ListKind, lists: &Lists) -> AbstractNumbering {
    (0..LIST_LEVELS).fold(
        AbstractNumbering::new(kind.abstract_num_id()),
        |numbering, ilvl| {
            let (format, text) = match kind {
                ListKind::Bullet => ("bullet", lists.bullet_list[ilvl].clone()),
                ListKind::Ordered => (lists.ordered_list[ilvl].ooxml(), format!("%{}.", ilvl + 1)),
                ListKind::Task => ("none", String::new()),
            };
            let left = lists.indent.0 * (ilvl as u32 + 1);
            numbering.add_level(
                Level::new(
                    ilvl,
                    Start::new(1),
                    NumberFormat::new(format),
                    LevelText::new(text),
                    LevelJc::new("left"),
                )
                .indent(
                    Some(left as i32),
                    Some(SpecialIndentType::Hanging(lists.hanging.0 as i32)),
                    None,
                    None,
                ),
            )
        },
    )
}
