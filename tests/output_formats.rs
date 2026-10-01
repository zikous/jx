//! How values are printed: indentation, key order, escaping, colors.

mod common;

use common::{exec, run_args as run};

const DOC: &str = r#"{"b":[1,{"z":1,"a":"é😀"}],"a":"x\ty","e":{},"l":[]}"#;

#[test]
fn pretty_printing_is_the_default_with_two_spaces() {
    assert_eq!(
        run(&["."], DOC),
        "{\n  \"b\": [\n    1,\n    {\n      \"z\": 1,\n      \"a\": \"é😀\"\n    }\n  ],\n  \"a\": \"x\\ty\",\n  \"e\": {},\n  \"l\": []\n}\n"
    );
}

#[test]
fn compact_output_removes_all_whitespace() {
    assert_eq!(
        run(&["-c", "."], DOC),
        "{\"b\":[1,{\"z\":1,\"a\":\"é😀\"}],\"a\":\"x\\ty\",\"e\":{},\"l\":[]}\n"
    );
}

#[test]
fn indent_width_and_tabs() {
    assert_eq!(
        run(&["--indent", "1", ".b"], DOC),
        "[\n 1,\n {\n  \"z\": 1,\n  \"a\": \"é😀\"\n }\n]\n"
    );
    assert_eq!(
        run(&["--indent", "4", ".b[1]"], DOC),
        "{\n    \"z\": 1,\n    \"a\": \"é😀\"\n}\n"
    );
    assert_eq!(
        run(&["--tab", ".b[1]"], DOC),
        "{\n\t\"z\": 1,\n\t\"a\": \"é😀\"\n}\n"
    );
    assert_eq!(
        run(&["--indent", "0", ".b"], DOC),
        "[1,{\"z\":1,\"a\":\"é😀\"}]\n"
    );
}

#[test]
fn the_last_layout_option_wins() {
    assert_eq!(
        run(&["--tab", "-c", ".b"], DOC),
        "[1,{\"z\":1,\"a\":\"é😀\"}]\n"
    );
    assert_eq!(
        run(&["-c", "--tab", ".b[1]"], DOC),
        "{\n\t\"z\": 1,\n\t\"a\": \"é😀\"\n}\n"
    );
    assert_eq!(
        run(&["--indent", "3", "-c", ".b"], DOC),
        "[1,{\"z\":1,\"a\":\"é😀\"}]\n"
    );
    assert_eq!(
        run(&["-c", "--indent", "1", ".b[1]"], DOC),
        "{\n \"z\": 1,\n \"a\": \"é😀\"\n}\n"
    );
}

#[test]
fn key_order_is_preserved_unless_sorted() {
    assert_eq!(
        run(&["-c", "keys_unsorted", "-S"], DOC),
        "[\"b\",\"a\",\"e\",\"l\"]\n",
        "-S only affects printing"
    );
    assert_eq!(
        run(&["-cS", "."], DOC),
        "{\"a\":\"x\\ty\",\"b\":[1,{\"a\":\"é😀\",\"z\":1}],\"e\":{},\"l\":[]}\n"
    );
}

#[test]
fn duplicate_keys_keep_the_last_value_in_the_first_position() {
    assert_eq!(
        run(&["-c", "."], r#"{"a":1,"b":2,"a":3}"#),
        "{\"a\":3,\"b\":2}\n"
    );
}

#[test]
fn raw_output_prints_strings_without_quotes() {
    assert_eq!(run(&["-r", ".a"], DOC), "x\ty\n");
    assert_eq!(
        run(&["-r", ".[]"], r#"["a","b",1,null]"#),
        "a\nb\n1\nnull\n"
    );
    assert_eq!(
        run(&["-r", "."], r#"[1,"a"]"#),
        "[\n  1,\n  \"a\"\n]\n",
        "only top-level strings are raw"
    );
}

#[test]
fn join_output_and_nul_separated_output() {
    assert_eq!(run(&["-j", ".[]"], r#"["a","b",1]"#), "ab1");
    assert_eq!(run(&["--raw-output0", ".[]"], r#"["a","b"]"#), "a\0b\0");
    let refused = exec(&["--raw-output0", "."], "\"a\\u0000b\"");
    assert_eq!(refused.code, 5);
}

#[test]
fn ascii_output_escapes_everything_outside_ascii() {
    assert_eq!(
        run(&["-c", "-a", "."], "\"é😀\""),
        "\"\\u00e9\\ud83d\\ude00\"\n"
    );
    assert_eq!(run(&["-r", "-a", "."], "\"é\""), "\"\\u00e9\"\n");
}

#[test]
fn control_characters_are_escaped() {
    assert_eq!(
        run(&["-c", "."], r#""\u0000\u001f\u007f\n\r\t\b\f\"\\""#),
        "\"\\u0000\\u001f\\u007f\\n\\r\\t\\b\\f\\\"\\\\\"\n"
    );
}

#[test]
fn colors_wrap_each_value_kind() {
    let colored = run(&["-C", "-c", "."], r#"{"a":[1,"x",null,true]}"#);
    assert!(colored.starts_with("\x1b[1;39m{"), "{colored:?}");
    assert!(
        colored.contains("\x1b[0;32m\"x\"\x1b[0m"),
        "strings are green: {colored:?}"
    );
    assert!(colored.ends_with("\x1b[0m\n"));
    assert_eq!(
        run(&["-M", "-c", "."], r#"{"a":[1]}"#),
        "{\"a\":[1]}\n",
        "-M turns colors off"
    );
}

#[test]
fn every_output_is_a_separate_line() {
    assert_eq!(
        run(&["-c", ".[]"], "[1,[2],{\"a\":3}]"),
        "1\n[2]\n{\"a\":3}\n"
    );
}
