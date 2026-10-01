//! Selecting and navigating values.

mod common;

use common::{exec, run};

const DOC: &str = r#"{"users":[{"id":1,"name":"Ada","tags":["x","y"],"manager":null},{"id":2,"name":"Linus","tags":[],"manager":1},{"id":3,"name":"Grace","tags":["y"]}]}"#;

#[test]
fn field_access_chains_and_missing_fields_are_null() {
    assert_eq!(run(".users[0].name", DOC), "\"Ada\"\n");
    assert_eq!(run(".users[2].manager", DOC), "null\n");
    assert_eq!(run(".nope.deeper.still", DOC), "null\n");
    assert_eq!(run(".users[].name", DOC), "\"Ada\"\n\"Linus\"\n\"Grace\"\n");
}

#[test]
fn select_keeps_only_matching_values() {
    assert_eq!(
        run(".users[] | select(.id > 1) | .name", DOC),
        "\"Linus\"\n\"Grace\"\n"
    );
    assert_eq!(
        run(".users[] | select(.tags | index(\"y\")) | .id", DOC),
        "1\n3\n"
    );
    assert_eq!(
        run(".users[] | select(.manager) | .name", DOC),
        "\"Linus\"\n"
    );
    assert_eq!(
        run(".users[] | select(.name | test(\"^[AG]\")) | .id", DOC),
        "1\n3\n"
    );
}

#[test]
fn optional_access_suppresses_errors() {
    assert_eq!(run("[.[] | .a?]", r#"[{"a":1},5,"s",{"a":2}]"#), "[1,2]\n");
    assert_eq!(run("[.[]?]", "3"), "[]\n");
    let failing = exec(&["-c", "[.[] | .a]"], r#"[{"a":1},5]"#);
    assert_eq!(failing.code, 5, "without ? the error is reported");
}

#[test]
fn recursive_descent_visits_every_value() {
    assert_eq!(run("[.. | numbers]", DOC), "[1,2,1,3]\n");
    assert_eq!(run("[.. | strings] | length", DOC), "6\n");
    assert_eq!(
        run("[.. | objects | .id?] | map(select(. != null))", DOC),
        "[1,2,3]\n"
    );
}

#[test]
fn alternative_operator_supplies_defaults() {
    assert_eq!(
        run(".users[] | .manager // \"none\"", DOC),
        "\"none\"\n1\n\"none\"\n"
    );
    assert_eq!(
        run(".a // .b // \"c\"", r#"{"a":false,"b":null}"#),
        "\"c\"\n"
    );
    assert_eq!(run("[.[] // 0]", "[0,null,false,1]"), "[0,1]\n");
}

#[test]
fn slices_and_negative_indices() {
    assert_eq!(run(".[2:4]", "[0,1,2,3,4,5]"), "[2,3]\n");
    assert_eq!(run(".[-2:]", "[0,1,2,3]"), "[2,3]\n");
    assert_eq!(run(".[:1]", "\"héllo\""), "\"h\"\n");
    assert_eq!(run(".[1:3]", "\"héllo\""), "\"él\"\n");
    assert_eq!(run(".[-1], .[10]", "[1,2,3]"), "3\nnull\n");
}

#[test]
fn first_and_limit_stop_early() {
    assert_eq!(run("first(range(1000000000))", "null"), "0\n");
    assert_eq!(run("[limit(3; range(1000000000))]", "null"), "[0,1,2]\n");
    assert_eq!(run("first(.[] | select(. > 2))", "[1,5,3]"), "5\n");
    assert_eq!(
        run("[.[] | select(. > 9)] | first // \"none\"", "[1,2]"),
        "\"none\"\n"
    );
}

#[test]
fn comparison_and_boolean_operators() {
    assert_eq!(
        run(
            "[1 < 2, 2 <= 2, \"a\" < \"b\", [1] < [1,0], {} == {}]",
            "null"
        ),
        "[true,true,true,true,true]\n"
    );
    assert_eq!(
        run("[(true, false) and (true, false)]", "null"),
        "[true,false,false]\n"
    );
    assert_eq!(
        run("[null, false, 0, \"\", [], {}] | map(not)", "null"),
        "[true,true,false,false,false,false]\n"
    );
    assert_eq!(
        run(
            "map(if . < 0 then \"neg\" elif . == 0 then \"zero\" else \"pos\" end)",
            "[-1,0,1]"
        ),
        "[\"neg\",\"zero\",\"pos\"]\n"
    );
}

#[test]
fn values_sort_by_kind_then_value() {
    assert_eq!(
        run("sort", r#"[{"a":1},[2],"s",1,true,false,null]"#),
        "[null,false,true,1,\"s\",[2],{\"a\":1}]\n"
    );
}

#[test]
fn paths_locate_values_and_getpath_follows_them() {
    assert_eq!(
        run("[paths(. == \"y\")]", DOC),
        "[[\"users\",0,\"tags\",1],[\"users\",2,\"tags\",0]]\n"
    );
    assert_eq!(run("getpath([\"users\", 1, \"name\"])", DOC), "\"Linus\"\n");
    assert_eq!(run("[paths(type == \"number\")] | length", DOC), "4\n");
}
