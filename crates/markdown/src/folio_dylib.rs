use std::ops::Range;
use std::sync::{Arc, RwLock};

pub trait MarkdownPreviewExtensionProvider: Send + Sync {
    fn is_alive(&self) -> bool {
        true
    }
    fn invoke_command(&self, command: &str, args: &[String]) -> Option<String>;
}

static EXTENSION_PROVIDER: RwLock<Option<Arc<dyn MarkdownPreviewExtensionProvider>>> =
    RwLock::new(None);

pub fn register_extension_provider(provider: Arc<dyn MarkdownPreviewExtensionProvider>) {
    if let Ok(mut guard) = EXTENSION_PROVIDER.write() {
        *guard = Some(provider);
    }
}

pub fn clear_extension_provider() {
    if let Ok(mut guard) = EXTENSION_PROVIDER.write() {
        *guard = None;
    }
}

pub fn is_extension_loaded() -> bool {
    let provider = EXTENSION_PROVIDER
        .read()
        .ok()
        .and_then(|guard| guard.clone());
    match provider {
        Some(p) if p.is_alive() => true,
        Some(_) => {
            clear_extension_provider();
            false
        }
        None => false,
    }
}

fn call_extension(command: &str, args: &[String]) -> Option<String> {
    let provider = EXTENSION_PROVIDER
        .read()
        .ok()
        .and_then(|guard| guard.clone())?;
    if !provider.is_alive() {
        clear_extension_provider();
        return None;
    }
    provider.invoke_command(command, args)
}

