use slugify::slugify;

#[test]
fn lowercases_and_dashifies() {
    assert_eq!(slugify("Hello World"), "hello-world");
}

#[test]
fn collapses_runs_of_junk() {
    assert_eq!(slugify("  Hello --  World!! "), "hello-world");
}

#[test]
fn no_leading_or_trailing_dashes() {
    assert_eq!(slugify("--hello world--"), "hello-world");
}

#[test]
fn digits_pass_through() {
    assert_eq!(slugify("Version 2.0.1"), "version-2-0-1");
}

#[test]
fn blank_is_empty() {
    assert_eq!(slugify("   "), "");
    assert_eq!(slugify(""), "");
}
