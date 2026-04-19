use std::borrow::Cow;

use syntect::parsing::{Scope, ScopeStackOp};

use crate::highlighting::{
    EXPANSION_HISTORY, STRING_QUOTED_SINGLE, STRING_QUOTED_SINGLE_ANSI, STRING_UNQUOTED_HEREDOC,
};

/// Test if a character is a valid character in a history expansion string
fn is_string_character(c: char) -> bool {
    !c.is_whitespace()
        && c != ':'
        && c != ';'
        && c != '^'
        && c != '$'
        && c != '*'
        && c != '-'
        && c != '%'
        && c != '"'
}

/// Consume a bang character '!'. Returns the index after the bang if
/// successful, or None if the character at index i is not a bang.
fn consume_bang(chars: &[(usize, char)], i: usize) -> Option<usize> {
    if chars[i].1 == '!' { Some(i + 1) } else { None }
}

/// Consume an event designator. Returns the index after the event designator if
/// successful, or None if there is no valid event designator at index i.
fn consume_event_designator(chars: &[(usize, char)], mut i: usize) -> Option<usize> {
    match chars[i].1 {
        '=' | '(' => None,

        '!' | '#' => Some(i + 1),

        '0'..='9' => {
            if i + 1 == chars.len() {
                Some(i + 1)
            } else {
                consume_number(chars, i)
            }
        }

        '-' => {
            if i + 1 == chars.len() {
                Some(i + 1)
            } else {
                consume_number(chars, i + 1)
            }
        }

        '?' => {
            if i + 1 == chars.len() {
                Some(i + 1)
            } else {
                i = consume_substring(chars, i + 1);
                if i < chars.len() && chars[i].1 == '?' {
                    Some(i + 1)
                } else {
                    Some(i)
                }
            }
        }

        c if is_string_character(c) => {
            if i + 1 == chars.len() {
                Some(i + 1)
            } else {
                consume_string(chars, i)
            }
        }

        _ => None,
    }
}

/// Consume a modifier. Returns the index after the modifier if successful, or
/// None if there is no valid modifier at index i.
fn consume_modifier(chars: &[(usize, char)], mut i: usize) -> Option<usize> {
    // consume leading colon
    if chars[i].1 != ':' {
        return None;
    }
    i += 1;

    if i == chars.len() {
        return Some(i);
    }

    match chars[i].1 {
        'a' | 'A' | 'c' | 'e' | 'l' | 'p' | 'P' | 'q' | 'Q' | 'r' | 'u' | 'x' | '&' => Some(i + 1),

        'h' | 't' => {
            i += 1;

            // consume optional number
            if i < chars.len() && chars[i].1.is_ascii_digit() {
                consume_number(chars, i)
            } else {
                Some(i)
            }
        }

        's' => consume_substitution(chars, i),

        'g' => {
            i += 1;

            // if string does not end, g must be followed by 's' (with
            // substitution) or '&'
            if i == chars.len() {
                Some(i)
            } else if chars[i].1 == 's' {
                consume_substitution(chars, i)
            } else if chars[i].1 == '&' {
                Some(i + 1)
            } else {
                None
            }
        }

        _ => None,
    }
}

/// Consume a number starting at index i. Returns the index after the number if
/// successful, or None if there is no number at index i.
fn consume_number(chars: &[(usize, char)], i: usize) -> Option<usize> {
    let mut j = i;
    while j < chars.len() && chars[j].1.is_ascii_digit() {
        j += 1;
    }
    if j > i { Some(j) } else { None }
}

/// Consume a string starting at index i. Returns the index after the string if
/// successful, or None if there is no string at index i.
fn consume_string(chars: &[(usize, char)], i: usize) -> Option<usize> {
    let mut j = i;
    while j < chars.len() && is_string_character(chars[j].1) {
        j += 1;
    }
    if j > i { Some(j) } else { None }
}

/// Consume a substitution modifier starting at index i. Returns the index after
/// the substitution if successful, or None if there is no valid substitution at
/// index i.
fn consume_substitution(chars: &[(usize, char)], mut i: usize) -> Option<usize> {
    // consume leading 's'
    if chars[i].1 != 's' {
        return None;
    }
    i += 1;

    if i == chars.len() {
        return Some(i);
    }

    consume_substitution_without_leading(chars, i)
}

