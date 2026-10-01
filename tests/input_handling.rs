//! Reading input: files, stdin, several files, huge inputs, null and raw input.

mod common;

use common::{exec, run_args as run};

fn fixture(name: &str, contents: &str) -> String {
    common::fixture("input_handling", name, contents)
}

#[test]
fn several_files_are_read_in_the_order_given() {
    let (a, b, empty) = (
        fixture("a.json", "{\"n\":1}\n{\"n\":2}\n"),
        fixture("b.json", "{\"n\":3}"),
        fixture("empty.json", ""),
    );
    assert_eq!(run(&["-c", ".n", &a, &empty, &b], ""), "1\n2\n3\n");
    assert_eq!(run(&["-c", ".n", &b, &a], ""), "3\n1\n2\n");
}

#[test]
fn stdin_is_used_when_no_files_are_given() {
    assert_eq!(run(&["-c", ".a"], "{\"a\":1}\n{\"a\":2}"), "1\n2\n");
}

#[test]
fn a_missing_file_is_an_error_with_a_clear_message() {
    let out = exec(&[".", "/no/such/file.json"], "");
    assert_eq!(out.code, 2);
    assert!(out.stderr.contains("No such file"), "{}", out.stderr);
}

#[test]
fn values_can_be_separated_by_any_whitespace_or_nothing() {
    assert_eq!(
        run(&["-c", "."], "1 2\n3\t\"a\"[4]{\"b\":5}"),
        "1\n2\n3\n\"a\"\n[4]\n{\"b\":5}\n"
    );
}

#[test]
fn slurp_gathers_every_value_into_one_array() {
    assert_eq!(run(&["-s", "-c", "."], "1 2\n3"), "[1,2,3]\n");
    assert_eq!(run(&["-s", "-c", "."], ""), "[]\n");
    let (a, b) = (fixture("x.json", "[1,2]"), fixture("y.json", "[3]"));
    assert_eq!(run(&["-s", "-c", "add", &a, &b], ""), "[1,2,3]\n");
}

#[test]
fn null_input_ignores_the_input() {
    assert_eq!(run(&["-n", "-c", "[1, 2]"], "ignored"), "[1,2]\n");
    assert_eq!(run(&["-n", "1 + 1"], ""), "2\n");
}

#[test]
fn input_and_inputs_read_ahead() {
    assert_eq!(run(&["-n", "-c", "[inputs]"], "1 2 3"), "[1,2,3]\n");
    assert_eq!(run(&["-n", "-c", "[input, input]"], "1 2 3"), "[1,2]\n");
    assert_eq!(run(&["-c", "[., input]"], "1 2 3 4"), "[1,2]\n[3,4]\n");
    assert_eq!(
        run(&["-n", "reduce inputs as $n (0; . + $n)"], "1 2 3"),
        "6\n"
    );
    assert_eq!(
        run(&["-n", "first(inputs | select(. > 1))"], "1 2 3"),
        "2\n"
    );
}

#[test]
fn input_filename_and_line_number() {
    let (a, b) = (fixture("first.json", "1"), fixture("second.json", "2"));
    assert_eq!(
        run(
            &["-c", "[(input_filename | split(\"/\")[-1]), .]", &a, &b],
            ""
        ),
        "[\"first.json\",1]\n[\"second.json\",2]\n"
    );
    assert_eq!(run(&["-c", "input_line_number"], "1\n2\n3\n"), "1\n2\n3\n");
    assert_eq!(run(&["-n", "input_filename"], ""), "null\n");
}

#[test]
fn a_big_file_keeps_its_order_across_all_workers() {
    let body: String = (0..120_000)
        .map(|n| format!("{{\"n\":{n},\"pad\":\"{}\"}}\n", "x".repeat(24)))
        .collect();
    let path = fixture("big.ndjson", &body);
    let expected: String = (0..120_000).map(|n| format!("{n}\n")).collect();
    assert_eq!(run(&["-c", ".n", &path], ""), expected);
    assert_eq!(
        run(&["-c", ".n"], &body),
        expected,
        "the same input on stdin"
    );
}

#[test]
fn records_larger_than_a_chunk_stay_whole() {
    let huge = "y".repeat(300_000);
    let input = format!(
        "{{\"id\":1,\"s\":\"{huge}\"}}\n{{\"id\":2,\"s\":\"z\"}}\n{{\"id\":3,\"s\":\"{huge}\"}}\n"
    );
    assert_eq!(
        run(&["-c", "[.id, (.s | length)]"], &input),
        "[1,300000]\n[2,1]\n[3,300000]\n"
    );
}

#[test]
fn pretty_printed_documents_and_ndjson_can_be_mixed() {
    assert_eq!(
        run(
            &["-c", ".a"],
            "{\"a\":1}\n{\n  \"a\": 2,\n  \"b\": [\n    3\n  ]\n}\n{\"a\":4}\n"
        ),
        "1\n2\n4\n"
    );
}

#[test]
fn skipped_fields_may_hold_tricky_strings() {
    let input = r#"{"junk":"}\"{[\\","list":[1,{"x":"]"}],"a":{"b":"ok"}}"#;
    assert_eq!(run(&["-r", ".a.b"], input), "ok\n");
}

#[test]
fn raw_input_reads_lines_as_strings() {
    assert_eq!(
        run(&["-R", "-c", "."], "a\n\nb\r\nc"),
        "\"a\"\n\"\"\n\"b\\r\"\n\"c\"\n"
    );
    assert_eq!(
        run(
            &["-R", "-r", "select(startswith(\"ERR\"))"],
            "ok\nERR one\nok\nERR two\n"
        ),
        "ERR one\nERR two\n"
    );
    assert_eq!(
        run(&["-R", "-s", "-c", "split(\"\\n\")"], "a\nb\n"),
        "[\"a\",\"b\",\"\"]\n"
    );
    assert_eq!(
        run(&["-R", "-n", "-c", "[inputs]"], "a\nb\n"),
        "[\"a\",\"b\"]\n"
    );
}

#[test]
fn input_that_is_not_a_regular_file_works() {
    assert_eq!(run(&["-c", ".", "/dev/null"], ""), "");
}

#[test]
fn errors_name_the_input_and_the_number_of_lines_read() {
    let out = exec(&[".a"], "{\"a\":1}\n[2]\n{\"a\":3}\n");
    assert!(
        out.stderr.contains("(at <stdin>:2): Cannot index array"),
        "{}",
        out.stderr
    );

    let path = fixture("located.json", "{\"a\":1}\n{\"a\":2}\n[3]");
    let out = run_failing(&[".a", &path]);
    assert!(
        out.contains(":2): Cannot index array"),
        "no trailing newline: {out}"
    );
}

fn run_failing(args: &[&str]) -> String {
    exec(args, "").stderr
}
