use std::iter::Peekable;
use std::str::Chars;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Sequence {
    Whitespace(String),
    Word { chars: String, original_len: usize },
}

pub struct UnbackslashIter<'a> {
    chars: Peekable<Chars<'a>>,
}

impl<'a> UnbackslashIter<'a> {
    fn new(s: &'a str) -> Self {
        Self {
            chars: s.chars().peekable(),
        }
    }
}

impl Iterator for UnbackslashIter<'_> {
    type Item = Sequence;

    fn next(&mut self) -> Option<Self::Item> {
        let c = *self.chars.peek()?;

        if c.is_whitespace() {
            let mut ws = String::new();
            while let Some(&c) = self.chars.peek() {
                if c.is_whitespace() {
                    ws.push(c);
                    self.chars.next();
                } else {
                    break;
                }
            }
            Some(Sequence::Whitespace(ws))
        } else {
            let mut word = String::new();
            let mut original_len = 0;
            loop {
                match self.chars.peek() {
                    Some('\\') => {
                        self.chars.next();
                        original_len += 1;
                        if let Some(&next) = self.chars.peek() {
                            word.push(next);
                            original_len += 1;
                            self.chars.next();
                        }
                    }
                    Some(&c) if !c.is_whitespace() => {
                        word.push(c);
                        original_len += 1;
                        self.chars.next();
                    }
                    _ => break,
                }
            }
            Some(Sequence::Word {
                chars: word,
                original_len,
            })
        }
    }
}

pub trait Unbackslash {
    fn unbackslash(&self) -> String;
    fn unbackslash_split(&self) -> UnbackslashIter<'_>;
}

impl Unbackslash for str {
    fn unbackslash(&self) -> String {
        let mut result = String::new();
        let mut chars = self.chars();
        while let Some(c) = chars.next() {
            if c == '\\' {
                if let Some(next) = chars.next() {
                    result.push(next);
                }
            } else {
                result.push(c);
            }
        }
        result
    }

    fn unbackslash_split(&self) -> UnbackslashIter<'_> {
        UnbackslashIter::new(self)
    }
}

impl Unbackslash for String {
    fn unbackslash(&self) -> String {
        self.as_str().unbackslash()
    }

    fn unbackslash_split(&self) -> UnbackslashIter<'_> {
        UnbackslashIter::new(self.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn simple_words() {
        let result = "hello world".unbackslash_split().collect::<Vec<_>>();
        assert_eq!(
            result,
            vec![
                Sequence::Word {
                    chars: "hello".into(),
                    original_len: 5
                },
                Sequence::Whitespace(" ".into()),
                Sequence::Word {
                    chars: "world".into(),
                    original_len: 5
                },
            ]
        );
    }

    #[test]
    fn escaped() {
        let result = r"hello\ world".unbackslash_split().collect::<Vec<_>>();
        assert_eq!(
            result,
            vec![Sequence::Word {
                chars: "hello world".into(),
                original_len: 12
            },]
        );

        let result = r"he\llo".unbackslash_split().collect::<Vec<_>>();
        assert_eq!(
            result,
            vec![Sequence::Word {
                chars: "hello".into(),
                original_len: 6
            },]
        );
    }

    #[test]
    fn multiple_whitespace() {
        let result = "a  \t b".unbackslash_split().collect::<Vec<_>>();
        assert_eq!(
            result,
            vec![
                Sequence::Word {
                    chars: "a".into(),
                    original_len: 1
                },
                Sequence::Whitespace("  \t ".into()),
                Sequence::Word {
                    chars: "b".into(),
                    original_len: 1
                },
            ]
        );
    }

    #[test]
    fn trailing_backslash() {
        let result = "abc\\".unbackslash_split().collect::<Vec<_>>();
        assert_eq!(
            result,
            vec![Sequence::Word {
                chars: "abc".into(),
                original_len: 4
            },]
        );
    }

    #[test]
    fn leading_trailing_whitespace() {
        let result = "  hi".unbackslash_split().collect::<Vec<_>>();
        assert_eq!(
            result,
            vec![
                Sequence::Whitespace("  ".into()),
                Sequence::Word {
                    chars: "hi".into(),
                    original_len: 2
                },
            ]
        );

        let result = "hi   ".unbackslash_split().collect::<Vec<_>>();
        assert_eq!(
            result,
            vec![
                Sequence::Word {
                    chars: "hi".into(),
                    original_len: 2
                },
                Sequence::Whitespace("   ".into()),
            ]
        );

        let result = "    hi   ".unbackslash_split().collect::<Vec<_>>();
        assert_eq!(
            result,
            vec![
                Sequence::Whitespace("    ".into()),
                Sequence::Word {
                    chars: "hi".into(),
                    original_len: 2
                },
                Sequence::Whitespace("   ".into()),
            ]
        );
    }

    #[test]
    fn multiple() {
        let s = String::from(r"\a\b\c a\ b\ c");
        let result = s.unbackslash_split().collect::<Vec<_>>();
        assert_eq!(
            result,
            vec![
                Sequence::Word {
                    chars: "abc".into(),
                    original_len: 6
                },
                Sequence::Whitespace(" ".into()),
                Sequence::Word {
                    chars: "a b c".into(),
                    original_len: 7
                }
            ]
        );
    }

    #[test]
    fn empty_input() {
        let result = "".unbackslash_split().collect::<Vec<_>>();
        assert_eq!(result, vec![]);
    }

    #[test]
    fn unbackslash() {
        assert_eq!(r"hello\ world".unbackslash(), "hello world");
        assert_eq!(r"he\llo".unbackslash(), "hello");
        assert_eq!("hello world".unbackslash(), "hello world");
        assert_eq!("abc\\".unbackslash(), "abc");
        assert_eq!("".unbackslash(), "");
        assert_eq!("-\\a\\b\\c".unbackslash(), "-abc");
        assert_eq!("a\\\\b\\c".unbackslash(), "a\\bc");
    }
}
