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
pub struct TextRange {
    pub start: TextSize,
    pub length: TextSize,
}

impl TextRange {
    pub const fn new(start: TextSize, length: TextSize) -> Self {
        Self { start, length }
    }

    pub fn from_bounds(start: TextSize, end: TextSize) -> Self {
        let length = TextSize::new(end.as_u32().saturating_sub(start.as_u32()));
        Self { start, length }
    }

    pub fn empty(at: TextSize) -> Self {
        Self::new(at, TextSize::ZERO)
    }

    pub fn end(self) -> TextSize {
        TextSize::new(self.start.as_u32().saturating_add(self.length.as_u32()))
    }

    pub fn len(self) -> u32 {
        self.length.as_u32()
    }

    pub fn is_empty(self) -> bool {
        self.length == TextSize::ZERO
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
