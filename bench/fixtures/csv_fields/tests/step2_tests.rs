use csv_fields::{encode_record, parse_record};

#[test]
fn encode_plain_fields_needs_no_quotes() {
    let fields = vec!["a".to_string(), "b".to_string()];
    assert_eq!(encode_record(&fields), "a,b");
}

#[test]
fn encode_quotes_special_fields() {
    let fields = vec!["a,b".to_string(), "say \"hi\"".to_string()];
    assert_eq!(encode_record(&fields), "\"a,b\",\"say \"\"hi\"\"\"");
}

#[test]
fn roundtrip_through_parse() {
    let cases: Vec<Vec<String>> = vec![
        vec!["a".into(), "b".into()],
        vec!["a,b".into(), "".into(), " x ".into()],
        vec!["\"quoted\"".into(), ",".into()],
    ];
    for fields in cases {
        let encoded = encode_record(&fields);
        let parsed = parse_record(&encoded)
            .unwrap_or_else(|e| panic!("roundtrip parse failed for {encoded:?}: {e}"));
        assert_eq!(parsed, fields, "roundtrip failed for {encoded:?}");
    }
}
