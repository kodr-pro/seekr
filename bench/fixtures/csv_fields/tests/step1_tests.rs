use csv_fields::parse_record;

#[test]
fn plain_fields_split_on_commas() {
    assert_eq!(
        parse_record("a,b,c").unwrap(),
        vec!["a".to_string(), "b".to_string(), "c".to_string()]
    );
}

#[test]
fn empty_line_is_one_empty_field() {
    assert_eq!(parse_record("").unwrap(), vec![String::new()]);
}

#[test]
fn trailing_comma_yields_trailing_empty_field() {
    assert_eq!(
        parse_record("a,").unwrap(),
        vec!["a".to_string(), String::new()]
    );
}

#[test]
fn quoted_field_may_contain_commas_and_quotes() {
    assert_eq!(
        parse_record("\"a,b\",\"say \"\"hi\"\"\"").unwrap(),
        vec!["a,b".to_string(), "say \"hi\"".to_string()]
    );
}

#[test]
fn unterminated_quote_is_an_error() {
    assert!(parse_record("\"abc").is_err());
}

#[test]
fn whitespace_is_verbatim_not_trimmed() {
    assert_eq!(
        parse_record(" a ,b ").unwrap(),
        vec![" a ".to_string(), "b ".to_string()]
    );
}
