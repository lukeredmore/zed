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

/// A 2D rectangle primitive for extension-rendered editor decorations.
#[derive(Debug, Clone, PartialEq)]
pub struct CanvasQuad {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
    pub background_color: u32,
    pub border_color: u32,
    pub border_width: f32,
    pub corner_radius: f32,
}

/// A 2D styled text run primitive for extension-rendered editor decorations.
#[derive(Debug, Clone, PartialEq)]
pub struct CanvasGlyphRun {
    pub x: f32,
    pub y: f32,
    pub text: String,
    pub color: u32,
    pub font_size: f32,
    pub font_weight: Option<u16>,
    pub italic: bool,
    pub monospace: bool,
}

/// An optional buffer edit triggered when a canvas decoration is clicked.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CanvasTextReplacement {
    pub range: Range<usize>,
    pub replacement_text: String,
}

/// A declarative 2D canvas element composed of quads and glyph runs.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct CanvasElement {
    pub width: f32,
    pub height: f32,
    pub quads: Vec<CanvasQuad>,
    pub glyph_runs: Vec<CanvasGlyphRun>,
    pub click_replacement: Option<CanvasTextReplacement>,
}

/// An inline fold decoration that collapses a UTF-8 byte range into a placeholder or canvas widget.
#[derive(Debug, Clone, PartialEq)]
pub struct InlineFoldDecoration {
    pub range: Range<usize>,
    pub placeholder_text: String,
    pub render_canvas: Option<CanvasElement>,
}

/// A custom block decoration anchored above or below a UTF-8 byte offset.
#[derive(Debug, Clone, PartialEq)]
pub struct BlockDecoration {
    pub anchor_offset: u32,
    pub above: bool,
    pub height_rows: u8,
    pub content: CanvasElement,
}

/// Inline fold and block decorations computed for an editor buffer.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct EditorDecorations {
    pub inline_folds: Vec<InlineFoldDecoration>,
    pub blocks: Vec<BlockDecoration>,
}
