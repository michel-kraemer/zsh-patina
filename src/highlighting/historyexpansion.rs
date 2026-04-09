use std::borrow::Cow;

use syntect::parsing::{Scope, ScopeStackOp};

use crate::highlighting::EXPANSION_HISTORY;

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
                i = consume_substring(chars, i + 1)?;
                if i < chars.len() && chars[i].1 == '?' {
                    Some(i + 1)
                } else {
                    Some(i)
                }
            }
        }

        '{' => {
            if i + 1 == chars.len() {
                Some(i + 1)
            } else {
                // ignore what the history expansion looks like, just skip ahead
                // to the end
                let mut j = i + 1;
                while j < chars.len() && chars[j].1 != '}' {
                    j += 1;
                }
                if j == chars.len() {
                    Some(j)
                } else {
                    Some(j + 1)
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
/// next '?' character). Returns the index after the substring if successful, or
/// None if there is no valid substring designator at index i.
fn consume_substring(chars: &[(usize, char)], i: usize) -> Option<usize> {
    let mut j = i;
    while j < chars.len() && chars[j].1 != '?' {
        j += 1;
    }
    if j > i { Some(j) } else { None }
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

    /// `true` if we're currently inside a single-quoted string. This also
    /// includes POSIX-quoted strings.
    inside_single_quotes: bool,

    /// `true` if history expansion has been disabled for the rest of the
    /// command line using the character sequence `!"`.
    disabled: bool,

    /// `true` if the first non-empty line of a multi-line command has been
    /// consumed
    fist_non_empty_line_consumed: bool,

    /// Cached scope for history expansions
    expansion_history_scope: Scope,
}

impl<'a, I> HistoryExpanded<'a, I>
where
    I: Iterator<Item = &'a str>,
{
    /// Wrap the given iterator into a `HistoryExpanded` iterator. All lines
    /// returned by the wrapped iterator will undergo history expansion.
    pub fn wrap(inner: I) -> Self {
        let expansion_history_scope = Scope::new(EXPANSION_HISTORY).unwrap();

        Self {
            inner,
            inside_single_quotes: false,
            disabled: false,
            fist_non_empty_line_consumed: false,
            expansion_history_scope,
        }
    }

    fn handle_expansion(
        &self,
        next: &str,
        byte_start_index: usize,
        byte_end_index: usize,
        last_byte_start_index: usize,
        modified: &mut String,
        expansions: &mut Vec<(usize, ExpansionOp)>,
    ) {
        if byte_start_index > last_byte_start_index {
            modified.push_str(&next[last_byte_start_index..byte_start_index]);
        }
        modified.push_str(&" ".repeat(byte_end_index - byte_start_index));

        expansions.push((
            byte_start_index,
            ExpansionOp::Push(self.expansion_history_scope),
        ));
        expansions.push((byte_end_index, ExpansionOp::Pop));
    }
}

impl<'a, I> Iterator for HistoryExpanded<'a, I>
where
    I: Iterator<Item = &'a str>,
{
    type Item = (Cow<'a, str>, HistoryExpansions);

    fn next(&mut self) -> Option<(Cow<'a, str>, HistoryExpansions)> {
        let next = self.inner.next()?;

        if self.disabled {
            return Some((Cow::Borrowed(next), HistoryExpansions::empty()));
        }

        if !self.inside_single_quotes && !next.contains('!') && !next.trim_start().starts_with('^')
        {
            return Some((Cow::Borrowed(next), HistoryExpansions::empty()));
        }

        let mut expansions = Vec::new();
        let mut modified = String::new();
        let mut last_byte_start_index = 0;
        let chars = next.char_indices().collect::<Vec<_>>();
        let mut i = 0;

        if !self.fist_non_empty_line_consumed {
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
                let byte_start_index = chars[i].0;
                let byte_end_index = if char_end_index == chars.len() {
                    next.len()
                } else {
                    chars[char_end_index].0
                };

                self.handle_expansion(
                    next,
                    byte_start_index,
                    byte_end_index,
                    last_byte_start_index,
                    &mut modified,
                    &mut expansions,
                );

                last_byte_start_index = byte_end_index;
                i = char_end_index;
            }

            self.fist_non_empty_line_consumed = true;
        }

        while i < chars.len() {
            if self.inside_single_quotes || chars[i].1 == '\'' {
                let start = if self.inside_single_quotes { i } else { i + 1 };

                // Look for the end of single quotes
                match consume_until_non_escaped(&chars, start, '\'') {
                    Some(j) => {
                        // just skip everything inside single quotes on the same
                        // line, also skip the trailing single quote
                        i = j + 1;
                        self.inside_single_quotes = false;
                    }
                    None => {
                        // No end of single quoted string found on this line.
                        // Look for it in the next line.
                        self.inside_single_quotes = true;
                        break;
                    }
                }
            } else if chars[i].1 == '\\' && i < chars.len() - 1 && chars[i + 1].1 == '!' {
                // skip escaped bang '!'
                i += 2;
            } else if chars[i].1 == '!' && i < chars.len() - 1 && chars[i + 1].1 == '"' {
                // disable history expansion for the rest of the command line
                self.disabled = true;

                let byte_start_index = chars[i].0;
                let byte_end_index = byte_start_index + 2;

                self.handle_expansion(
                    next,
                    byte_start_index,
                    byte_end_index,
                    last_byte_start_index,
                    &mut modified,
                    &mut expansions,
                );

                last_byte_start_index = byte_end_index;
                break;
            } else if chars[i].1 == '!'
                && let Some(char_end_index) = consume_history_expansion(&chars, i)
            {
                let byte_start_index = chars[i].0;
                let byte_end_index = if char_end_index == chars.len() {
                    next.len()
                } else {
                    chars[char_end_index].0
                };

                self.handle_expansion(
                    next,
                    byte_start_index,
                    byte_end_index,
                    last_byte_start_index,
                    &mut modified,
                    &mut expansions,
                );

                last_byte_start_index = byte_end_index;
                i = char_end_index;
            } else {
                i += 1;
            }
        }
        modified.push_str(&next[last_byte_start_index..]);

        Some((Cow::Owned(modified), HistoryExpansions::new(expansions)))
    }
}

#[derive(Debug)]
enum ExpansionOp {
    Push(Scope),
    Pop,
}

#[derive(Debug)]
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
                    result.push((ni.0, ScopeStackOp::Push(s)));
                }
                ExpansionOp::Pop => {
                    // a pop of a history expansion must happen before (<) all
                    // other operations at the same location
                    while let Some(nj) = j.peek()
                        && nj.0 < ni.0
                    {
                        result.push(j.next().unwrap());
                    }
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

    use crate::highlighting::historyexpansion::{ExpansionOp, HistoryExpanded, HistoryExpansions};

    fn assert_history_expansions(
        expected: &[(&str, Vec<(usize, usize)>)],
        history_expansions: Vec<(Cow<'_, str>, HistoryExpansions)>,
    ) {
        assert_eq!(expected.len(), history_expansions.len());
        for (e, he) in expected.iter().zip(history_expansions) {
            assert_eq!(e.0, he.0);
            assert_eq!(e.1.len() * 2, he.1.ops.len(), "{:?} != {:?}", e.1, he.1);
            for (r, o) in e.1.iter().zip(he.1.ops.chunks(2)) {
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

    fn assert_expanded(input: &str, expected: &[(&str, Vec<(usize, usize)>)]) {
        let he = HistoryExpanded::wrap(input.lines()).collect::<Vec<_>>();
        assert_history_expansions(expected, he);
    }

    #[test]
    fn last_command() {
        assert_expanded("!!", &[("  ", vec![(0, 2)])]);
        assert_expanded("ls !!", &[("ls   ", vec![(3, 5)])]);
        assert_expanded("ls; !!", &[("ls;   ", vec![(4, 6)])]);
        assert_expanded(
            r#"echo !! "!!""#,
            &[(r#"echo    "  ""#, vec![(5, 7), (9, 11)])],
        );
    }

    #[test]
    fn previous_command_by_number() {
        assert_expanded("!4", &[("  ", vec![(0, 2)])]);
        assert_expanded("echo !4hello", &[("echo   hello", vec![(5, 7)])]);
        assert_expanded("!20", &[("   ", vec![(0, 3)])]);
        assert_expanded("echo !20hello", &[("echo    hello", vec![(5, 8)])]);
        assert_expanded("!-4", &[("   ", vec![(0, 3)])]);
        assert_expanded("echo !-4hello", &[("echo    hello", vec![(5, 8)])]);
        assert_expanded("!-20", &[("    ", vec![(0, 4)])]);
        assert_expanded("echo !-20hello", &[("echo     hello", vec![(5, 9)])]);
        assert_expanded("!-20 foobar", &[("     foobar", vec![(0, 4)])]);
        assert_expanded("!4echo", &[("  echo", vec![(0, 2)])]);
    }

    #[test]
    fn previous_command_by_str() {
        assert_expanded("!a", &[("  ", vec![(0, 2)])]);
        assert_expanded("!a param", &[("   param", vec![(0, 2)])]);
        assert_expanded("!ls", &[("   ", vec![(0, 3)])]);
        assert_expanded("!?echo", &[("      ", vec![(0, 6)])]);
        assert_expanded("!ls?", &[("    ", vec![(0, 4)])]);
        assert_expanded("!?ls", &[("    ", vec![(0, 4)])]);
        assert_expanded("!?ls?", &[("     ", vec![(0, 5)])]);
        assert_expanded("!?ls:$?", &[("       ", vec![(0, 7)])]);
        assert_expanded(
            "echo !ls:s/ls/ll/ && echo !:&",
            &[("echo              && echo    ", vec![(5, 17), (26, 29)])],
        );
    }

    #[test]
    fn current_command() {
        assert_expanded("echo !# hello", &[("echo    hello", vec![(5, 7)])]);
        assert_expanded("echo !#hello", &[("echo   hello", vec![(5, 7)])]);
    }

    #[test]
    fn insulate() {
        assert_expanded("echo !{ls}hello", &[("echo      hello", vec![(5, 10)])]);
    }

    #[test]
    fn word_designators() {
        assert_expanded("vi !!:0", &[("vi     ", vec![(3, 7)])]);
        assert_expanded("vi !!:20", &[("vi      ", vec![(3, 8)])]);
        assert_expanded("vi !!:20.bak", &[("vi      .bak", vec![(3, 8)])]);
        assert_expanded("vi !!:$", &[("vi     ", vec![(3, 7)])]);
        assert_expanded("vi !!:$.bak", &[("vi     .bak", vec![(3, 7)])]);
        assert_expanded("vi !!:^", &[("vi     ", vec![(3, 7)])]);
        assert_expanded("vi !!:^.bak", &[("vi     .bak", vec![(3, 7)])]);
        assert_expanded("vi !!:%", &[("vi     ", vec![(3, 7)])]);
        assert_expanded("vi !^", &[("vi   ", vec![(3, 5)])]);
        assert_expanded("vi !-", &[("vi   ", vec![(3, 5)])]);
        assert_expanded("!% stop", &[("   stop", vec![(0, 2)])]);
        assert_expanded("vi !*", &[("vi   ", vec![(3, 5)])]);
        assert_expanded("vi !*10", &[("vi   10", vec![(3, 5)])]);
        assert_expanded("vi !^.bak", &[("vi   .bak", vec![(3, 5)])]);
        assert_expanded(
            "command !!:$ next parameter",
            &[("command      next parameter", vec![(8, 12)])],
        );
        assert_expanded("ls -l !!:$", &[("ls -l     ", vec![(6, 10)])]);

        assert_expanded("!zsh-patina", &[("     patina", vec![(0, 5)])]);
        assert_expanded(
            r#"echo "!?cbr?-i""#,
            &[(r#"echo "       i""#, vec![(6, 13)])],
        );
    }

    #[test]
    fn both_designators() {
        assert_expanded("ls -l !cp:2", &[("ls -l      ", vec![(6, 11)])]);
        assert_expanded("ls -l !cp:$", &[("ls -l      ", vec![(6, 11)])]);
        assert_expanded("ls -l !cp:^", &[("ls -l      ", vec![(6, 11)])]);
        assert_expanded("ls -l !cp:*", &[("ls -l      ", vec![(6, 11)])]);
        assert_expanded("ls -l !cp:2*", &[("ls -l       ", vec![(6, 12)])]);
        assert_expanded("ls -l !cp:2*.bak", &[("ls -l       .bak", vec![(6, 12)])]);
        assert_expanded("ls -l !tar:3-5", &[("ls -l         ", vec![(6, 14)])]);
        assert_expanded("ls -l !tar:2-$", &[("ls -l         ", vec![(6, 14)])]);
        assert_expanded("ls -l !tar:2-", &[("ls -l        ", vec![(6, 13)])]);
        assert_expanded("ls -l !tar:$-", &[("ls -l        ", vec![(6, 13)])]);
        assert_expanded("ls -l !tar:^-", &[("ls -l        ", vec![(6, 13)])]);
        assert_expanded("ls -l !tar:%-", &[("ls -l        ", vec![(6, 13)])]);
        assert_expanded(
            "tar cvfz new-file.tar !tar:3-:p",
            &[("tar cvfz new-file.tar          ", vec![(22, 31)])],
        );
    }

    #[test]
    fn modifiers() {
        assert_expanded("ls -l !!:$:r", &[("ls -l       ", vec![(6, 12)])]);
        assert_expanded("ls -l !!:$:h", &[("ls -l       ", vec![(6, 12)])]);
        assert_expanded("ls -l !!:$:h30", &[("ls -l         ", vec![(6, 14)])]);
        assert_expanded("ls -l !!:$:t", &[("ls -l       ", vec![(6, 12)])]);
        assert_expanded("ls -l !!:$:t30", &[("ls -l         ", vec![(6, 14)])]);
        assert_expanded("!!:&", &[("    ", vec![(0, 4)])]);
        assert_expanded("!!:g&", &[("     ", vec![(0, 5)])]);
    }

    #[test]
    fn substitutions() {
        assert_expanded("!!:s/ls -l/cat", &[("              ", vec![(0, 14)])]);
        assert_expanded("!!:s/ls -l/cat/", &[("               ", vec![(0, 15)])]);
        assert_expanded(
            "!!:s/ls -l/cat param",
            &[("                    ", vec![(0, 20)])],
        );
        assert_expanded(
            "!!:s/ls -l/cat/ param",
            &[("                param", vec![(0, 15)])],
        );
        assert_expanded(
            r#"!!:s/ls \//ls .\//"#,
            &[(r#"                  "#, vec![(0, 18)])],
        );
        assert_expanded("!!:gs/foo/bar", &[("             ", vec![(0, 13)])]);
        assert_expanded("!!:gs/foo/bar/", &[("              ", vec![(0, 14)])]);
        assert_expanded("!!:s/foo/bar/:G", &[("               ", vec![(0, 15)])]);

        assert_expanded("!!:s^ls -l^cat", &[("              ", vec![(0, 14)])]);
        assert_expanded("!!:gs^foo^bar", &[("             ", vec![(0, 13)])]);
        assert_expanded("!!:s#foo#bar", &[("            ", vec![(0, 12)])]);
        assert_expanded("!!:s@foo@bar@:G", &[("               ", vec![(0, 15)])]);
        assert_expanded(
            r#"!!:s@foo\@@bar@:G"#,
            &[("                 ", vec![(0, 17)])],
        );
        assert_expanded("!!:spfoopbar", &[("            ", vec![(0, 12)])]);
    }

    #[test]
    fn quick_substitutions() {
        assert_expanded("^ls -l^cat", &[("          ", vec![(0, 10)])]);
        assert_expanded("^ls -l^cat^", &[("           ", vec![(0, 11)])]);
        assert_expanded("^ls -l^cat param", &[("                ", vec![(0, 16)])]);
        assert_expanded("^ls -l^cat^ param", &[("            param", vec![(0, 11)])]);
        assert_expanded(r#"^ls \^^ls ./^"#, &[(r#"             "#, vec![(0, 13)])]);
        assert_expanded("^foo^bar^:G", &[("           ", vec![(0, 11)])]);
        assert_expanded(
            "^foo^bar^:G echo !!",
            &[("            echo   ", vec![(0, 11), (17, 19)])],
        );
    }

    #[test]
    fn unicode() {
        assert_expanded(
            "foobar 😎 !:20 -r hello",
            &[("foobar 😎      -r hello", vec![(12, 16)])],
        );
        assert_expanded(
            "foobar !😎:20 -r hello",
            &[("foobar          -r hello", vec![(7, 15)])],
        );
    }

    #[test]
    fn single_quotes() {
        assert_expanded(
            "echo !! 'Hello!!' !! world",
            &[("echo    'Hello!!'    world", vec![(5, 7), (18, 20)])],
        );
        assert_expanded(
            "echo !! $'Hello!!' !! world",
            &[("echo    $'Hello!!'    world", vec![(5, 7), (19, 21)])],
        );
        assert_expanded(
            r#"echo !! 'Hello!!\'!!' !! world"#,
            &[(r#"echo    'Hello!!\'!!'    world"#, vec![(5, 7), (22, 24)])],
        );

        assert_expanded(
            "echo !! 'Hello\n!!' !! world",
            &[
                ("echo    'Hello", vec![(5, 7)]),
                ("!!'    world", vec![(4, 6)]),
            ],
        );

        assert_expanded(
            "echo !! 'Hello\n' \n !! world",
            &[
                ("echo    'Hello", vec![(5, 7)]),
                ("' ", vec![]),
                ("    world", vec![(1, 3)]),
            ],
        );
    }

    #[test]
    fn escaped_bang() {
        assert_expanded(
            r#"echo Hello\!ls:1 world"#,
            &[(r#"echo Hello\!ls:1 world"#, vec![])],
        );
    }

    #[test]
    fn no_history_expansion_at_end() {
        assert_expanded("echo Hello!", &[("echo Hello!", vec![])]);
    }

    #[test]
    fn no_history_expansion_if_followed_by() {
        assert_expanded("!=", &[("!=", vec![])]);
        assert_expanded("!(", &[("!(", vec![])]);
    }

    #[test]
    fn bang_without_history_expansion() {
        assert_expanded("echo Hello! world", &[("echo Hello! world", vec![])]);
    }

    #[test]
    fn disable() {
        assert_expanded(
            r#"echo !! !" !!; echo !!"#,
            &[(r#"echo       !!; echo !!"#, vec![(5, 7), (8, 10)])],
        );
        assert_expanded(
            r#"echo "!!" !" !!; echo !!"#,
            &[(r#"echo "  "    !!; echo !!"#, vec![(6, 8), (10, 12)])],
        );
        assert_expanded(r#"echo !"Hello!"#, &[(r#"echo   Hello!"#, vec![(5, 7)])]);
        assert_expanded(
            r#"echo !"Hello!; echo !!"#,
            &[(r#"echo   Hello!; echo !!"#, vec![(5, 7)])],
        );
        assert_expanded(
            r#"echo !"Hello!"world""#,
            &[(r#"echo   Hello!"world""#, vec![(5, 7)])],
        );
        assert_expanded(
            r#"echo OK !! && echo !"Hello! && echo !!"#,
            &[(
                r#"echo OK    && echo   Hello! && echo !!"#,
                vec![(8, 10), (19, 21)],
            )],
        );

        // multi-line
        assert_expanded(
            "echo OK !! && echo !\"Hello! &&\necho !!",
            &[
                (r#"echo OK    && echo   Hello! &&"#, vec![(8, 10), (19, 21)]),
                (r#"echo !!"#, vec![]),
            ],
        );

        // escaped
        assert_expanded(r#"echo "Hello\!""#, &[(r#"echo "Hello\!""#, vec![])]);
        assert_expanded(
            r#"echo "Hello\!" !!"#,
            &[(r#"echo "Hello\!"   "#, vec![(15, 17)])],
        );
    }
}
