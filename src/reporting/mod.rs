use crate::engine::Source;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash, salsa::Update)]
pub struct TextSize(u32);

impl TextSize {
    pub const ZERO: Self = Self(0);

    pub const fn new(value: u32) -> Self {
        Self(value)
    }

    pub fn from_usize(value: usize) -> Self {
        Self(u32::try_from(value).unwrap_or(u32::MAX))
    }

    pub const fn as_u32(self) -> u32 {
        self.0
    }

    pub const fn as_usize(self) -> usize {
        self.0 as usize
    }
}

impl From<u32> for TextSize {
    fn from(value: u32) -> Self {
        Self::new(value)
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, salsa::Update)]
pub enum TextRange {
    Located {
        source: Source,
        start: TextSize,
        length: TextSize,
    },
    #[default]
    Generated,
}

impl TextRange {
    pub fn new(source: Source, start: TextSize, length: TextSize) -> Self {
        Self::Located {
            source,
            start,
            length,
        }
    }

    pub fn from_bounds(source: Source, start: TextSize, end: TextSize) -> Self {
        let length = TextSize::new(end.as_u32().saturating_sub(start.as_u32()));
        Self::Located {
            source,
            start,
            length,
        }
    }

    pub fn empty(source: Source, at: TextSize) -> Self {
        Self::new(source, at, TextSize::ZERO)
    }

    pub const fn generated() -> Self {
        Self::Generated
    }

    pub fn source(self) -> Option<Source> {
        match self {
            Self::Located { source, .. } => Some(source),
            Self::Generated => None,
        }
    }

    pub fn start(self) -> Option<TextSize> {
        match self {
            Self::Located { start, .. } => Some(start),
            Self::Generated => None,
        }
    }

    pub fn end(self) -> Option<TextSize> {
        match self {
            Self::Located { start, length, .. } => Some(TextSize::new(
                start.as_u32().saturating_add(length.as_u32()),
            )),
            Self::Generated => None,
        }
    }

    pub fn len(self) -> Option<u32> {
        match self {
            Self::Located { length, .. } => Some(length.as_u32()),
            Self::Generated => None,
        }
    }

    pub fn is_empty(self) -> Option<bool> {
        self.len().map(|length| length == 0)
    }

    pub fn text(&self, source: &str) -> Option<String> {
        let start = self.start()?.as_usize();
        let end = self.end()?.as_usize();
        source.get(start..end).map(ToOwned::to_owned)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, salsa::Update)]
pub enum DiagnosticSeverity {
    Error,
    Warning,
    Note,
}

#[derive(Debug, Clone, PartialEq, Eq, salsa::Update)]
pub struct Diagnostic {
    pub severity: DiagnosticSeverity,
    pub range: TextRange,
    pub message: String,
}

impl Diagnostic {
    pub fn new(severity: DiagnosticSeverity, range: TextRange, message: impl Into<String>) -> Self {
        Self {
            severity,
            range,
            message: message.into(),
        }
    }

    pub fn error(range: TextRange, message: impl Into<String>) -> Self {
        Self::new(DiagnosticSeverity::Error, range, message)
    }

    pub fn warning(range: TextRange, message: impl Into<String>) -> Self {
        Self::new(DiagnosticSeverity::Warning, range, message)
    }
}

#[salsa::accumulator]
#[derive(Clone, Debug)]
pub struct Diag(pub Diagnostic);

#[cfg(test)]
mod tests;
