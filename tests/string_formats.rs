//! The `@format` strings for exporting and escaping values.

mod common;

use common::{exec, run_args};

fn run(program: &str, input: &str) -> String {
    run_args(&["-r", program], input)
}

#[test]
fn csv_quotes_strings_and_leaves_numbers_bare() {
    assert_eq!(
        run("@csv", r#"[1,"a,b","say \"hi\"",null,true]"#),
        "1,\"a,b\",\"say \"\"hi\"\"\",,true\n"
    );
    assert_eq!(
        run(".[] | @csv", r#"[[1,2],["x","y"]]"#),
        "1,2\n\"x\",\"y\"\n"
    );
}

#[test]
fn tsv_escapes_tabs_and_newlines() {
    assert_eq!(
        run("@tsv", "[\"a\\tb\",\"c\\nd\",\"e\\\\f\",1]"),
        "a\\tb\tc\\nd\te\\\\f\t1\n"
    );
}

#[test]
fn json_and_text() {
    assert_eq!(run("@json", r#"{"a":[1,"x"]}"#), "{\"a\":[1,\"x\"]}\n");
    assert_eq!(run("@text", "[1,2]"), "[1,2]\n");
    assert_eq!(run("@text", "\"plain\""), "plain\n");
}

#[test]
fn html_escapes_markup() {
    assert_eq!(
        run("@html", r#""<a href=\"x\">Tom & 'Jerry'</a>""#),
        "&lt;a href=&quot;x&quot;&gt;Tom &amp; &apos;Jerry&apos;&lt;/a&gt;\n"
    );
}

#[test]
fn uri_percent_encodes_everything_but_unreserved_characters() {
    assert_eq!(
        run("@uri", "\"a b&c=d/é~-_.\""),
        "a%20b%26c%3Dd%2F%C3%A9~-_.\n"
    );
    assert_eq!(run("@urid", "\"a%20b%C3%A9\""), "a bé\n");
}

#[test]
fn sh_quotes_for_the_shell() {
    assert_eq!(run("@sh", "\"it's\""), "'it'\\''s'\n");
    assert_eq!(run("@sh", r#"["a b",1,null,"c"]"#), "'a b' 1 null 'c'\n");
    assert_eq!(
        exec(&["@sh"], r#"{"a":1}"#).code,
        5,
        "objects cannot be quoted"
    );
}

#[test]
fn base64_and_base32() {
    assert_eq!(run("@base64", "\"hello\""), "aGVsbG8=\n");
    assert_eq!(run("@base64d", "\"aGVsbG8=\""), "hello\n");
    assert_eq!(
        run("@base64d", "\"aGVsbG8\""),
        "hello\n",
        "padding is optional when decoding"
    );
    assert_eq!(run("@base32", "\"hello\""), "NBSWY3DP\n");
    assert_eq!(run("@base32d", "\"NBSWY3DP\""), "hello\n");
    assert_eq!(run("@base64 | @base64d", "\"héllo 😀\""), "héllo 😀\n");
}

#[test]
fn a_format_can_prefix_an_interpolated_string() {
    assert_eq!(
        run(
            "@uri \"https://x.org/?q=\\(.q)&n=\\(.n)\"",
            r#"{"q":"a b&c","n":2}"#
        ),
        "https://x.org/?q=a%20b%26c&n=2\n"
    );
    assert_eq!(
        run("@sh \"echo \\(.msg)\"", r#"{"msg":"hi there"}"#),
        "echo 'hi there'\n"
    );
    assert_eq!(run("@json \"v=\\(.)\"", "[1,\"a\"]"), "v=[1,\"a\"]\n");
    assert_eq!(run("@base64 \"\\(.)!\"", "\"hi\""), "aGk=!\n");
}

#[test]
fn unknown_formats_are_reported() {
    assert_eq!(exec(&["@nope"], "1").code, 5);
}
