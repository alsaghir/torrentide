use std::{
    collections::{BTreeMap, btree_map::Entry},
    error::Error,
    fmt::{self, Display, Formatter},
};

use crate::{Dict, Value};

/// Each nested container is one recursive call, so this bounds stack use on untrusted input.
const MAX_DEPTH: usize = 64;

/// Decodes exactly one bencode value, which must span the whole input.
///
/// # Errors
///
/// Returns a [`DecodeError`] when the input is not canonical bencode or has bytes after the value.
pub fn decode(input: &[u8]) -> Result<Value<'_>, DecodeError> {
    let mut decoder = Decoder { input, pos: 0 };
    let value = decoder.value(0)?;
    if decoder.pos == input.len() {
        Ok(value)
    } else {
        Err(decoder.error(ErrorKind::TrailingData))
    }
}

struct Decoder<'src> {
    input: &'src [u8],
    pos: usize,
}

impl<'src> Decoder<'src> {
    /// `depth` is the number of containers enclosing this value; the root is at depth 0.
    fn value(&mut self, depth: usize) -> Result<Value<'src>, DecodeError> {
        match self.peek()? {
            b'i' => self.int().map(Value::Int),
            b'0'..=b'9' => self.bytes().map(Value::Bytes),
            b'l' | b'd' if depth >= MAX_DEPTH => Err(self.error(ErrorKind::MaxDepthExceeded)),
            b'l' => self.list(depth).map(Value::List),
            b'd' => self.dict(depth).map(Value::Dict),
            other => Err(self.error(ErrorKind::UnexpectedByte(other))),
        }
    }

    /// `l<values>e`; its elements sit one level deeper than the list itself.
    fn list(&mut self, depth: usize) -> Result<Vec<Value<'src>>, DecodeError> {
        self.expect(b'l')?;
        let mut items = Vec::new();
        while self.peek()? != b'e' {
            items.push(self.value(depth + 1)?);
        }
        self.expect(b'e')?;
        Ok(items)
    }

    /// `d<key><value>...e` with byte-string keys.
    ///
    /// Unsorted keys are accepted because real-world torrents contain them; duplicates are
    /// rejected because readers that keep the first or the last value would disagree.
    fn dict(&mut self, depth: usize) -> Result<Dict<'src>, DecodeError> {
        let start = self.pos;
        self.expect(b'd')?;
        // Sorted by key, so iteration and the future encoder see canonical order.
        let mut entries = BTreeMap::new();
        // Exits via `break` at the closing `e`, or early via `return`/`?` on an error.
        loop {
            let key_start = self.pos;
            let key = match self.peek()? {
                b'e' => break,
                // A key must be a byte string, and those start with their length's digits.
                b'0'..=b'9' => self.bytes()?,
                other => return Err(self.error(ErrorKind::UnexpectedByte(other))),
            };
            // Checked before parsing the value, so a duplicate costs nothing to reject.
            let Entry::Vacant(entry) = entries.entry(key) else {
                return Err(DecodeError {
                    kind: ErrorKind::DuplicateKey,
                    offset: key_start,
                });
            };
            entry.insert(self.value(depth + 1)?);
        }
        self.expect(b'e')?;
        // `start..pos` now spans from the `d` through the closing `e`, borrowed from the input.
        Ok(Dict::new(entries, &self.input[start..self.pos]))
    }

    /// `<length>:<bytes>`; the length is canonical like an integer.
    fn bytes(&mut self) -> Result<&'src [u8], DecodeError> {
        // `3:cow` -> length 3; `digits` also rejects a missing or leading-zero length.
        let length = self.digits(self.pos)?;
        self.expect(b':')?;
        let start = self.pos;
        // `None` when the length does not fit `usize` (32-bit targets) or `start + length`
        // overflows; both mean the input cannot be that long, so they end up as `UnexpectedEnd`.
        let end = usize::try_from(length)
            .ok()
            .and_then(|length| start.checked_add(length));
        // `get` returns `None` instead of panicking when the input is shorter than `end`.
        let data = end
            .and_then(|end| self.input.get(start..end))
            .ok_or(DecodeError {
                kind: ErrorKind::UnexpectedEnd,
                offset: self.input.len(),
            })?;
        self.pos += data.len();
        // A sub-slice of the input: borrowed, not copied.
        Ok(data)
    }

    /// `i<number>e`; BEP 3 forbids leading zeros and `-0`.
    fn int(&mut self) -> Result<i64, DecodeError> {
        self.expect(b'i')?;
        let number_start = self.pos;
        let negative = self.input.get(self.pos) == Some(&b'-');
        if negative {
            self.pos += 1;
        }

        let decoded_magnitude = self.digits(number_start)?;

        if negative && decoded_magnitude == 0 {
            return Err(DecodeError {
                kind: ErrorKind::NonCanonicalNumber,
                offset: number_start,
            });
        }
        let value = if negative {
            0_i64.checked_sub_unsigned(decoded_magnitude)
        } else {
            0_i64.checked_add_unsigned(decoded_magnitude)
        };
        let value = value.ok_or(DecodeError {
            kind: ErrorKind::NumberOverflow,
            offset: number_start,
        })?;
        self.expect(b'e')?;
        Ok(value)
    }

    /// Reads a non-empty run of ASCII digits without a leading zero.
    ///
    /// `number_start` is where the whole number begins (its sign, if any) and is only used as
    /// the offset of errors, so they point at the number rather than into it.
    fn digits(&mut self, number_start: usize) -> Result<u64, DecodeError> {
        let start = self.pos;
        let mut value: u64 = 0;
        while let Some(&byte) = self.input.get(self.pos)
            && byte.is_ascii_digit()
        {
            value = value
                .checked_mul(10)
                .and_then(|value| value.checked_add(u64::from(byte - b'0')))
                .ok_or(DecodeError {
                    kind: ErrorKind::NumberOverflow,
                    offset: number_start,
                })?;
            self.pos += 1;
        }
        match self.pos - start {
            0 => Err(self.error(ErrorKind::MissingDigits)),
            1 => Ok(value),
            _ if self.input[start] == b'0' => Err(DecodeError {
                kind: ErrorKind::NonCanonicalNumber,
                offset: number_start,
            }),
            _ => Ok(value),
        }
    }

    fn peek(&self) -> Result<u8, DecodeError> {
        self.input
            .get(self.pos)
            .copied()
            .ok_or(self.error(ErrorKind::UnexpectedEnd))
    }

    fn expect(&mut self, wanted: u8) -> Result<(), DecodeError> {
        let byte = self.peek()?;
        if byte == wanted {
            self.pos += 1;
            Ok(())
        } else {
            Err(self.error(ErrorKind::UnexpectedByte(byte)))
        }
    }

    fn error(&self, kind: ErrorKind) -> DecodeError {
        DecodeError {
            kind,
            offset: self.pos,
        }
    }
}

