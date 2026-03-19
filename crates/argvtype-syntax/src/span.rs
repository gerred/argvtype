use serde::Serialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
pub struct SourceId(pub u32);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
pub struct Span {
    pub start: u32,
    pub end: u32,
}

impl Span {
    pub fn new(start: u32, end: u32) -> Self {
        Self { start, end }
    }

    pub fn to_miette(self) -> miette::SourceSpan {
        (self.start as usize, (self.end - self.start) as usize).into()
    }
}

#[derive(Debug, Clone)]
pub struct SourceFile {
    pub id: SourceId,
    pub name: String,
    pub source: String,
}

impl SourceFile {
    pub fn new(id: SourceId, name: String, source: String) -> Self {
        Self { id, name, source }
    }

    pub fn text(&self, span: Span) -> &str {
        &self.source[span.start as usize..span.end as usize]
    }

    pub fn line_col(&self, offset: u32) -> (usize, usize) {
        let offset = offset as usize;
        let mut line = 0;
        let mut col = 0;
        for (i, ch) in self.source.char_indices() {
            if i == offset {
                return (line, col);
            }
            if ch == '\n' {
                line += 1;
                col = 0;
            } else {
                col += 1;
            }
        }
        (line, col)
    }
}
