use std::ops::Range;

/// A styled highlight for a UTF-8 byte range in an editor buffer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TextHighlight {
    /// The UTF-8 byte range in the buffer to highlight.
    pub range: Range<usize>,
    /// Optional foreground color in packed `0xRRGGBBAA` format.
    pub color: Option<u32>,
    /// Optional background color in packed `0xRRGGBBAA` format.
    pub background_color: Option<u32>,
    /// Optional font weight (`100`..=`900`).
    pub font_weight: Option<u16>,
    /// Whether the text should be rendered in italics.
    pub italic: bool,
    /// Whether the text should be underlined.
    pub underline: bool,
    /// Whether the text should be struck through.
    pub strikethrough: bool,
    /// Whether the text should be faded out.
    pub fade_out: bool,
}

/// A background highlight for an inclusive 0-based buffer row range.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RowHighlight {
    /// The starting buffer row (0-based, inclusive).
    pub start_row: u32,
    /// The ending buffer row (0-based, inclusive).
    pub end_row: u32,
    /// Background color in packed `0xRRGGBBAA` format.
    pub color: u32,
    /// Whether the editor should autoscroll to this highlight.
    pub should_autoscroll: bool,
}

/// Dynamic text and row highlights computed for an editor buffer.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct EditorHighlights {
    /// Span-level text highlights to apply to the buffer.
    pub text_highlights: Vec<TextHighlight>,
    /// Full-width row background highlights to apply to the buffer.
    pub row_highlights: Vec<RowHighlight>,
}
