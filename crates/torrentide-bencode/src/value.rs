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
    ///
    /// Iterates in key order regardless of the order in the input.
    Dict(BTreeMap<&'src [u8], Value<'src>>),
}