#[derive(Debug, Clone, PartialEq)]
pub enum GpuiMathPrimitive {
    ShapedGlyphRun {
        text: String,
        origin_x: f32,
        baseline_y: f32,
        font_size: f32,
        italic: bool,
        bold: bool,
        serif: bool,
    },
    RuleQuad {
        x: f32,
        y: f32,
        width: f32,
        height: f32,
    },
    RadicalStroke {
        x: f32,
        top_y: f32,
        bottom_y: f32,
        hook_width: f32,
        radicand_width: f32,
        line_width: f32,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub struct GpuiMathCanvasDescriptor {
    pub width: f32,
    pub ascent: f32,
    pub descent: f32,
    pub primitives: Vec<GpuiMathPrimitive>,
    pub plain_text_accessibility: String,
}

impl GpuiMathCanvasDescriptor {
    pub fn total_height(&self) -> f32 {
        (self.ascent + self.descent).max(1.0)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KeystrokeDecision {
    Unhandled,
    HandledNoOp,
    TogglePrettyMode,
    ClipboardCut(Range<usize>),
    ClipboardPaste(Range<usize>),
    SelectRange(Range<usize>),
    SourceEdit {
        edit_range: Range<usize>,
        replacement: String,
        post_selection: Option<Range<usize>>,
    },
}

fn parse_f32_bits(s: &str) -> Option<f32> {
    s.parse::<u32>().ok().map(f32::from_bits)
}

pub fn normalize_svg_markup(raw_svg: &str) -> Option<(String, f32, f32)> {
    let out = call_extension("normalize_svg", &[raw_svg.to_string()])?;
    let (header, svg) = out.split_once('\n')?;
    let mut parts = header.split('\t');
    if parts.next()? != "1" {
        return None;
    }
    let width = parse_f32_bits(parts.next()?)?;
    let height = parse_f32_bits(parts.next()?)?;
    Some((svg.to_string(), width, height))
}

pub fn render_mermaid_to_svg(source: &str, is_dark: bool) -> Option<String> {
    let out = call_extension(
        "render_mermaid_svg",
        &[
            source.to_string(),
            if is_dark { "1" } else { "0" }.to_string(),
        ],
    )?;
    let (header, svg) = out.split_once('\n')?;
    let mut parts = header.split('\t');
    if parts.next()? != "1" {
        return None;
    }
    Some(svg.to_string())
}

pub fn build_gpui_math_canvas(
    latex: &str,
    base_font_size: f32,
    display_mode: bool,
) -> GpuiMathCanvasDescriptor {
    let Some(out) = call_extension(
        "build_math_canvas",
        &[
            latex.to_string(),
            base_font_size.to_string(),
            if display_mode { "1" } else { "0" }.to_string(),
        ],
    ) else {
        return fallback_math_canvas(latex, base_font_size);
    };
    decode_math_canvas(&out).unwrap_or_else(|| fallback_math_canvas(latex, base_font_size))
}

fn fallback_math_canvas(latex: &str, base_font_size: f32) -> GpuiMathCanvasDescriptor {
    let text = latex.trim().to_string();
    let width = (text.chars().count() as f32 * base_font_size * 0.56).max(base_font_size * 0.8);
    let ascent = base_font_size * 0.82;
    let descent = base_font_size * 0.28;
    GpuiMathCanvasDescriptor {
        width,
        ascent,
        descent,
        primitives: vec![GpuiMathPrimitive::ShapedGlyphRun {
            text: text.clone(),
            origin_x: 0.0,
            baseline_y: ascent,
            font_size: base_font_size,
            italic: true,
            bold: false,
            serif: true,
        }],
        plain_text_accessibility: text,
    }
}

fn decode_math_canvas(out: &str) -> Option<GpuiMathCanvasDescriptor> {
    let mut lines = out.lines();
    let header = lines.next()?;
    let mut hparts = header.split('\t');
    let width = parse_f32_bits(hparts.next()?)?;
    let ascent = parse_f32_bits(hparts.next()?)?;
    let descent = parse_f32_bits(hparts.next()?)?;
    let count = hparts.next()?.parse::<usize>().ok()?;
    let mut primitives = Vec::with_capacity(count);
    for _ in 0..count {
        let line = lines.next()?;
        let mut p = line.splitn(8, '\t');
        match p.next()? {
            "G" => {
                let origin_x = parse_f32_bits(p.next()?)?;
                let baseline_y = parse_f32_bits(p.next()?)?;
                let font_size = parse_f32_bits(p.next()?)?;
                let italic = p.next()? == "1";
                let bold = p.next()? == "1";
                let serif = p.next()? == "1";
                let text = p.next()?.to_string();
                primitives.push(GpuiMathPrimitive::ShapedGlyphRun {
                    text,
                    origin_x,
                    baseline_y,
                    font_size,
                    italic,
                    bold,
                    serif,
                });
            }
            "R" => {
                let x = parse_f32_bits(p.next()?)?;
                let y = parse_f32_bits(p.next()?)?;
                let width = parse_f32_bits(p.next()?)?;
                let height = parse_f32_bits(p.next()?)?;
                primitives.push(GpuiMathPrimitive::RuleQuad {
                    x,
                    y,
                    width,
                    height,
                });
            }
            "S" => {
                let x = parse_f32_bits(p.next()?)?;
                let top_y = parse_f32_bits(p.next()?)?;
                let bottom_y = parse_f32_bits(p.next()?)?;
                let hook_width = parse_f32_bits(p.next()?)?;
                let radicand_width = parse_f32_bits(p.next()?)?;
                let line_width = parse_f32_bits(p.next()?)?;
                primitives.push(GpuiMathPrimitive::RadicalStroke {
                    x,
                    top_y,
                    bottom_y,
                    hook_width,
                    radicand_width,
                    line_width,
                });
            }
            _ => return None,
        }
    }
    let plain_text_accessibility = lines.collect::<Vec<_>>().join("\n");
    Some(GpuiMathCanvasDescriptor {
        width,
        ascent,
        descent,
        primitives,
        plain_text_accessibility,
    })
}

#[allow(clippy::too_many_arguments)]
pub fn evaluate_keystroke(
    source: &str,
    selection: Range<usize>,
    head: usize,
    tail: usize,
    key: &str,
    platform: bool,
    control: bool,
    alt: bool,
    shift: bool,
    function: bool,
) -> KeystrokeDecision {
    let mut flags = 0u32;
    if platform {
        flags |= 1;
    }
    if control {
        flags |= 2;
    }
    if alt {
        flags |= 4;
    }
    if shift {
        flags |= 8;
    }
    if function {
        flags |= 16;
    }
    let Some(out) = call_extension(
        "evaluate_keystroke",
        &[
            source.to_string(),
            selection.start.to_string(),
            selection.end.to_string(),
            head.to_string(),
            tail.to_string(),
            key.to_string(),
            flags.to_string(),
        ],
    ) else {
        return KeystrokeDecision::Unhandled;
    };
    decode_keystroke_decision(&out).unwrap_or(KeystrokeDecision::Unhandled)
}

fn decode_keystroke_decision(out: &str) -> Option<KeystrokeDecision> {
    if let Some((header, replacement)) = out.split_once('\n') {
        let mut parts = header.split('\t');
        if parts.next()? == "6" {
            let start = parts.next()?.parse::<usize>().ok()?;
            let end = parts.next()?.parse::<usize>().ok()?;
            let has_ps = parts.next()? == "1";
            let ps_start = parts.next()?.parse::<usize>().ok()?;
            let ps_end = parts.next()?.parse::<usize>().ok()?;
            return Some(KeystrokeDecision::SourceEdit {
                edit_range: start..end,
                replacement: replacement.to_string(),
                post_selection: has_ps.then_some(ps_start..ps_end),
            });
        }
    }
    let mut parts = out.split('\t');
    match parts.next()? {
        "0" => Some(KeystrokeDecision::Unhandled),
        "1" => Some(KeystrokeDecision::HandledNoOp),
        "2" => Some(KeystrokeDecision::TogglePrettyMode),
        "3" => {
            let s = parts.next()?.parse::<usize>().ok()?;
            let e = parts.next()?.parse::<usize>().ok()?;
            Some(KeystrokeDecision::ClipboardCut(s..e))
        }
        "4" => {
            let s = parts.next()?.parse::<usize>().ok()?;
            let e = parts.next()?.parse::<usize>().ok()?;
            Some(KeystrokeDecision::ClipboardPaste(s..e))
        }
        "5" => {
            let s = parts.next()?.parse::<usize>().ok()?;
            let e = parts.next()?.parse::<usize>().ok()?;
            Some(KeystrokeDecision::SelectRange(s..e))
        }
        _ => None,
    }
}

pub fn surrounding_code_span_delimiters(
    source: &str,
    content_range: &Range<usize>,
) -> Option<(Range<usize>, Range<usize>)> {
    let out = call_extension(
        "surrounding_code_span_delimiters",
        &[
            source.to_string(),
            content_range.start.to_string(),
            content_range.end.to_string(),
        ],
    )?;
    let mut parts = out.split('\t');
    if parts.next()? != "1" {
        return None;
    }
    let os = parts.next()?.parse::<usize>().ok()?;
    let oe = parts.next()?.parse::<usize>().ok()?;
    let cs = parts.next()?.parse::<usize>().ok()?;
    let ce = parts.next()?.parse::<usize>().ok()?;
    Some((os..oe, cs..ce))
}

pub fn cycle_code_block_language(
    source: &str,
    block_range: Range<usize>,
    current_lang: &str,
) -> Option<(Range<usize>, String)> {
    let out = call_extension(
        "cycle_code_block_language",
        &[
            source.to_string(),
            block_range.start.to_string(),
            block_range.end.to_string(),
            current_lang.to_string(),
        ],
    )?;
    let (header, replacement) = out.split_once('\n')?;
    let mut parts = header.split('\t');
    if parts.next()? != "1" {
        return None;
    }
    let start = parts.next()?.parse::<usize>().ok()?;
    let end = parts.next()?.parse::<usize>().ok()?;
    Some((start..end, replacement.to_string()))
}

pub fn extract_html_attr(tag: &str, attr_name: &str) -> Option<String> {
    let out = call_extension(
        "extract_html_attr",
        &[tag.to_string(), attr_name.to_string()],
    )?;
    let (flag, val) = out.split_once('\n')?;
    (flag == "1").then(|| val.to_string())
}

pub fn percent_decode_str(input: &str) -> String {
    call_extension("percent_decode", &[input.to_string()]).unwrap_or_else(|| input.to_string())
}