/// Consume a substitution modifier starting at index i without the leading
/// character (typically 's'). Returns the index after the substitution if
/// successful, or None if there is no valid substitution at index i.
fn consume_substitution_without_leading(chars: &[(usize, char)], mut i: usize) -> Option<usize> {
    // consume separation character
    let separation_char = chars[i].1;
    i += 1;

    if i == chars.len() {
        return Some(i);
    }

    // consume string to replace (including the next separation character)
    if let Some(j) = consume_until_non_escaped(chars, i, separation_char) {
        i = j + 1;
    } else {
        return Some(chars.len());
    };

    if i == chars.len() {
        return Some(i);
    }

    // consume replacement string (including the next separation character)
    if let Some(j) = consume_until_non_escaped(chars, i, separation_char) {
        i = j + 1;
    } else {
        return Some(chars.len());
    };

    if i == chars.len() {
        return Some(i);
    }

    // consume optional ":G" modifier
    if chars[i].1 == ':' {
        i += 1;
        if i == chars.len() {
            return Some(i);
        }
        if chars[i].1 != 'G' {
            return None;
        }
        i += 1;
    }

    Some(i)
}

/// Consume a substring designator starting at index i (until and excluding the
/// next '?' character). Returns the index after the substring if successful.
/// Note that the substring can be empty.
fn consume_substring(chars: &[(usize, char)], mut i: usize) -> usize {
    while i < chars.len() && chars[i].1 != '?' {
        i += 1;
    }
    i
}

/// Consume characters until the first non-escaped occurrence of the character
/// `until`. Returns the index of the `until` character if successful, or None
/// if there is no non-escaped `until` character after index i.
fn consume_until_non_escaped(chars: &[(usize, char)], mut i: usize, until: char) -> Option<usize> {
    let mut backslash = false;
    loop {
        if i == chars.len() {
            break None;
        }
        if backslash {
            backslash = false;
        } else {
            if chars[i].1 == until {
                break Some(i);
            } else if chars[i].1 == '\\' {
                backslash = true;
            }
        }
        i += 1;
    }
}

/// Consume a word designator starting at index i. Returns the index after the
/// word designator if successful, or None if there is no valid word designator
/// at index i.
fn consume_word_designator(chars: &[(usize, char)], mut i: usize) -> Option<usize> {
    // Consume leading colon. If the colon is missing, the next character must
    // not be a digit. In other words, the colon is optional if the word
    // designator starts with one of '^', '$', '%', '-', '*'
    if chars[i].1 == ':' {
        i += 1;
        if i == chars.len() {
            return Some(i);
        }
    } else if chars[i].1.is_ascii_digit() {
        return None;
    }

    // consume range start
    let start_consumed = match chars[i].1 {
        '^' | '$' | '%' => {
            i += 1;
            if i == chars.len() {
                return Some(i);
            }
            true
        }

        c if c.is_ascii_digit() => {
            i = consume_number(chars, i).unwrap();
            if i == chars.len() {
                return Some(i);
            }
            true
        }

        _ => false,
    };

    // Consume optional '*' and return immediately if found
    if chars[i].1 == '*' {
        return Some(i + 1);
    }

    // consume '-'
    if chars[i].1 != '-' {
        return if start_consumed { Some(i) } else { None };
    }
    i += 1;

    if i == chars.len() {
        return Some(i);
    }

    // consume range end
    match chars[i].1 {
        '^' | '$' | '%' => Some(i + 1),
        c if c.is_ascii_digit() => consume_number(chars, i),
        _ => Some(i),
    }
}

