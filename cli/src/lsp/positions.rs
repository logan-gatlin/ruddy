//! UTF-16 positions indexed once per source version. ASCII columns need no
//! character table; only non-ASCII scalars contribute conversion checkpoints.
use ruddy::tracking::Span;
use serde_json::{Value, json};
use std::{
    cell::RefCell,
    collections::HashMap,
    path::{Path, PathBuf},
    sync::Arc,
};

struct Wide {
    byte: usize,
    utf16: usize,
    bytes: usize,
    units: usize,
}

struct Line {
    start: usize,
    end: usize,
    wide: Vec<Wide>,
}

/// An immutable source snapshot with cached UTF-16 line/column conversions.
pub struct LineIndex {
    text: Arc<str>,
    lines: Vec<Line>,
}

impl LineIndex {
    pub fn new(text: &str) -> Self {
        let mut start = 0;
        let lines = text
            .split('\n')
            .map(|line| {
                ruddy::cancellation::checkpoint();
                let mut units = 0;
                let wide = line
                    .char_indices()
                    .filter_map(|(byte, ch)| {
                        if byte % 4096 == 0 {
                            ruddy::cancellation::checkpoint();
                        }
                        let utf16 = units;
                        units += ch.len_utf16();
                        (ch.len_utf8() > 1).then_some(Wide {
                            byte,
                            utf16,
                            bytes: ch.len_utf8(),
                            units: ch.len_utf16(),
                        })
                    })
                    .collect();
                let result = Line {
                    start,
                    end: start + line.trim_end_matches('\r').len(),
                    wide,
                };
                start += line.len() + 1;
                result
            })
            .collect();
        Self {
            text: Arc::from(text),
            lines,
        }
    }

    pub fn offset(&self, position: &Value) -> Option<usize> {
        let row = usize::try_from(position.get("line")?.as_u64()?).ok()?;
        let column = usize::try_from(position.get("character")?.as_u64()?).ok()?;
        let line = self.lines.get(row)?;
        let before = line.wide.partition_point(|wide| wide.utf16 < column);
        let extra = if before == 0 {
            0
        } else {
            let wide = &line.wide[before - 1];
            if column < wide.utf16 + wide.units {
                return None;
            }
            wide.byte + wide.bytes - wide.utf16 - wide.units
        };
        Some(
            line.start
                .saturating_add(column)
                .saturating_add(extra)
                .min(line.end),
        )
    }

    pub fn position(&self, at: usize) -> Value {
        let at = self.text.floor_char_boundary(at.min(self.text.len()));
        let row = self.lines.partition_point(|line| line.start <= at) - 1;
        let line = &self.lines[row];
        let column = at - line.start;
        let before = line.wide.partition_point(|wide| wide.byte < column);
        let extra = if before == 0 {
            0
        } else {
            let wide = &line.wide[before - 1];
            wide.byte + wide.bytes - wide.utf16 - wide.units
        };
        json!({"line":row,"character":column - extra})
    }

    pub(super) fn range(&self, span: Span) -> Value {
        json!({"start":self.position(span.start),"end":self.position(span.end())})
    }
}

#[derive(Default)]
pub(super) struct LineIndexes(RefCell<HashMap<PathBuf, Arc<LineIndex>>>);

impl LineIndexes {
    pub fn get(&self, path: &Path, text: &str) -> Arc<LineIndex> {
        let mut indexes = self.0.borrow_mut();
        if let Some(index) = indexes.get(path)
            && index.text.as_ref() == text
        {
            return index.clone();
        }
        let index = Arc::new(LineIndex::new(text));
        indexes.insert(path.to_owned(), index.clone());
        index
    }

    pub fn retain(&self, paths: &std::collections::HashSet<PathBuf>) {
        self.0.borrow_mut().retain(|path, _| paths.contains(path));
    }
}
