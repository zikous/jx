//! String functions.

mod common;

use common::run;

#[test]
fn length_counts_characters_and_bytes() {
    assert_eq!(run("[length, utf8bytelength]", "\"héllo 😀\""), "[7,11]\n");
}

#[test]
fn split_and_join() {
    assert_eq!(run("split(\", \")", "\"a, b, c\""), "[\"a\",\"b\",\"c\"]\n");
    assert_eq!(run("split(\"\")", "\"abc\""), "[\"a\",\"b\",\"c\"]\n");
    assert_eq!(run("join(\"-\")", "[\"a\",\"b\"]"), "\"a-b\"\n");
    assert_eq!(run("join(\",\")", "[1,null,\"x\",true]"), "\"1,,x,true\"\n");
    assert_eq!(run("join(\",\")", "[]"), "\"\"\n");
}

#[test]
fn prefixes_suffixes_and_trimming() {
    assert_eq!(
        run(
            "[startswith(\"he\"), endswith(\"lo\"), ltrimstr(\"he\"), rtrimstr(\"lo\")]",
            "\"hello\""
        ),
        "[true,true,\"llo\",\"hel\"]\n"
    );
    assert_eq!(
        run("[ltrimstr(\"x\"), rtrimstr(\"x\")]", "\"hello\""),
        "[\"hello\",\"hello\"]\n"
    );
    assert_eq!(
        run("ltrimstr(\"a\")", "1"),
        "1\n",
        "non-strings pass through"
    );
}

#[test]
fn case_conversion_affects_only_ascii() {
    assert_eq!(
        run("[ascii_downcase, ascii_upcase]", "\"AbÉ\""),
        "[\"abÉ\",\"ABÉ\"]\n"
    );
}

#[test]
fn searching_inside_strings() {
    assert_eq!(
        run(
            "[contains(\"ell\"), (\"ell\" | inside(\"hello\"))]",
            "\"hello\""
        ),
        "[true,true]\n"
    );
    assert_eq!(
        run("[index(\"l\"), rindex(\"l\"), indices(\"l\")]", "\"hello\""),
        "[2,3,[2,3]]\n"
    );
    assert_eq!(run("indices(\", \")", "\"a, b, cd, efg\""), "[1,4,8]\n");
}

#[test]
fn codepoints_round_trip() {
    assert_eq!(run("explode", "\"aé😀\""), "[97,233,128512]\n");
    assert_eq!(run("implode", "[104,105,128512]"), "\"hi😀\"\n");
    assert_eq!(run("explode | reverse | implode", "\"abc\""), "\"cba\"\n");
}

#[test]
fn json_conversion() {
    assert_eq!(
        run("tojson", r#"{"a":[1,"x",null]}"#),
        "\"{\\\"a\\\":[1,\\\"x\\\",null]}\"\n"
    );
    assert_eq!(
        run("tojson | fromjson", r#"{"a":[1,"x",null]}"#),
        "{\"a\":[1,\"x\",null]}\n"
    );
    assert_eq!(run("fromjson", "\"[1, 2]\""), "[1,2]\n");
    assert_eq!(
        run("map(tostring)", r#"[1,"a",null,true,[1],{"b":2}]"#),
        "[\"1\",\"a\",\"null\",\"true\",\"[1]\",\"{\\\"b\\\":2}\"]\n"
    );
}

#[test]
fn string_multiplication_and_division() {
    assert_eq!(run("\"ab\" * 3", "null"), "\"ababab\"\n");
    assert_eq!(run("\"a,b,c\" / \",\"", "null"), "[\"a\",\"b\",\"c\"]\n");
    assert_eq!(run("\"x\" * 0", "null"), "\"\"\n");
}

#[test]
fn escapes_in_literals_and_output() {
    assert_eq!(
        run(".", r#""tab\there \"quoted\" \\ \u00e9 \ud83d\ude00""#),
        "\"tab\\there \\\"quoted\\\" \\\\ é 😀\"\n"
    );
    assert_eq!(
        run(".", r#""\u0000\u001f\u007f""#),
        "\"\\u0000\\u001f\\u007f\"\n"
    );
}

#[test]
fn sort_and_comparison_use_codepoint_order() {
    assert_eq!(
        run("sort", r#"["b","a","B","é","z"]"#),
        "[\"B\",\"a\",\"b\",\"z\",\"é\"]\n"
    );
    assert_eq!(run("max, min", r#"["b","a","c"]"#), "\"c\"\n\"a\"\n");
}

#[test]
fn case_conversion_errors_name_the_function() {
    let out = common::exec(&["ascii_downcase"], "1");
    assert!(
        out.stderr.contains("ascii_downcase input must be a string"),
        "{}",
        out.stderr
    );
}