/// Why decoding failed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum ErrorKind {
    /// The input ended before the value was complete.
    UnexpectedEnd,
    /// A byte that cannot appear at this position.
    UnexpectedByte(u8),
    /// A number with a leading zero, or negative zero.
    NonCanonicalNumber,
    /// A number without any digits.
    MissingDigits,
    /// A number that does not fit the target type.
    NumberOverflow,
    /// Bytes remain after the root value.
    TrailingData,
    /// Values are nested deeper than the decoder allows.
    MaxDepthExceeded,
    /// A dictionary key that already appeared in the same dictionary.
    DuplicateKey,
}

impl Display for ErrorKind {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnexpectedEnd => f.write_str("unexpected end of input"),
            Self::UnexpectedByte(byte) => write!(f, "unexpected byte 0x{byte:02x}"),
            Self::NonCanonicalNumber => f.write_str("number with a leading zero or negative zero"),
            Self::MissingDigits => f.write_str("number without digits"),
            Self::NumberOverflow => f.write_str("number out of range"),
            Self::TrailingData => f.write_str("data after the root value"),
            Self::MaxDepthExceeded => f.write_str("maximum nesting depth exceeded"),
            Self::DuplicateKey => f.write_str("duplicate dictionary key"),
        }
    }
}

/// A decoding failure and the byte offset where it was detected.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DecodeError {
    kind: ErrorKind,
    offset: usize,
}

impl DecodeError {
    /// Returns why decoding failed.
    #[must_use]
    pub fn kind(&self) -> ErrorKind {
        self.kind
    }

    /// Returns the offset of the first byte of the offending token.
    #[must_use]
    pub fn offset(&self) -> usize {
        self.offset
    }
}

impl Display for DecodeError {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        write!(f, "{} at offset {}", self.kind, self.offset)
    }
}

impl Error for DecodeError {}

#[cfg(test)]
mod tests {
    use std::assert_matches;

    use super::*;

    fn rejected(input: &[u8]) -> (ErrorKind, usize) {
        let error = decode(input).expect_err("input must be rejected");
        (error.kind(), error.offset())
    }

    #[test]
    fn decodes_zero() {
        assert_eq!(decode(b"i0e"), Ok(Value::Int(0)));
    }

    #[test]
    fn decodes_negative_integer() {
        assert_eq!(decode(b"i-42e"), Ok(Value::Int(-42)));
    }

    #[test]
    fn decodes_integer_extremes() {
        assert_eq!(decode(b"i9223372036854775807e"), Ok(Value::Int(i64::MAX)));
        assert_eq!(decode(b"i-9223372036854775808e"), Ok(Value::Int(i64::MIN)));
    }

