use base32::{decode, encode};

#[test]
fn decode_rfc4648_vectors() {
    assert_eq!(decode("").unwrap(), b"");
    assert_eq!(decode("MY======").unwrap(), b"f");
    assert_eq!(decode("MZXQ====").unwrap(), b"fo");
    assert_eq!(decode("MZXW6===").unwrap(), b"foo");
    assert_eq!(decode("MZXW6YQ=").unwrap(), b"foob");
    assert_eq!(decode("MZXW6YTB").unwrap(), b"fooba");
    assert_eq!(decode("MZXW6YTBOI======").unwrap(), b"foobar");
}

#[test]
fn decode_accepts_lowercase() {
    assert_eq!(decode("mzxw6ytb").unwrap(), b"fooba");
}

#[test]
fn decode_rejects_bad_characters_and_padding() {
    assert!(decode("MZXW6YT*").is_err(), "invalid character");
    assert!(decode("MZXW6YTB==").is_err(), "padding in wrong position");
    assert!(decode("MZXW6Y").is_err(), "group length not multiple of 8");
    assert!(decode("MZXW6YTBOI=====").is_err(), "wrong pad count");
}

#[test]
fn roundtrip_random_lengths() {
    // deterministic pseudo-random bytes
    let mut x: u32 = 0x12345678;
    let mut next = || {
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        (x & 0xff) as u8
    };
    for n in 0..40usize {
        let data: Vec<u8> = (0..n).map(|_| next()).collect();
        let text = encode(&data);
        assert_eq!(decode(&text).unwrap(), data, "roundtrip len {n}");
    }
}
