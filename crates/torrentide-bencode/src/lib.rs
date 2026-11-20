//! Bencode decoding and encoding (BEP 3).

mod decode;
mod value;

pub use decode::{DecodeError, ErrorKind, decode};
pub use value::{Dict, Value};
