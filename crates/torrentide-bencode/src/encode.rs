use crate::Value;

/// Encode a `Value` into its Bencode representation.
///
/// The output is canonical: dictionary keys are written in sorted order
/// (the order of [`Dict::entries`](crate::Dict::entries)), and
/// [`Dict::raw`](crate::Dict::raw) is not used.
///
/// # Examples
///
/// ```
/// use torrentide_bencode::Value;
/// use torrentide_bencode::encode;
///
/// let value = Value::Int(42);
/// let encoded = encode(&value);
/// assert_eq!(encoded, b"i42e");
/// ```
#[must_use]
pub fn encode(value: &Value<'_>) -> Vec<u8> {
    let mut buf = Vec::new();
    encode_value(value, &mut buf);
    buf
}

fn encode_value(value: &Value<'_>, buf: &mut Vec<u8>) {
    match value {
        Value::Int(i) => {
            buf.push(b'i');
            buf.extend_from_slice(i.to_string().as_bytes());
            buf.push(b'e');
        }
        Value::Bytes(bytes) => {
            encode_bytes(bytes, buf);
        }
        Value::List(list) => {
            buf.push(b'l');
            for item in list {
                encode_value(item, buf);
            }
            buf.push(b'e');
        }
        Value::Dict(dict) => {
            buf.push(b'd');
            for (key, value) in dict.entries() {
                encode_bytes(key, buf);
                encode_value(value, buf);
            }
            buf.push(b'e');
        }
    }
}

fn encode_bytes(bytes: &[u8], buf: &mut Vec<u8>) {
    buf.extend_from_slice(bytes.len().to_string().as_bytes());
    buf.push(b':');
    buf.extend_from_slice(bytes);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::decode;

    /// Canonical input must come back byte for byte.
    fn assert_round_trip(input: &[u8]) {
        let value = decode(input).expect("test input must be valid bencode");
        assert_eq!(encode(&value), input);
    }

    #[test]
    fn encodes_values_built_directly() {
        let value = Value::List(vec![Value::Int(-1), Value::Bytes(b"x")]);
        assert_eq!(encode(&value), b"li-1e1:xe");
    }

    #[test]
    fn round_trips_integers() {
        assert_round_trip(b"i0e");
        assert_round_trip(b"i42e");
        assert_round_trip(b"i-7e");
        assert_round_trip(b"i9223372036854775807e"); // i64::MAX
        assert_round_trip(b"i-9223372036854775808e"); // i64::MIN
    }

    #[test]
    fn round_trips_byte_strings() {
        assert_round_trip(b"0:");
        assert_round_trip(b"4:spam");
        // Not UTF-8: the length counts bytes, not characters.
        assert_round_trip(b"3:\xff\x00\x01");
    }

    #[test]
    fn round_trips_lists() {
        assert_round_trip(b"le");
        assert_round_trip(b"l4:spami42ee");
        assert_round_trip(b"lli1eee");
    }

    #[test]
    fn round_trips_dicts() {
        assert_round_trip(b"de");
        // {
        //   "announce": "http://tracker",
        //   "info": {
        //     "length": 1024,
        //     "name": "file.txt",
        //     "piece length": 16384,
        //     "pieces": ""
        //   }
        // }
        assert_round_trip(
            b"d8:announce14:http://tracker4:infod6:lengthi1024e4:name8:file.txt\
              12:piece lengthi16384e6:pieces0:ee",
        );
    }

    #[test]
    fn sorts_unsorted_dict_keys() {
        // {"b": 2, "a": 1} -> {"a": 1, "b": 2}
        let value = decode(b"d1:bi2e1:ai1ee").expect("unsorted keys are accepted");
        assert_eq!(encode(&value), b"d1:ai1e1:bi2ee");
    }

    #[test]
    fn sorts_nested_unsorted_dict_keys() {
        // [{"z": [], "m": {"y": 0, "x": 1}}] -> [{"m": {"x": 1, "y": 0}, "z": []}]
        let value = decode(b"ld1:zle1:md1:yi0e1:xi1eeee").expect("unsorted keys are accepted");
        assert_eq!(encode(&value), b"ld1:md1:xi1e1:yi0ee1:zleee");
    }
}
