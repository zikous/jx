//! Regular expressions: test, match, capture, scan, split, sub.

mod common;

use common::{exec, run};

#[test]
fn test_reports_whether_a_pattern_matches() {
    assert_eq!(
        run("[test(\"b+\"), test(\"^b\"), test(\"[0-9]\")]", "\"abbc\""),
        "[true,false,false]\n"
    );
    assert_eq!(
        run("[test(\"ABC\"), test(\"ABC\"; \"i\")]", "\"abc\""),
        "[false,true]\n"
    );
    assert_eq!(run("test(\"a b\"; \"x\")", "\"ab\""), "true\n");
}

#[test]
fn match_describes_each_match_with_offsets_in_characters() {
    assert_eq!(
        run("match(\"é+\") | [.offset, .length, .string]", "\"aééb\""),
        "[1,2,\"éé\"]\n"
    );
    assert_eq!(
        run("[match(\"a\"; \"g\") | .offset]", "\"banana\""),
        "[1,3,5]\n"
    );
    assert_eq!(
        run("match(\"(a)(x)?\") | .captures | map(.string)", "\"abc\""),
        "[\"a\",null]\n"
    );
}

#[test]
fn capture_names_groups() {
    assert_eq!(
        run(
            "capture(\"(?<year>\\\\d{4})-(?<month>\\\\d{2})\")",
            "\"on 2024-05-17\""
        ),
        "{\"year\":\"2024\",\"month\":\"05\"}\n"
    );
    assert_eq!(
        run(
            "[capture(\"(?<n>\\\\d+)\"; \"g\") | .n | tonumber] | add",
            "\"1 22 333\""
        ),
        "356\n"
    );
}

#[test]
fn scan_collects_all_matches() {
    assert_eq!(
        run("[scan(\"[a-z]+\")]", "\"ab 12 cd\""),
        "[\"ab\",\"cd\"]\n"
    );
    assert_eq!(
        run("[scan(\"(\\\\d)(\\\\w)\")]", "\"1a 2b\""),
        "[[\"1\",\"a\"],[\"2\",\"b\"]]\n"
    );
}

#[test]
fn split_and_splits_on_patterns() {
    assert_eq!(
        run("[splits(\", *\")]", "\"a, b,c\""),
        "[\"a\",\"b\",\"c\"]\n"
    );
    assert_eq!(
        run("split(\"\\\\s+\"; null)", "\"a  b\\tc\""),
        "[\"a\",\"b\",\"c\"]\n"
    );
}

#[test]
fn sub_and_gsub_replace_with_expressions() {
    assert_eq!(run("sub(\"b\"; \"X\")", "\"abab\""), "\"aXab\"\n");
    assert_eq!(run("gsub(\"b\"; \"X\")", "\"abab\""), "\"aXaX\"\n");
    assert_eq!(
        run("gsub(\"(?<d>\\\\d)\"; \"<\\(.d)>\")", "\"a1b22\""),
        "\"a<1>b<2><2>\"\n"
    );
    assert_eq!(
        run(
            "gsub(\"(?<w>\\\\w+)\"; .w | ascii_upcase)",
            "\"hello world\""
        ),
        "\"HELLO WORLD\"\n"
    );
    assert_eq!(run("gsub(\"\\\\s\"; \"\")", "\" a b \""), "\"ab\"\n");
    assert_eq!(run("gsub(\"A\"; \"x\"; \"i\")", "\"aAa\""), "\"xxx\"\n");
}

#[test]
fn zero_width_matches_are_handled() {
    assert_eq!(run("gsub(\"\"; \"-\")", "\"abc\""), "\"-a-b-c-\"\n");
    assert_eq!(run("[match(\"\"; \"g\")] | length", "\"abc\""), "4\n");
}

#[test]
fn invalid_patterns_and_flags_are_errors() {
    assert_eq!(exec(&["test(\"(\")"], "\"a\"").code, 5);
    assert_eq!(exec(&["test(\"a\"; \"q\")"], "\"a\"").code, 5);
}