/// Consume a history expansion starting at index i. Returns the index after the
/// history expansion if successful, or None if there is no valid history
/// expansion at index i.
fn consume_history_expansion(chars: &[(usize, char)], mut i: usize) -> Option<usize> {
    // consume leading bang '!'
    i = consume_bang(chars, i)?;
    if i == chars.len() {
        return None;
    }

    // insulated history expansions: !{...}
    if chars[i].1 == '{' {
        // ignore what the history expansion looks like, just skip ahead
        // to the end
        let mut j = i + 1;
        while j < chars.len() && chars[j].1 != '}' {
            j += 1;
        }
        return if j == chars.len() {
            Some(j)
        } else {
            Some(j + 1)
        };
    }

    // consume optional event designator
    let event_designator_consumed = match consume_event_designator(chars, i) {
        Some(j) => {
            i = j;
            true
        }
        None => false,
    };

    if event_designator_consumed && i == chars.len() {
        return Some(i);
    }

    // consume optional word designator
    let word_designator_consumed = match consume_word_designator(chars, i) {
        Some(j) => {
            i = j;
            true
        }
        None => false,
    };

    if word_designator_consumed && i == chars.len() {
        return Some(i);
    }

    // Consume optional modifiers. Consume as many as possible.
    let mut modifier_consumed = false;
    while i < chars.len() && chars[i].1 == ':' {
        let Some(j) = consume_modifier(chars, i) else {
            break;
        };
        modifier_consumed = true;
        i = j;
    }

    // either the event designator, the word designator, or a modifier must be
    // given
    if !event_designator_consumed && !word_designator_consumed && !modifier_consumed {
        return None;
    }

    Some(i)
}

pub struct HistoryExpanded<'a, I>
where
    I: Iterator<Item = &'a str>,
{
    /// The wrapped iterator
    inner: I,

    /// `true` if history expansion has been disabled for the rest of the
    /// command line using the character sequence `!"`.
    disabled: bool,

    /// `true` if the first non-empty line of a multi-line command has been
    /// consumed
    first_non_empty_line_consumed: bool,

    /// Cached scope for history expansions
    expansion_history_scope: Scope,

    /// Cached scope for single quotes
    string_quoted_single_scope: Scope,

    /// Cached scope for POSIX quotes
    string_quoted_single_ansi_scope: Scope,

    /// Cached scope for heredocs
    string_unquoted_heredoc_scope: Scope,
}

