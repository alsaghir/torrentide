use std::collections::BTreeMap;

/// A decoded bencode value that borrows its byte strings from the input.
#[derive(Debug, PartialEq, Eq)]
pub enum Value<'src> {
    /// An integer, e.g. `i42e`.
    Int(i64),
    /// A byte string, e.g. `4:spam`; not necessarily UTF-8.
    Bytes(&'src [u8]),
    /// A list of bencode values, e.g. `l4:spam4:eggse`.
    List(Vec<Value<'src>>),
    /// A dictionary with byte-string keys, e.g. `d3:cow3:mooe`.
    Dict(Dict<'src>),
}

/// A decoded dictionary that also keeps its exact encoded bytes.
///
/// Equality compares both the entries and the encoded bytes.
#[derive(Debug, PartialEq, Eq)]
pub struct Dict<'src> {
    entries: BTreeMap<&'src [u8], Value<'src>>,
    raw: &'src [u8],
}

impl<'src> Dict<'src> {
    pub(crate) fn new(entries: BTreeMap<&'src [u8], Value<'src>>, raw: &'src [u8]) -> Self {
        Self { entries, raw }
    }

    /// Returns the entries in key order, regardless of their order in the input.
    #[must_use]
    pub fn entries(&self) -> &BTreeMap<&'src [u8], Value<'src>> {
        &self.entries
    }

    /// Returns the dictionary's bytes exactly as they appeared in the input, from `d` to `e`.
    ///
    /// Hash these rather than a re-encoding: unsorted input would re-encode differently, and
    /// the info-hash must match the bytes everyone else hashed.
    #[must_use]
    pub fn raw(&self) -> &'src [u8] {
        self.raw
    }
}
