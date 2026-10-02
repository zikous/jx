//! Functions, recursion, loops and early exits.

mod common;

use common::run;

#[test]
fn functions_take_filters_as_arguments() {
    assert_eq!(run("def twice(f): f | f; 3 | twice(. * 2)", "null"), "12\n");
    assert_eq!(
        run(
            "def addvalue(f): f as $x | map(. + $x); addvalue(.[0])",
            "[[1,2],[10,20]]"
        ),
        "[[1,2,1,2],[10,20,1,2]]\n"
    );
    assert_eq!(
        run("def apply(f; g): [f, g]; apply(1, 2; 3)", "null"),
        "[1,2,3]\n"
    );
}

#[test]
fn value_parameters_are_evaluated_once_per_output() {
    assert_eq!(
        run(
            "def add2($a; $b): $a + $b; add2(1; 2), add2(1, 2; 10, 20)",
            "null"
        ),
        "3\n11\n21\n12\n22\n"
    );
}

#[test]
fn functions_can_be_recursive_and_nested() {
    assert_eq!(
        run(
            "def fac: if . <= 1 then 1 else . * (. - 1 | fac) end; map(fac)",
            "[1,5,10]"
        ),
        "[1,120,3628800]\n"
    );
    assert_eq!(
        run(
            "def fib: if . < 2 then . else (. - 1 | fib) + (. - 2 | fib) end; [range(10) | fib]",
            "null"
        ),
        "[0,1,1,2,3,5,8,13,21,34]\n"
    );
    assert_eq!(
        run(
            "def fib: if . < 2 then . else (. - 1 | fib) + (. - 2 | fib) end; 25 | fib",
            "null"
        ),
        "75025\n"
    );
    assert_eq!(
        run("def outer: def inner: 5; inner * 2; outer", "null"),
        "10\n"
    );
    assert_eq!(
        run(
            "def depth: if type == \"array\" then 1 + (map(depth) | max // 0) else 0 end; depth",
            "[1,[2,[3]],[]]"
        ),
        "3\n"
    );
}

#[test]
fn recursion_handles_very_deep_calls() {
    assert_eq!(
        run(
            "def down: if . == 0 then . else . - 1 | down end; 100000 | down",
            "null"
        ),
        "0\n"
    );
}

#[test]
fn recursion_limit_reports_a_clean_runtime_error() {
    let out = common::exec(
        &[
            "--recursion-limit",
            "50",
            "-n",
            "def down: if . == 0 then . else . - 1 | down end; 100 | down",
        ],
        "",
    );
    assert_eq!(out.code, 5);
    assert!(
        out.stderr.contains("recursion limit exceeded"),
        "{}",
        out.stderr
    );
}

#[test]
fn recurse_walks_structures_and_unfolds_sequences() {
    assert_eq!(
        run("[recurse(if . < 100 then . * 2 else empty end)]", "3"),
        "[3,6,12,24,48,96,192]\n"
    );
    assert_eq!(
        run(
            "[recurse(.children[]?) | .name]",
            r#"{"name":"a","children":[{"name":"b","children":[{"name":"c"}]},{"name":"d"}]}"#
        ),
        "[\"a\",\"b\",\"c\",\"d\"]\n"
    );
    assert_eq!(run("[recurse(. * 2; . < 50)]", "3"), "[3,6,12,24,48]\n");
}

#[test]
fn while_until_and_repeat_generate_values() {
    assert_eq!(run("[while(. < 40; . * 3)]", "1"), "[1,3,9,27]\n");
    assert_eq!(run("until(. > 100; . * 2)", "3"), "192\n");
    assert_eq!(
        run("[limit(3; repeat(\"x\"))]", "null"),
        "[\"x\",\"x\",\"x\"]\n"
    );
}

#[test]
fn range_has_three_forms() {
    assert_eq!(run("[range(4)]", "null"), "[0,1,2,3]\n");
    assert_eq!(run("[range(2; 5)]", "null"), "[2,3,4]\n");
    assert_eq!(run("[range(10; 0; -3)]", "null"), "[10,7,4,1]\n");
    assert_eq!(run("[range(0; 1; 0.25)]", "null"), "[0,0.25,0.5,0.75]\n");
    assert_eq!(
        run("[range(0, 1; 3, 4)]", "null"),
        "[0,1,2,0,1,2,3,1,2,1,2,3]\n"
    );
}

#[test]
fn label_and_break_leave_nested_loops() {
    assert_eq!(
        run(
            "[label $out | range(10) | if . > 3 then break $out else . end]",
            "null"
        ),
        "[0,1,2,3]\n"
    );
    assert_eq!(
        run(
            "[.[] | label $skip | if . == 2 then break $skip else . end]",
            "[1,2,3]"
        ),
        "[1,3]\n"
    );
}

#[test]
fn first_last_nth_and_until() {
    assert_eq!(run("[first, last, nth(1)]", "[5,6,7]"), "[5,7,6]\n");
    assert_eq!(run("nth(2; range(10, 20))", "null"), "2\n");
    assert_eq!(run("last(range(5))", "null"), "4\n");
    assert_eq!(run("[skip(2; range(5))]", "null"), "[2,3,4]\n");
}

#[test]
fn isempty_any_and_all_short_circuit() {
    assert_eq!(
        run("isempty(empty), isempty(1, error(\"never\"))", "null"),
        "true\nfalse\n"
    );
    assert_eq!(run("any(range(1000000000); . > 3)", "null"), "true\n");
    assert_eq!(
        run("all(1, 2, error(\"x\"); . > 1)", "null"),
        "false
"
    );
}

#[test]
fn conditionals_can_omit_else_and_chain_elif() {
    assert_eq!(
        run("map(if . > 1 then \"big\" end)", "[1,2]"),
        "[1,\"big\"]\n"
    );
    assert_eq!(
        run(
            "[.[] | if . == 1 then \"one\" elif . == 2 then \"two\" else \"many\" end]",
            "[1,2,3]"
        ),
        "[\"one\",\"two\",\"many\"]\n"
    );
}

#[test]
fn generators_combine_by_cartesian_product() {
    assert_eq!(run("[(1, 2) + (10, 20)]", "null"), "[11,12,21,22]\n");
    assert_eq!(run("[(1, 2) * (3, 4)]", "null"), "[3,6,4,8]\n");
    assert_eq!(
        run("\"\\(1, 2)-\\(3, 4)\"", "null"),
        "\"1-3\"\n\"2-3\"\n\"1-4\"\n\"2-4\"\n"
    );
}

#[test]
fn walk_applies_a_function_everywhere() {
    assert_eq!(
        run(
            "walk(if type == \"number\" then . + 1 else . end)",
            "[1,[2,{\"a\":3}]]"
        ),
        "[2,[3,{\"a\":4}]]\n"
    );
    assert_eq!(
        run(
            "walk(if type == \"array\" then sort else . end)",
            "[3,[2,1]]"
        ),
        "[3,[1,2]]\n"
    );
}