impl<'a, I> HistoryExpanded<'a, I>
where
    I: Iterator<Item = &'a str>,
{
    /// Wrap the given iterator into a `HistoryExpanded` iterator. All lines
    /// returned by the wrapped iterator will undergo history expansion.
    pub fn wrap(inner: I) -> Self {
        let expansion_history_scope = Scope::new(EXPANSION_HISTORY).unwrap();
        let string_quoted_single_scope = Scope::new(STRING_QUOTED_SINGLE).unwrap();
        let string_quoted_single_ansi_scope = Scope::new(STRING_QUOTED_SINGLE_ANSI).unwrap();
        let string_unquoted_heredoc_scope = Scope::new(STRING_UNQUOTED_HEREDOC).unwrap();

        Self {
            inner,
            disabled: false,
            first_non_empty_line_consumed: false,
            expansion_history_scope,
            string_quoted_single_scope,
            string_quoted_single_ansi_scope,
            string_unquoted_heredoc_scope,
        }
    }

    pub fn disable(&mut self) {
        self.disabled = true;
    }

    fn is_inside_single_quote(&self, scope_stack: &[Scope]) -> bool {
        scope_stack.iter().any(|s| {
            *s == self.string_quoted_single_scope || *s == self.string_quoted_single_ansi_scope
        })
    }

    fn is_inside_heredoc(&self, scope_stack: &[Scope]) -> bool {
        scope_stack.contains(&self.string_unquoted_heredoc_scope)
    }

    fn handle_expansion(
        &self,
        next: &str,
        chars: &[(usize, char)],
        char_range: (usize, usize),
        last_byte_start_index: &mut usize,
        modified: &mut String,
        expansions: &mut Vec<(usize, ExpansionOp)>,
    ) {
        let byte_start_index = chars[char_range.0].0;
        let byte_end_index = if char_range.1 == chars.len() {
            next.len()
        } else {
            chars[char_range.1].0
        };
        if byte_start_index > *last_byte_start_index {
            modified.push_str(&next[*last_byte_start_index..byte_start_index]);
        }

        match byte_end_index - byte_start_index {
            0 => {}

            // 1 character can only happen in case it's an incomplete quick
            // substitution at the end of a line. Since quick substitutions have
            // to appear at the beginning of the first line, this character must
            // per definition be the only character in the line, so it's safe to
            // leave it as it is. The parser will mark it as a callable and
            // produce the following operations (in the given order):
            //
            // - Push(meta.function-call.shell),
            //   - Push(variable.function.shell)
            //   - Pop
            // - Pop
            //
            // HistoryExpansions::apply() will later replace these operations
            // with a Push and a Pop for `meta.group.expansion.history.shell`.
            1 => modified.push('^'),

            // Replace history expansions with 2 or more characters with a
            // single-quoted string. This hides them from the Syntect parser.
            // Since history expansions are not allowed inside single-quoted
            // strings, this is safe. The parser will produce the following
            // operations (in the given order):
            //
            // - Push(string.quoted.single.shell),
            //   - Push(punctuation.definition.string.begin.shell)
            //   - Pop
            //   - Push(punctuation.definition.string.end.shell)
            //   - Pop
            // - Pop
            //
            // HistoryExpansions::apply() will later replace these operations
            // with a Push and a Pop for `meta.group.expansion.history.shell`.
            len => {
                modified.push('\'');
                modified.push_str(&" ".repeat(len - 2));
                modified.push('\'');
            }
        }

        if byte_end_index - byte_start_index > 0 {
            expansions.push((
                byte_start_index,
                ExpansionOp::Push(self.expansion_history_scope),
            ));
            expansions.push((byte_end_index, ExpansionOp::Pop));
        }

        *last_byte_start_index = byte_end_index;
    }

    pub fn next(&mut self, scope_stack: &[Scope]) -> Option<(Cow<'a, str>, HistoryExpansions)> {
        let next = self.inner.next()?;

        if self.disabled || self.is_inside_heredoc(scope_stack) {
            return Some((Cow::Borrowed(next), HistoryExpansions::empty()));
        }

        let mut inside_single_quotes = self.is_inside_single_quote(scope_stack);

        if !inside_single_quotes && !next.contains('!') && !next.trim_start().starts_with('^') {
            return Some((Cow::Borrowed(next), HistoryExpansions::empty()));
        }

        let mut expansions = Vec::new();
        let mut modified = String::new();
        let mut last_byte_start_index = 0;
        let chars = next.char_indices().collect::<Vec<_>>();
        let mut i = 0;

        if !self.first_non_empty_line_consumed {
            // skip leading whitespace
            while i < chars.len() && chars[i].1.is_whitespace() {
                i += 1;
            }
            if i == chars.len() {
                // Already consumed the whole line. Return immediately. With
                // this, we're not only taking a shortcut, we're also skipping
                // all leading empty lines before we set
                // self.fist_non_empty_line_consumed to true.
                return Some((Cow::Borrowed(next), HistoryExpansions::empty()));
            }

            // consume quick substitution
            if i < chars.len()
                && chars[i].1 == '^'
                && let Some(char_end_index) = consume_substitution_without_leading(&chars, i)
            {
                self.handle_expansion(
                    next,
                    &chars,
                    (i, char_end_index),
                    &mut last_byte_start_index,
                    &mut modified,
                    &mut expansions,
                );

                i = char_end_index;
            }

            self.first_non_empty_line_consumed = true;
        }

        while i < chars.len() {
            if inside_single_quotes || chars[i].1 == '\'' {
                let start = if inside_single_quotes { i } else { i + 1 };

                // Look for the end of single quotes
                match consume_until_non_escaped(&chars, start, '\'') {
                    Some(j) => {
                        // just skip everything inside single quotes on the same
                        // line, also skip the trailing single quote
                        i = j + 1;
                        inside_single_quotes = false;
                    }
                    None => {
                        // No end of single quoted string found on this line. We
                        // can stop here.
                        break;
                    }
                }
            } else if chars[i].1 == '\\' && i < chars.len() - 1 && chars[i + 1].1 == '!' {
                // skip escaped bang '!'
                i += 2;
            } else if chars[i].1 == '!' && i < chars.len() - 1 && chars[i + 1].1 == '"' {
                // disable history expansion for the rest of the command line
                self.disabled = true;

                self.handle_expansion(
                    next,
                    &chars,
                    (i, i + 2),
                    &mut last_byte_start_index,
                    &mut modified,
                    &mut expansions,
                );

                break;
            } else if chars[i].1 == '!'
                && let Some(char_end_index) = consume_history_expansion(&chars, i)
            {
                self.handle_expansion(
                    next,
                    &chars,
                    (i, char_end_index),
                    &mut last_byte_start_index,
                    &mut modified,
                    &mut expansions,
                );

                i = char_end_index;
            } else {
                i += 1;
            }
        }
        modified.push_str(&next[last_byte_start_index..]);

        Some((Cow::Owned(modified), HistoryExpansions::new(expansions)))
    }
}

