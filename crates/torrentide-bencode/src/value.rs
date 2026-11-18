/// A decoded bencode value that borrows its byte strings from the input.
#[derive(Debug, PartialEq, Eq)]
pub enum Value<'src> {
    /// An integer, e.g. `i42e`.
    Int(i64),
    /// A byte string, e.g. `4:spam`; not necessarily UTF-8.
    Bytes(&'src [u8]),
}
