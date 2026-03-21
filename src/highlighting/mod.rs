use std::ops::Range;

mod dynamic;
mod highlighter;

pub use highlighter::Highlighter;

use crate::theme::Theme;

const ARGUMENTS: &str = "meta.function-call.arguments.shell";
const DYNAMIC_PATH_DIRECTORY: &str = "dynamic.path.directory.shell";
const DYNAMIC_PATH_FILE: &str = "dynamic.path.file.shell";

const CALLABLE: &str = "variable.function.shell";
const DYNAMIC_CALLABLE_ALIAS: &str = "dynamic.callable.alias.shell";
const DYNAMIC_CALLABLE_BUILTIN: &str = "dynamic.callable.builtin.shell";
const DYNAMIC_CALLABLE_COMMAND: &str = "dynamic.callable.command.shell";
const DYNAMIC_CALLABLE_FUNCTION: &str = "dynamic.callable.function.shell";
const DYNAMIC_CALLABLE_MISSING: &str = "dynamic.callable.missing.shell";

const CHARACTER_ESCAPE: &str = "constant.character.escape.shell";
const CHARACTER_ESCAPE_ARGUMENTS: &str =
    "meta.function-call.arguments.shell constant.character.escape.shell";
const CHARACTER_ESCAPE_QUOTED_ANSI: &str =
    "string.quoted.single.ansi-c.shell constant.character.escape.shell";
const TILDE: &str = "variable.language.tilde.shell";
const TILDE_ARGUMENTS: &str = "meta.function-call.arguments.shell variable.language.tilde.shell";
const TILDE_CALLABLE: &str = "variable.function.shell variable.language.tilde.shell";

const STRING_QUOTED_SINGLE: &str = "string.quoted.single.shell";
const STRING_QUOTED_SINGLE_ANSI: &str = "string.quoted.single.ansi-c.shell";
const STRING_QUOTED_DOUBLE: &str = "string.quoted.double.shell";
const STRING_QUOTED_BEGIN: &str = "punctuation.definition.string.begin.shell";
const STRING_QUOTED_END: &str = "punctuation.definition.string.end.shell";
const STRING_QUOTED_BEGIN_CALLABLE: &str =
    "variable.function.shell punctuation.definition.string.begin.shell";
const STRING_QUOTED_BEGIN_ARGUMENTS: &str =
    "meta.function-call.arguments.shell punctuation.definition.string.begin.shell";
const STRING_QUOTED_END_CALLABLE: &str =
    "variable.function.shell punctuation.definition.string.end.shell";
const STRING_QUOTED_END_ARGUMENTS: &str =
    "meta.function-call.arguments.shell punctuation.definition.string.end.shell";
const STRING_QUOTED_SINGLE_CALLABLE: &str = "variable.function.shell string.quoted.single.shell";
const STRING_QUOTED_SINGLE_ARGUMENTS: &str =
    "meta.function-call.arguments.shell string.quoted.single.shell";
const STRING_QUOTED_SINGLE_ANSI_CALLABLE: &str =
    "variable.function.shell string.quoted.single.ansi-c.shell";
const STRING_QUOTED_SINGLE_ANSI_ARGUMENTS: &str =
    "meta.function-call.arguments.shell string.quoted.single.ansi-c.shell";
const STRING_QUOTED_DOUBLE_CALLABLE: &str = "variable.function.shell string.quoted.double.shell";
const STRING_QUOTED_DOUBLE_ARGUMENTS: &str =
    "meta.function-call.arguments.shell string.quoted.double.shell";

/// A span of text with a foreground color. The range is specified in terms of
/// character indices, not byte indices.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Span {
    /// The starting character index of the span (inclusive)
    pub start: usize,

    /// The ending character index of the span (exclusive)
    pub end: usize,

    /// The span's style
    pub style: SpanStyle,
}

#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct StaticStyle {
    /// The foreground color of the span
    pub foreground_color: Option<String>,

    /// The background color of the span
    pub background_color: Option<String>,

    /// `true` if the text should be shown in bold
    pub bold: bool,

    /// `true` if the text should be shown underlined
    pub underline: bool,
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub enum DynamicStyle {
    Callable { parsed_callable: String },
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub enum SpanStyle {
    Static(StaticStyle),
    Dynamic(DynamicStyle),
}

/// A token with a scope, line and column number, and range in the input command
/// (byte indices). The line and column numbers are 1-based.
pub struct Token {
    /// The scope of the token (e.g. `keyword.control.for.shell`)
    pub scope: String,

    /// The line number of the token (1-based)
    pub line: usize,

    /// The column of the token (1-based)
    pub column: usize,

    /// The range of the token in the input command (byte indices)
    pub range: Range<usize>,
}

/// Lookup a scope in a theme and convert the retrieved style to a
/// [`StaticStyle`] struct
fn resolve_static_style(scope: &str, theme: &Theme) -> Option<StaticStyle> {
    let style = theme.resolve(scope)?;

    let fg = style.foreground.map(|c| c.to_ansi_color());
    let bg = style.background.map(|c| c.to_ansi_color());

    if fg.is_none() && bg.is_none() && !style.bold && !style.underline {
        None
    } else {
        Some(StaticStyle {
            foreground_color: fg,
            background_color: bg,
            bold: style.bold,
            underline: style.underline,
        })
    }
}