#[derive(Clone, Copy, Debug)]
enum ExpansionOp {
    Push(Scope),
    Pop,
}

#[derive(Clone, Debug)]
pub struct HistoryExpansions {
    ops: Vec<(usize, ExpansionOp)>,
}

impl HistoryExpansions {
    fn empty() -> Self {
        Self { ops: Vec::new() }
    }

    fn new(ops: Vec<(usize, ExpansionOp)>) -> Self {
        Self { ops }
    }

    /// Apply this history expansions to the given stack operations. Return new
    /// operations with the history expansions mixed in at the right locations.
    pub fn apply(self, ops: Vec<(usize, ScopeStackOp)>) -> Vec<(usize, ScopeStackOp)> {
        if self.ops.is_empty() {
            return ops;
        }

        // see HistoryExpanded::handle_expansion() for more information about
        // how to handle ExpansionOps and why single-quoted strings are a
        // placeholder.
        let mut result = Vec::new();
        let mut j = ops.into_iter().peekable();
        for ni in self.ops {
            match ni.1 {
                ExpansionOp::Push(s) => {
                    // a push of a history expansion must happen after (<=) all
                    // other operations at the same location
                    while let Some(nj) = j.peek()
                        && nj.0 <= ni.0
                    {
                        result.push(j.next().unwrap());
                    }

                    // a) if the history expansion was an incomplete quick
                    // substitution consisting of 1 character, remove operations
                    // for `meta.function-call.shell` and `variable.function.shell`
                    // b) if it had 2 or more characters, remove operations for
                    // `string.quoted.single.shell` and
                    // `punctuation.definition.string.begin.shell`
                    result.pop();
                    result.pop();

                    result.push((ni.0, ScopeStackOp::Push(s)));
                }
                ExpansionOp::Pop => {
                    // Since we've replaced the history expansion with a single
                    // quoted-string, every element up to the end of the history
                    // expansion belongs to this single-quoted string and can be
                    // skipped. This also works in case the history expansion
                    // was an incomplete quick substitution consisting of 1
                    // character.
                    while let Some(nj) = j.peek()
                        && nj.0 < ni.0
                    {
                        j.next();
                    }

                    // skip next two elements, which either pop
                    // `variable.function.shell` and `meta.function-call.shell`,
                    // or `punctuation.definition.string.end.shell` and
                    // `string.quoted.single.shell`
                    j.next();
                    j.next();

                    result.push((ni.0, ScopeStackOp::Pop(1)));
                }
            }
        }
        result.extend(j);

        result
    }
}

#[cfg(test)]
mod tests {
    use std::borrow::Cow;

    use syntect::{
        parsing::{ParseState, ScopeStack, SyntaxSet},
        util::LinesWithEndings,
    };

    use crate::highlighting::historyexpansion::{ExpansionOp, HistoryExpanded, HistoryExpansions};

