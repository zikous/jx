//! Binding names to parts of a value with `as`.

mod common;

use common::run;

#[test]
fn variables_bind_each_output() {
    assert_eq!(run(".[] as $x | $x * 2", "[1,2,3]"), "2\n4\n6\n");
    assert_eq!(
        run(". as $whole | map(. + ($whole | length))", "[10,20]"),
        "[12,22]\n"
    );
    assert_eq!(
        run("1 as $a | 2 as $b | [$a, $b, $a + $b]", "null"),
        "[1,2,3]\n"
    );
}

#[test]
fn array_patterns_pick_elements_by_position() {
    assert_eq!(
        run(". as [$a, $b] | {a: $a, b: $b}", "[1,2,3]"),
        "{\"a\":1,\"b\":2}\n"
    );
    assert_eq!(run(". as [$a, [$b]] | [$a, $b]", "[1,[2]]"), "[1,2]\n");
    assert_eq!(run(". as [$a, $b] | [$a, $b]", "[1]"), "[1,null]\n");
    assert_eq!(
        run(".[] as [$k, $v] | {($k): $v}", r#"[["a",1],["b",2]]"#),
        "{\"a\":1}\n{\"b\":2}\n"
    );
}

#[test]
fn object_patterns_pick_fields_by_name() {
    assert_eq!(
        run(
            ". as {name: $n, age: $a} | \"\\($n) is \\($a)\"",
            r#"{"name":"Ada","age":36}"#
        ),
        "\"Ada is 36\"\n"
    );
    assert_eq!(
        run(
            ". as {$name, $age} | [$name, $age]",
            r#"{"name":"Ada","age":36}"#
        ),
        "[\"Ada\",36]\n"
    );
    assert_eq!(
        run(". as {a: {b: [$x, $y]}} | $x + $y", r#"{"a":{"b":[3,4]}}"#),
        "7\n"
    );
    assert_eq!(
        run(". as {(\"k\" + \"ey\"): $v} | $v", r#"{"key":9}"#),
        "9\n"
    );
    assert_eq!(
        run(". as {$a: [$first]} | [$a, $first]", r#"{"a":[1,2]}"#),
        "[[1,2],1]\n"
    );
}

#[test]
fn alternative_patterns_try_each_shape_in_turn() {
    assert_eq!(
        run(".[] as [$a] ?// {a: $a} | $a", r#"[[1],{"a":2}]"#),
        "1\n2\n"
    );
    assert_eq!(run(".[] as [$a] ?// $a | $a", "[[3],4]"), "3\n4\n");
}

#[test]
fn reduce_and_foreach_destructure_each_item() {
    assert_eq!(
        run(
            "reduce .[] as [$k, $v] ({}; .[$k] = $v)",
            r#"[["a",1],["b",2]]"#
        ),
        "{\"a\":1,\"b\":2}\n"
    );
    assert_eq!(
        run(
            "[foreach .[] as {n: $n} (0; . + $n)]",
            r#"[{"n":1},{"n":2}]"#
        ),
        "[1,3]\n"
    );
}

#[test]
fn bindings_shadow_and_scope_to_the_rest_of_the_expression() {
    assert_eq!(run("1 as $x | (2 as $x | $x) + $x", "null"), "3\n");
    assert_eq!(run("[.[] as $x | $x] | length", "[1,2,3]"), "3\n");
}

#[test]
fn location_is_available_as_a_variable() {
    assert_eq!(
        run("$__loc__", "null"),
        "{\"file\":\"<top-level>\",\"line\":1}\n"
    );
}
