use std::ops::Range;

mod dynamic;
mod highlighter;
mod historyexpansion;

pub use highlighter::{Highlighter, HighlighterBuilder, HighlightingRequest};

use crate::theme::Theme;

const ARGUMENTS: &str = "meta.function-call.arguments.shell";
const DYNAMIC_PATH_DIRECTORY_COMPLETE: &str = "dynamic.path.directory.complete.shell";
const DYNAMIC_PATH_DIRECTORY_PARTIAL: &str = "dynamic.path.directory.partial.shell";
const DYNAMIC_PATH_FILE_COMPLETE: &str = "dynamic.path.file.complete.shell";
const DYNAMIC_PATH_FILE_PARTIAL: &str = "dynamic.path.file.partial.shell";

const CALLABLE: &str = "variable.function.shell";
const DYNAMIC_CALLABLE_ALIAS: &str = "dynamic.callable.alias.shell";
const DYNAMIC_CALLABLE_BUILTIN: &str = "dynamic.callable.builtin.shell";
const DYNAMIC_CALLABLE_COMMAND: &str = "dynamic.callable.command.shell";
const DYNAMIC_CALLABLE_FUNCTION: &str = "dynamic.callable.function.shell";
const DYNAMIC_CALLABLE_MISSING: &str = "dynamic.callable.missing.shell";

const EXPANSION_HISTORY: &str = "meta.group.expansion.history.shell";

const CHARACTER_ESCAPE: &str = "constant.character.escape.shell";
const TILDE_VARIABLE: &str = "variable.language.tilde.shell";
const TILDE_META: &str = "meta.group.expansion.tilde";

const STRING_QUOTED_SINGLE: &str = "string.quoted.single.shell";
const STRING_QUOTED_SINGLE_ANSI: &str = "string.quoted.single.ansi-c.shell";
const STRING_QUOTED_DOUBLE: &str = "string.quoted.double.shell";
const STRING_QUOTED_BEGIN: &str = "punctuation.definition.string.begin.shell";
const STRING_QUOTED_END: &str = "punctuation.definition.string.end.shell";
const STRING_UNQUOTED_HEREDOC: &str = "string.unquoted.heredoc.shell";

const REDIRECTION: &str = "keyword.operator.assignment.redirection.shell";

#[cfg(test)]
const COMMENT: &str = "comment.line.number-sign.shell";
#[cfg(test)]
const KEYWORD_BUILTIN_BUILTIN: &str = "keyword.builtin.builtin.shell";
#[cfg(test)]
const KEYWORD_BUILTIN_COMMAND: &str = "keyword.builtin.command.shell";
#[cfg(test)]
const KEYWORD_BUILTIN_DASH: &str = "keyword.builtin.dash.shell";
#[cfg(test)]
const KEYWORD_BUILTIN_EXEC: &str = "keyword.builtin.exec.shell";
#[cfg(test)]
const KEYWORD_BUILTIN_NOGLOB: &str = "keyword.builtin.noglob.shell";
#[cfg(test)]
const CONTROL_BREAK: &str = "keyword.control.break.shell";
#[cfg(test)]
const CONTROL_CASE: &str = "keyword.control.case.shell";
#[cfg(test)]
const CONTROL_CASE_ITEM: &str = "keyword.control.case.item.shell";
#[cfg(test)]
const CONTROL_DO: &str = "keyword.control.do.shell";
#[cfg(test)]
const CONTROL_DONE: &str = "keyword.control.done.shell";
#[cfg(test)]
const CONTROL_END: &str = "keyword.control.end.shell";
#[cfg(test)]
const CONTROL_ESAC: &str = "keyword.control.esac.shell";
#[cfg(test)]
const CONTROL_FOREACH: &str = "keyword.control.foreach.shell";
#[cfg(test)]
const CONTROL_IN: &str = "keyword.control.in.shell";
#[cfg(test)]
const CONTROL_NOCORRECT: &str = "keyword.control.flow.nocorrect.shell";
#[cfg(test)]
const CONTROL_REPEAT: &str = "keyword.control.flow.repeat.shell";
#[cfg(test)]
const CONTROL_SELECT: &str = "keyword.control.select.shell";
#[cfg(test)]
const CONTROL_TIME: &str = "keyword.control.flow.time.shell";
#[cfg(test)]
const ENVIRONMENT_VARIABLE: &str = "variable.other.readwrite.shell";
#[cfg(test)]
const OPERATOR_ARITHMETIC: &str = "keyword.operator.arithmetic.shell";
#[cfg(test)]
const OPERATOR_LOGICAL_AND: &str = "keyword.operator.logical.and.shell";
#[cfg(test)]
const OPERATOR_LOGICAL_CONTINUE: &str = "keyword.operator.logical.continue.shell";
#[cfg(test)]
const OPERATOR_REGEXP_QUANTIFIER: &str = "keyword.operator.regexp.quantifier.shell";
#[cfg(test)]
const PARAMETER: &str = "variable.parameter.option.shell";

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