    fn assert_history_expansions(
        expected: &[Vec<(usize, usize)>],
        history_expansions: Vec<(Cow<'_, str>, HistoryExpansions)>,
    ) {
        assert_eq!(expected.len(), history_expansions.len());
        for (e, he) in expected.iter().zip(history_expansions) {
            let mut expected_str = he.0.to_string();
            for w in he.1.ops.chunks(2) {
                if w[1].0 - w[0].0 == 1 {
                    expected_str.replace_range(w[0].0..w[1].0, "^");
                } else {
                    expected_str.replace_range(
                        w[0].0..w[1].0,
                        &format!("'{}'", " ".repeat(w[1].0 - w[0].0 - 2)),
                    );
                }
            }
            assert_eq!(expected_str, he.0);
            assert_eq!(e.len() * 2, he.1.ops.len(), "{:?} != {:?}", e, he.1);
            for (r, o) in e.iter().zip(he.1.ops.chunks(2)) {
                assert!(
                    matches!(o[0], (start, ExpansionOp::Push(_)) if start == r.0),
                    "{:?} != ({}, Push(_))",
                    o[0],
                    r.0
                );
                assert!(
                    matches!(o[1], (end, ExpansionOp::Pop) if end == r.1),
                    "{:?} != ({}, Pop)",
                    o[1],
                    r.1
                );
            }
        }
    }

    fn assert_expanded(input: &str, expected: &[Vec<(usize, usize)>]) {
        let syntax_set: SyntaxSet = syntect::dumps::from_uncompressed_data(include_bytes!(
            concat!(env!("OUT_DIR"), "/syntax_set.packdump")
        ))
        .expect("Unable to load shell syntax");

        let syntax = syntax_set.find_syntax_by_extension("sh").unwrap();
        let mut parse_state = ParseState::new(syntax);

        let mut scope_stack = ScopeStack::new();

        let mut history_expanded =
            HistoryExpanded::wrap(LinesWithEndings::from(input.trim_ascii_end()));
        let mut he = Vec::new();
        while let Some((line, expansions)) = history_expanded.next(&scope_stack.scopes) {
            he.push((line.clone(), expansions.clone()));
            let ops = expansions.apply(parse_state.parse_line(&line, &syntax_set).unwrap());
            ops.iter().for_each(|op| scope_stack.apply(&op.1).unwrap());
        }
        assert_history_expansions(expected, he);
    }

    #[test]
    fn last_command() {
        assert_expanded("!!", &[vec![(0, 2)]]);
        assert_expanded("ls !!", &[vec![(3, 5)]]);
        assert_expanded("ls; !!", &[vec![(4, 6)]]);
        assert_expanded(r#"echo !! "!!""#, &[vec![(5, 7), (9, 11)]]);
    }

    #[test]
    fn previous_command_by_number() {
        assert_expanded("!4", &[vec![(0, 2)]]);
        assert_expanded("echo !4hello", &[vec![(5, 7)]]);
        assert_expanded("!20", &[vec![(0, 3)]]);
        assert_expanded("echo !20hello", &[vec![(5, 8)]]);
        assert_expanded("!-4", &[vec![(0, 3)]]);
        assert_expanded("echo !-4hello", &[vec![(5, 8)]]);
        assert_expanded("!-20", &[vec![(0, 4)]]);
        assert_expanded("echo !-20hello", &[vec![(5, 9)]]);
        assert_expanded("!-20 foobar", &[vec![(0, 4)]]);
        assert_expanded("!4echo", &[vec![(0, 2)]]);
    }

    #[test]
    fn previous_command_by_str() {
        assert_expanded("!a", &[vec![(0, 2)]]);
        assert_expanded("!a param", &[vec![(0, 2)]]);
        assert_expanded("!ls", &[vec![(0, 3)]]);
        assert_expanded("!?echo", &[vec![(0, 6)]]);
        assert_expanded("!ls?", &[vec![(0, 4)]]);
        assert_expanded("!?ls", &[vec![(0, 4)]]);
        assert_expanded("!?ls?", &[vec![(0, 5)]]);
        assert_expanded("!?ls:$?", &[vec![(0, 7)]]);
        assert_expanded("!??", &[vec![(0, 3)]]);
        assert_expanded("echo !ls:s/ls/ll/ && echo !:&", &[vec![(5, 17), (26, 29)]]);
    }

    #[test]
    fn current_command() {
        assert_expanded("echo !# hello", &[vec![(5, 7)]]);
        assert_expanded("echo !#hello", &[vec![(5, 7)]]);
    }

    #[test]
    fn insulate() {
        assert_expanded("echo !{ls}hello", &[vec![(5, 10)]]);
        assert_expanded("echo !{ls}-hello", &[vec![(5, 10)]]);
    }

    #[test]
    fn word_designators() {
        assert_expanded("vi !!:0", &[vec![(3, 7)]]);
        assert_expanded("vi !!:20", &[vec![(3, 8)]]);
        assert_expanded("vi !!:20.bak", &[vec![(3, 8)]]);
        assert_expanded("vi !!:$", &[vec![(3, 7)]]);
        assert_expanded("vi !!:$.bak", &[vec![(3, 7)]]);
        assert_expanded("vi !!:^", &[vec![(3, 7)]]);
        assert_expanded("vi !!:^.bak", &[vec![(3, 7)]]);
        assert_expanded("vi !!:%", &[vec![(3, 7)]]);
        assert_expanded("vi !^", &[vec![(3, 5)]]);
        assert_expanded("vi !-", &[vec![(3, 5)]]);
        assert_expanded("!% stop", &[vec![(0, 2)]]);
        assert_expanded("vi !*", &[vec![(3, 5)]]);
        assert_expanded("vi !*10", &[vec![(3, 5)]]);
        assert_expanded("vi !^.bak", &[vec![(3, 5)]]);
        assert_expanded("command !!:$ next parameter", &[vec![(8, 12)]]);
        assert_expanded("ls -l !!:$", &[vec![(6, 10)]]);
        assert_expanded("!zsh-patina", &[vec![(0, 5)]]);
        assert_expanded(r#"echo "!?cbr?-i""#, &[vec![(6, 13)]]);
    }

    #[test]
    fn both_designators() {
        assert_expanded("ls -l !cp:2", &[vec![(6, 11)]]);
        assert_expanded("ls -l !cp:$", &[vec![(6, 11)]]);
        assert_expanded("ls -l !cp:^", &[vec![(6, 11)]]);
        assert_expanded("ls -l !cp:*", &[vec![(6, 11)]]);
        assert_expanded("ls -l !cp:2*", &[vec![(6, 12)]]);
        assert_expanded("ls -l !cp:2*.bak", &[vec![(6, 12)]]);
        assert_expanded("ls -l !tar:3-5", &[vec![(6, 14)]]);
        assert_expanded("ls -l !tar:2-$", &[vec![(6, 14)]]);
        assert_expanded("ls -l !tar:2-", &[vec![(6, 13)]]);
        assert_expanded("ls -l !tar:$-", &[vec![(6, 13)]]);
        assert_expanded("ls -l !tar:^-", &[vec![(6, 13)]]);
        assert_expanded("ls -l !tar:%-", &[vec![(6, 13)]]);
        assert_expanded("tar cvfz new-file.tar !tar:3-:p", &[vec![(22, 31)]]);
    }

    #[test]
    fn modifiers() {
        assert_expanded("ls -l !!:$:r", &[vec![(6, 12)]]);
        assert_expanded("ls -l !!:$:h", &[vec![(6, 12)]]);
        assert_expanded("ls -l !!:$:h30", &[vec![(6, 14)]]);
        assert_expanded("ls -l !!:$:t", &[vec![(6, 12)]]);
        assert_expanded("ls -l !!:$:t30", &[vec![(6, 14)]]);
        assert_expanded("!!:&", &[vec![(0, 4)]]);
        assert_expanded("!!:g&", &[vec![(0, 5)]]);
    }

    #[test]
    fn substitutions() {
        assert_expanded("!!:s/ls -l/cat", &[vec![(0, 14)]]);
        assert_expanded("!!:s/ls -l/cat/", &[vec![(0, 15)]]);
        assert_expanded("!!:s/ls -l/cat param", &[vec![(0, 20)]]);
        assert_expanded("!!:s/ls -l/cat/ param", &[vec![(0, 15)]]);
        assert_expanded(r#"!!:s/ls \//ls .\//"#, &[vec![(0, 18)]]);
        assert_expanded("!!:gs/foo/bar", &[vec![(0, 13)]]);
        assert_expanded("!!:gs/foo/bar/", &[vec![(0, 14)]]);
        assert_expanded("!!:s/foo/bar/:G", &[vec![(0, 15)]]);

        assert_expanded("!!:s^ls -l^cat", &[vec![(0, 14)]]);
        assert_expanded("!!:gs^foo^bar", &[vec![(0, 13)]]);
        assert_expanded("!!:s#foo#bar", &[vec![(0, 12)]]);
        assert_expanded("!!:s@foo@bar@:G", &[vec![(0, 15)]]);
        assert_expanded(r#"!!:s@foo\@@bar@:G"#, &[vec![(0, 17)]]);
        assert_expanded("!!:spfoopbar", &[vec![(0, 12)]]);
    }

    #[test]
    fn quick_substitutions() {
        assert_expanded("^ls -l^cat", &[vec![(0, 10)]]);
        assert_expanded("^ls -l^cat^", &[vec![(0, 11)]]);
        assert_expanded("^ls -l^cat param", &[vec![(0, 16)]]);
        assert_expanded("^ls -l^cat^ param", &[vec![(0, 11)]]);
        assert_expanded(r#"^ls \^^ls ./^"#, &[vec![(0, 13)]]);
        assert_expanded("^foo^bar^:G", &[vec![(0, 11)]]);
        assert_expanded("^foo^bar^:G echo !!", &[vec![(0, 11), (17, 19)]]);
    }

    #[test]
    fn unicode() {
        assert_expanded("foobar 😎 !:20 -r hello", &[vec![(12, 16)]]);
        assert_expanded("foobar !😎:20 -r hello", &[vec![(7, 15)]]);
    }

    #[test]
    fn single_quotes() {
        assert_expanded("echo !! 'Hello!!' !! world", &[vec![(5, 7), (18, 20)]]);
        assert_expanded("echo !! $'Hello!!' !! world", &[vec![(5, 7), (19, 21)]]);
        assert_expanded(
            r#"echo !! 'Hello!!\'!!' !! world"#,
            &[vec![(5, 7), (22, 24)]],
        );

        assert_expanded(
            "echo !! 'Hello\n!!' !! world",
            &[vec![(5, 7)], vec![(4, 6)]],
        );

        assert_expanded(
            "echo !! 'Hello\n' \n !! world",
            &[vec![(5, 7)], vec![], vec![(1, 3)]],
        );

        assert_expanded(
            "'\necho !! 'Hello\necho !!",
            &[vec![], vec![], vec![(5, 7)]],
        );
        assert_expanded(
            "$'\necho !! 'Hello\necho !!",
            &[vec![], vec![], vec![(5, 7)]],
        );
    }

    #[test]
    fn new_line_after_history_expansion() {
        assert_expanded("echo !!\nHello !! world", &[vec![(5, 7)], vec![(6, 8)]]);
    }

    #[test]
    fn escaped_bang() {
        assert_expanded(r#"echo Hello\!ls:1 world"#, &[vec![]]);
    }

    #[test]
    fn no_history_expansion_at_end() {
        assert_expanded("echo Hello!", &[vec![]]);
    }

    #[test]
    fn no_history_expansion_if_followed_by() {
        assert_expanded("!=", &[vec![]]);
        assert_expanded("!(", &[vec![]]);
    }

    #[test]
    fn bang_without_history_expansion() {
        assert_expanded("echo Hello! world", &[vec![]]);
    }

    #[test]
    fn disable() {
        assert_expanded(r#"echo !! !" !!; echo !!"#, &[vec![(5, 7), (8, 10)]]);
        assert_expanded(r#"echo "!!" !" !!; echo !!"#, &[vec![(6, 8), (10, 12)]]);
        assert_expanded(r#"echo !"Hello!"#, &[vec![(5, 7)]]);
        assert_expanded(r#"echo !"Hello!; echo !!"#, &[vec![(5, 7)]]);
        assert_expanded(r#"echo !"Hello!"world""#, &[vec![(5, 7)]]);
        assert_expanded(
            r#"echo OK !! && echo !"Hello! && echo !!"#,
            &[vec![(8, 10), (19, 21)]],
        );

        // multi-line
        assert_expanded(
            "echo OK !! && echo !\"Hello! &&\necho !!",
            &[vec![(8, 10), (19, 21)], vec![]],
        );

        // escaped
        assert_expanded(r#"echo "Hello\!""#, &[vec![]]);
        assert_expanded(r#"echo "Hello\!" !!"#, &[vec![(15, 17)]]);
    }

    #[test]
    fn heredoc() {
        assert_expanded("cat <<EOF\necho !!\nEOF", &[vec![], vec![], vec![]]);
        assert_expanded("cat <<EOF\necho !!", &[vec![], vec![]]);
        assert_expanded(
            "cat <<EOF\necho !!\nEOF\necho !!",
            &[vec![], vec![], vec![], vec![(5, 7)]],
        );
        assert_expanded(
            "cat <<EOF\n^foo^bar\nEOF\necho !!",
            &[vec![], vec![], vec![], vec![(5, 7)]],
        );
        assert_expanded(
            "echo \"$(cat <<EOF\necho !!\nEOF\n)\"",
            &[vec![], vec![], vec![], vec![]],
        );
    }

    #[test]
    fn incomplete() {
        assert_expanded("^", &[vec![(0, 1)]]);
        assert_expanded("^l", &[vec![(0, 2)]]);
    }
}
