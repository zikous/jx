//! Passing data into a program: `--arg`, `--argjson`, files, positional values.

mod common;

use common::{exec, run_args as run};

fn fixture(name: &str, contents: &str) -> String {
    common::fixture("arguments", name, contents)
}

#[test]
fn arg_binds_a_string_variable() {
    assert_eq!(
        run(
            &["-c", "--arg", "who", "world", "\"hello \\($who)\""],
            "null"
        ),
        "\"hello world\"\n"
    );
    let tricky = r#"a "quoted" $name with 'apostrophes'"#;
    assert_eq!(
        run(&["-n", "-r", "--arg", "v", tricky, "$v"], ""),
        format!("{tricky}\n")
    );
}

#[test]
fn argjson_binds_any_json_value() {
    assert_eq!(
        run(
            &[
                "-c",
                "--argjson",
                "p",
                r#"{"min":1,"max":3}"#,
                "[.x, $p.max]"
            ],
            r#"{"x":8}"#
        ),
        "[8,3]\n"
    );
    assert_eq!(
        run(&["-n", "-c", "--argjson", "n", "1.50", "$n"], ""),
        "1.50\n"
    );
    assert_ne!(exec(&["-n", "--argjson", "n", "{oops", "$n"], "").code, 0);
}

#[test]
fn named_arguments_are_collected_in_dollar_args() {
    assert_eq!(
        run(
            &[
                "-n",
                "-c",
                "--arg",
                "a",
                "1",
                "--argjson",
                "b",
                "2",
                "$ARGS.named"
            ],
            ""
        ),
        "{\"a\":\"1\",\"b\":2}\n"
    );
}

#[test]
fn positional_arguments_after_args_and_jsonargs() {
    assert_eq!(
        run(
            &["-n", "-c", "$ARGS.positional", "--args", "one", "two words"],
            ""
        ),
        "[\"one\",\"two words\"]\n"
    );
    assert_eq!(
        run(
            &[
                "-n",
                "-c",
                "$ARGS.positional",
                "--jsonargs",
                "1",
                "{\"a\":[2]}",
                "null"
            ],
            ""
        ),
        "[1,{\"a\":[2]},null]\n"
    );
}

#[test]
fn slurpfile_and_rawfile_load_files_into_variables() {
    let data = fixture("data.json", "{\"x\":1}\n{\"x\":2}\n");
    let text = fixture("note.txt", "line one\nline two\n");
    assert_eq!(
        run(&["-n", "-c", "--slurpfile", "d", &data, "$d | map(.x)"], ""),
        "[1,2]\n"
    );
    assert_eq!(
        run(&["-n", "-c", "--rawfile", "t", &text, "$t"], ""),
        "\"line one\\nline two\\n\"\n"
    );
}

#[test]
fn the_environment_is_available() {
    assert_eq!(run(&["-n", "-r", "$ENV.PATH | type"], ""), "string\n");
    assert_eq!(run(&["-n", "env | has(\"PATH\")"], ""), "true\n");
}

#[test]
fn undefined_variables_are_compile_errors() {
    let out = exec(&["-n", "$missing"], "");
    assert_eq!(out.code, 3);
    assert!(
        out.stderr.contains("$missing is not defined"),
        "{}",
        out.stderr
    );
}

#[test]
fn a_program_can_come_from_a_file() {
    let program = fixture("prog.jx", "# add one to .a\n.a + 1\n");
    assert_eq!(run(&["-f", &program], "{\"a\":41}"), "42\n");
}
