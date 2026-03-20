use syntect::easy::HighlightLines;
use syntect::highlighting::{Style as SyntectStyle, Theme, ThemeSet};
use syntect::parsing::SyntaxSet;
use std::sync::OnceLock;

/// Global syntax set (language definitions) loaded once.
fn syntax_set() -> &'static SyntaxSet {
    static SYNTAX_SET: OnceLock<SyntaxSet> = OnceLock::new();
    SYNTAX_SET.get_or_init(|| SyntaxSet::load_defaults_newlines())
}

/// Global theme set loaded once, using a built-in dark theme.
fn theme() -> &'static Theme {
    static THEME: OnceLock<Theme> = OnceLock::new();
    THEME.get_or_init(|| {
        let ts = ThemeSet::load_defaults();
        ts.themes["base16-ocean.dark"].clone()
    })
}

/// Convert syntect style to ratatui style.
fn to_ratatui_style(syntect_style: SyntectStyle) -> ratatui::style::Style {
    let fg = syntect_style.foreground;
    ratatui::style::Style::default()
        .fg(ratatui::style::Color::Rgb(fg.r, fg.g, fg.b))
        .bg(ratatui::style::Color::Rgb(
            syntect_style.background.r,
            syntect_style.background.g,
            syntect_style.background.b,
        ))
}

/// Highlight a line of code with a given language.
/// Returns a vector of (ratatui style, text) spans.
pub fn highlight_line(line: &str, language: Option<&str>) -> Vec<(ratatui::style::Style, String)> {
    let lang = language.unwrap_or("txt");
    let syntax_set = syntax_set();
    let theme = theme();
    
    // Find syntax definition for language
    let syntax = syntax_set
        .find_syntax_by_token(lang)
        .or_else(|| syntax_set.find_syntax_by_extension(lang))
        .unwrap_or_else(|| syntax_set.find_syntax_plain_text());
    
    let mut highlighter = HighlightLines::new(syntax, theme);
    
    match highlighter.highlight_line(line, syntax_set) {
        Ok(ranges) => ranges
            .into_iter()
            .map(|(style, chunk)| (to_ratatui_style(style), chunk.to_string()))
            .collect(),
        Err(_) => vec![(ratatui::style::Style::default(), line.to_string())],
    }
}