    #[test]
    fn rejects_negative_zero() {
        assert_eq!(rejected(b"i-0e"), (ErrorKind::NonCanonicalNumber, 1));
    }

    #[test]
    fn rejects_integer_leading_zero() {
        assert_eq!(rejected(b"i03e"), (ErrorKind::NonCanonicalNumber, 1));
        assert_eq!(rejected(b"i-03e"), (ErrorKind::NonCanonicalNumber, 1));
    }

    #[test]
    fn rejects_integer_without_digits() {
        assert_eq!(rejected(b"ie"), (ErrorKind::MissingDigits, 1));
        assert_eq!(rejected(b"i-e"), (ErrorKind::MissingDigits, 2));
    }

    #[test]
    fn rejects_fractional_integer() {
        assert_eq!(rejected(b"i1.5e"), (ErrorKind::UnexpectedByte(b'.'), 2));
    }

    #[test]
    fn rejects_integer_overflow() {
        assert_eq!(
            rejected(b"i9223372036854775808e"),
            (ErrorKind::NumberOverflow, 1)
        );
        assert_eq!(
            rejected(b"i-9223372036854775809e"),
            (ErrorKind::NumberOverflow, 1)
        );
    }

    #[test]
    fn rejects_unterminated_integer() {
        assert_eq!(rejected(b"i42"), (ErrorKind::UnexpectedEnd, 3));
    }

    #[test]
    fn decodes_empty_byte_string() {
        assert_eq!(decode(b"0:"), Ok(Value::Bytes(b"")));
    }

    #[test]
    fn decodes_byte_string() {
        assert_eq!(decode(b"4:spam"), Ok(Value::Bytes(b"spam")));
    }

    #[test]
    fn decodes_non_utf8_byte_string() {
        assert_eq!(decode(b"2:\xff\xfe"), Ok(Value::Bytes(b"\xff\xfe")));
    }

    #[test]
    fn rejects_short_byte_string() {
        assert_eq!(rejected(b"5:spam"), (ErrorKind::UnexpectedEnd, 6));
    }

    #[test]
    fn rejects_byte_string_without_colon() {
        assert_eq!(rejected(b"4spam"), (ErrorKind::UnexpectedByte(b's'), 1));
    }

    #[test]
    fn rejects_length_leading_zero() {
        assert_eq!(rejected(b"03:abc"), (ErrorKind::NonCanonicalNumber, 0));
    }

    #[test]
    fn rejects_length_beyond_u64() {
        assert_eq!(
            rejected(b"99999999999999999999:x"),
            (ErrorKind::NumberOverflow, 0)
        );
    }

    #[test]
    fn rejects_length_beyond_input() {
        assert_eq!(
            rejected(b"18446744073709551615:x"),
            (ErrorKind::UnexpectedEnd, 22)
        );
    }

    #[test]
    fn rejects_empty_input() {
        assert_eq!(rejected(b""), (ErrorKind::UnexpectedEnd, 0));
    }

    #[test]
    fn rejects_unknown_type() {
        assert_eq!(rejected(b"x"), (ErrorKind::UnexpectedByte(b'x'), 0));
    }

    #[test]
    fn rejects_trailing_data() {
        assert_eq!(rejected(b"i1ei2e"), (ErrorKind::TrailingData, 3));
    }

    #[test]
    fn error_message_names_kind_and_offset() {
        let error = decode(b"i-0e").expect_err("negative zero must be rejected");
        assert_eq!(
            error.to_string(),
            "number with a leading zero or negative zero at offset 1"
        );
    }

    #[test]
    fn decodes_empty_list() {
        assert_eq!(decode(b"le"), Ok(Value::List(vec![])));
    }

    #[test]
    fn decodes_mixed_list() {
        assert_eq!(
            decode(b"l4:spam4:eggsi42ee"),
            Ok(Value::List(vec![
                Value::Bytes(b"spam"),
                Value::Bytes(b"eggs"),
                Value::Int(42)
            ]))
        );
        assert_eq!(
            decode(b"l4:spami42ee"),
            Ok(Value::List(vec![Value::Bytes(b"spam"), Value::Int(42)]))
        );
    }

    #[test]
    fn decodes_nested_list() {
        assert_eq!(
            // [["cat", "dog"], 10, 20, ["bird", 30]]
            decode(b"ll3:cat3:dogei10ei20el4:birdi30eee"),
            Ok(Value::List(vec![
                Value::List(vec![Value::Bytes(b"cat"), Value::Bytes(b"dog")]),
                Value::Int(10),
                Value::Int(20),
                Value::List(vec![Value::Bytes(b"bird"), Value::Int(30)])
            ]))
        );
        assert_eq!(
            decode(b"lli1eee"),
            Ok(Value::List(vec![Value::List(vec![Value::Int(1)])]))
        );
    }

