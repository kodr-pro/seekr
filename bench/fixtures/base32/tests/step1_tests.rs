use base32::encode;

#[test]
fn rfc4648_test_vectors() {
    assert_eq!(encode(b""), "");
    assert_eq!(encode(b"f"), "MY======");
    assert_eq!(encode(b"fo"), "MZXQ====");
    assert_eq!(encode(b"foo"), "MZXW6===");
    assert_eq!(encode(b"foob"), "MZXW6YQ=");
    assert_eq!(encode(b"fooba"), "MZXW6YTB");
    assert_eq!(encode(b"foobar"), "MZXW6YTBOI======");
}

#[test]
fn output_length_is_multiple_of_eight() {
    for n in 0..=10usize {
        let data = vec![0u8; n];
        assert_eq!(encode(&data).len() % 8, 0, "len {n}");
    }
}
