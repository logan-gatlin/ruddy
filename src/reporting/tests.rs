use crate::engine::{Eng, Source};

use super::{TextRange, TextSize};

#[test]
fn from_bounds_clamps_to_empty_when_end_precedes_start() {
    let db = Eng::default();
    let source = Source::new(&db, "test.hc".to_owned(), "let x = 1".to_owned());

    let range = TextRange::from_bounds(source, TextSize::new(10), TextSize::new(3));
    assert_eq!(range.source(), Some(source));
    assert_eq!(range.start(), Some(TextSize::new(10)));
    assert_eq!(range.len(), Some(0));
    assert_eq!(range.end(), Some(TextSize::new(10)));
    assert_eq!(range.is_empty(), Some(true));
}

#[test]
fn new_stores_start_and_length() {
    let db = Eng::default();
    let source = Source::new(&db, "test.hc".to_owned(), "let x = 1".to_owned());

    let range = TextRange::new(source, TextSize::new(5), TextSize::new(7));
    assert_eq!(range.source(), Some(source));
    assert_eq!(range.start(), Some(TextSize::new(5)));
    assert_eq!(range.len(), Some(7));
    assert_eq!(range.end(), Some(TextSize::new(12)));
    assert_eq!(range.is_empty(), Some(false));
}

#[test]
fn generated_range_has_no_source_or_bounds() {
    let range = TextRange::generated();
    assert_eq!(range.source(), None);
    assert_eq!(range.start(), None);
    assert_eq!(range.len(), None);
    assert_eq!(range.end(), None);
    assert_eq!(range.is_empty(), None);
}