    #[test]
    fn rejects_unterminated_list() {
        assert_eq!(rejected(b"l4:spam4:eggs"), (ErrorKind::UnexpectedEnd, 13));
        assert_eq!(rejected(b"l4:spam"), (ErrorKind::UnexpectedEnd, 7));
    }

    #[test]
    fn rejects_invalid_list_element() {
        assert_eq!(rejected(b"li-0ee"), (ErrorKind::NonCanonicalNumber, 2));
        assert_eq!(rejected(b"lxe"), (ErrorKind::UnexpectedByte(b'x'), 1));
    }

    fn nested_lists(depth: usize) -> Vec<u8> {
        [b"l".repeat(depth), b"e".repeat(depth)].concat()
    }

    #[test]
    fn decodes_nesting_at_limit() {
        assert_matches!(decode(&nested_lists(MAX_DEPTH)), Ok(Value::List(_)));
    }

    #[test]
    fn rejects_nesting_beyond_limit() {
        assert_eq!(
            rejected(&nested_lists(MAX_DEPTH + 1)),
            (ErrorKind::MaxDepthExceeded, MAX_DEPTH)
        );
    }

    #[test]
    fn rejects_hostile_nesting_without_stack_overflow() {
        assert_eq!(
            rejected(&b"l".repeat(1_000_000)),
            (ErrorKind::MaxDepthExceeded, MAX_DEPTH)
        );
    }

    fn dict<'a, const N: usize>(raw: &'a [u8], entries: [(&'a [u8], Value<'a>); N]) -> Value<'a> {
        Value::Dict(Dict::new(BTreeMap::from(entries), raw))
    }

    #[test]
    fn decodes_empty_dict() {
        assert_eq!(decode(b"de"), Ok(dict(b"de", [])));
    }

    #[test]
    fn decodes_dict() {
        let input = b"d3:cow3:moo4:spaml1:aee";
        assert_eq!(
            decode(input),
            Ok(dict(
                input,
                [
                    (b"cow", Value::Bytes(b"moo")),
                    (b"spam", Value::List(vec![Value::Bytes(b"a")])),
                ]
            ))
        );
    }

    #[test]
    fn decodes_nested_dict() {
        assert_eq!(
            // {"announce": "http://t",
            //  "info": {"files": [{"length": 5, "path": ["a", "b.txt"]}], "name": "dir"}}
            decode(b"d8:announce8:http://t4:infod5:filesld6:lengthi5e4:pathl1:a5:b.txteee4:name3:diree"),
            Ok(dict(
                b"d8:announce8:http://t4:infod5:filesld6:lengthi5e4:pathl1:a5:b.txteee4:name3:diree",
                [
                    (b"announce", Value::Bytes(b"http://t")),
                    (
                        b"info",
                        dict(
                            b"d5:filesld6:lengthi5e4:pathl1:a5:b.txteee4:name3:dire",
                            [
                                (
                                    b"files",
                                    Value::List(vec![dict(
                                        b"d6:lengthi5e4:pathl1:a5:b.txtee",
                                        [
                                            (b"length", Value::Int(5)),
                                            (
                                                b"path",
                                                Value::List(vec![
                                                    Value::Bytes(b"a"),
                                                    Value::Bytes(b"b.txt")
                                                ])
                                            ),
                                        ]
                                    )])
                                ),
                                (b"name", Value::Bytes(b"dir")),
                            ]
                        )
                    ),
                ]
            ))
        );
    }

    #[test]
    fn decodes_unsorted_dict_keeping_raw_order() {
        let input = b"d4:spami1e3:cowi2ee";
        assert_eq!(
            decode(input),
            Ok(dict(
                input,
                [(b"cow", Value::Int(2)), (b"spam", Value::Int(1))]
            ))
        );
    }

    #[test]
    fn rejects_duplicate_key() {
        assert_eq!(
            rejected(b"d3:cowi1e3:cowi2ee"),
            (ErrorKind::DuplicateKey, 9)
        );
    }

    #[test]
    fn rejects_non_string_key() {
        assert_eq!(rejected(b"di1ei2ee"), (ErrorKind::UnexpectedByte(b'i'), 1));
    }

    #[test]
    fn rejects_key_without_value() {
        assert_eq!(rejected(b"d3:cowe"), (ErrorKind::UnexpectedByte(b'e'), 6));
    }

    #[test]
    fn rejects_unterminated_dict() {
        assert_eq!(rejected(b"d3:cowi1e"), (ErrorKind::UnexpectedEnd, 9));
    }

    #[test]
    fn dict_counts_towards_nesting_limit() {
        let input = [
            b"l".repeat(MAX_DEPTH),
            b"de".to_vec(),
            b"e".repeat(MAX_DEPTH),
        ]
        .concat();
        assert_eq!(rejected(&input), (ErrorKind::MaxDepthExceeded, MAX_DEPTH));
    }
}
