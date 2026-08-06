//! Tests for [`ruddy::tracking`].

use ruddy::tracking::{FileID, FileManager, Span, Tracked};

#[test]
fn a_span_with_nowhere_to_point_points_at_generated_code() {
    // The default is what a span is when nobody wrote the thing it spans:
    // width zero, offset zero, and the file every generated span shares.
    let span = Span::default();

    assert_eq!(span.file_id, FileID::GENERATED);
    assert_eq!(span.start, 0);
    assert_eq!(span.width, 0);
    assert!(span.is_generated());
    assert!(span.file_id.is_generated());
    assert_eq!(span, Span::generated(0, 0));
}

#[test]
fn merging_covers_both_spans_whichever_order_they_arrive_in() {
    let file = FileManager::new().register_new_file("a.hc".into(), "let x = 1".into());
    let first = file.span(0, 3);
    let second = file.span(6, 3);

    // The smallest span covering both, and the same one either way round —
    // which is the case that reaches for `other`'s end rather than its own.
    let merged = first.merge(second);
    assert_eq!(merged, file.span(0, 9));
    assert_eq!(second.merge(first), file.span(0, 9));

    // A span already covering the other keeps its own end.
    assert_eq!(file.span(0, 9).merge(file.span(2, 1)), file.span(0, 9));
    assert_eq!(first.end(), 3);
    assert!(!first.is_generated());
}

#[test]
fn tracking_attaches_a_span_and_gives_the_value_back() {
    let span = Span::generated(4, 2);

    // The two ways to build one agree, and unwrapping returns what went in.
    let built = Tracked::new("x", span);
    assert_eq!(built, span.track("x"));
    assert_eq!(built.span, span);
    assert_eq!(built.tracked, "x");
    assert_eq!(built.into_tracked(), "x");
}

#[test]
fn a_new_file_manager_already_knows_the_generated_file() {
    // `Default` is `new`, and both start out holding the one file that exists
    // before any source is read.
    let mut files = FileManager::default();
    let generated = files.get_file(FileID::GENERATED).clone();

    assert_eq!(generated.path, "@GENERATED-CODE");
    assert_eq!(generated.content, "");
    assert_eq!(&generated, FileManager::new().get_file(FileID::GENERATED));
}

#[test]
fn registering_hands_back_an_id_that_reads_the_file_again() {
    let mut files = FileManager::new();
    let first = files.register_new_file("a.hc".into(), "let a = 1".into());
    let second = files.register_new_file("b.hc".into(), "let b = 2".into());

    assert_ne!(first, second);
    assert!(!first.is_generated() && !second.is_generated());
    assert_eq!(files.get_file(first).path, "a.hc");
    assert_eq!(files.get_file(second).content, "let b = 2");
}

#[test]
#[should_panic(expected = "Non-existant FileID")]
fn a_file_id_from_nowhere_cannot_be_read() {
    // Ids are minted by one manager and mean nothing to another.
    let mut other = FileManager::new();
    let stranger = FileManager::new().register_new_file("a.hc".into(), String::new());

    other.get_file(stranger);
}
