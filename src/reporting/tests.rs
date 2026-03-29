use super::{TextRange, TextSize};

#[test]
fn from_bounds_clamps_to_empty_when_end_precedes_start() {
    let range = TextRange::from_bounds(TextSize::new(10), TextSize::new(3));
    assert_eq!(range.start, TextSize::new(10));
    assert_eq!(range.length, TextSize::ZERO);
    assert_eq!(range.end(), TextSize::new(10));
}

#[test]
fn new_stores_start_and_length() {
    let range = TextRange::new(TextSize::new(5), TextSize::new(7));
    assert_eq!(range.start, TextSize::new(5));
    assert_eq!(range.length, TextSize::new(7));
    assert_eq!(range.end(), TextSize::new(12));
}
