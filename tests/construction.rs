//! Building new values: objects, arrays, strings and merges.

mod common;

use common::run;

const PERSON: &str = r#"{"first":"Ada","last":"Lovelace","born":1815,"langs":["en","fr"]}"#;

#[test]
fn objects_can_be_built_from_shorthand_and_expressions() {
    assert_eq!(
        run("{first, born}", PERSON),
        "{\"first\":\"Ada\",\"born\":1815}\n"
    );
    assert_eq!(
        run(
            "{name: (.first + \" \" + .last), age: (2024 - .born)}",
            PERSON
        ),
        "{\"name\":\"Ada Lovelace\",\"age\":209}\n"
    );
    assert_eq!(run("{(.first): .born}", PERSON), "{\"Ada\":1815}\n");
    assert_eq!(
        run("{\"a b\": 1, \"c\": .born}", PERSON),
        "{\"a b\":1,\"c\":1815}\n"
    );
}

#[test]
fn several_outputs_in_an_object_produce_every_combination() {
    assert_eq!(
        run("{a: (1, 2), b: (3, 4)}", "null"),
        "{\"a\":1,\"b\":3}\n{\"a\":1,\"b\":4}\n{\"a\":2,\"b\":3}\n{\"a\":2,\"b\":4}\n"
    );
}

#[test]
fn arrays_collect_outputs() {
    assert_eq!(
        run("[.langs[] | ascii_upcase]", PERSON),
        "[\"EN\",\"FR\"]\n"
    );
    assert_eq!(
        run("[.first, .last] | join(\" \")", PERSON),
        "\"Ada Lovelace\"\n"
    );
    assert_eq!(run("[range(3)] | map(. * .)", "null"), "[0,1,4]\n");
}

#[test]
fn string_interpolation_formats_values_inside_text() {
    assert_eq!(
        run(
            "\"\\(.first) was born in \\(.born); speaks \\(.langs | length) languages\"",
            PERSON
        ),
        "\"Ada was born in 1815; speaks 2 languages\"\n"
    );
    assert_eq!(
        run("\"\\(.langs)\"", PERSON),
        "\"[\\\"en\\\",\\\"fr\\\"]\"\n"
    );
}

#[test]
fn plus_merges_shallowly_and_star_merges_recursively() {
    assert_eq!(
        run(
            ".a + .b",
            r#"{"a":{"x":1,"y":{"p":1}},"b":{"y":{"q":2},"z":3}}"#
        ),
        "{\"x\":1,\"y\":{\"q\":2},\"z\":3}\n"
    );
    assert_eq!(
        run(
            ".a * .b",
            r#"{"a":{"x":1,"y":{"p":1}},"b":{"y":{"q":2},"z":3}}"#
        ),
        "{\"x\":1,\"y\":{\"p\":1,\"q\":2},\"z\":3}\n"
    );
    assert_eq!(
        run("{role: \"user\"} * .", r#"{"role":"admin","id":7}"#),
        "{\"role\":\"admin\",\"id\":7}\n"
    );
}

#[test]
fn entries_convert_between_objects_and_lists() {
    assert_eq!(
        run(
            "to_entries | map(\"\\(.key)=\\(.value)\") | join(\"&\")",
            r#"{"a":1,"b":"x"}"#
        ),
        "\"a=1&b=x\"\n"
    );
    assert_eq!(
        run("with_entries(.key |= ascii_upcase)", r#"{"a":1,"b":2}"#),
        "{\"A\":1,\"B\":2}\n"
    );
    assert_eq!(
        run("with_entries(select(.value > 1))", r#"{"a":1,"b":2}"#),
        "{\"b\":2}\n"
    );
    assert_eq!(
        run(
            "from_entries",
            r#"[{"key":"a","value":1},{"name":"b","value":2}]"#
        ),
        "{\"a\":1,\"b\":2}\n"
    );
    assert_eq!(
        run("map_values(. + 1)", r#"{"a":1,"b":2}"#),
        "{\"a\":2,\"b\":3}\n"
    );
}

#[test]
fn index_builds_a_lookup_table() {
    let rows = r#"[{"id":"u1","n":"Ada"},{"id":"u2","n":"Linus"}]"#;
    assert_eq!(
        run("INDEX(.id)", rows),
        "{\"u1\":{\"id\":\"u1\",\"n\":\"Ada\"},\"u2\":{\"id\":\"u2\",\"n\":\"Linus\"}}\n"
    );
    assert_eq!(run("INDEX(.id) | .u2.n", rows), "\"Linus\"\n");
}

#[test]
fn keys_values_and_membership() {
    assert_eq!(run("keys", r#"{"b":1,"a":2}"#), "[\"a\",\"b\"]\n");
    assert_eq!(run("keys_unsorted", r#"{"b":1,"a":2}"#), "[\"b\",\"a\"]\n");
    assert_eq!(
        run("[has(\"a\"), has(\"z\")]", r#"{"a":1}"#),
        "[true,false]\n"
    );
    assert_eq!(run("map(has(1))", "[[1],[1,2]]"), "[false,true]\n");
    assert_eq!(
        run("[.[] | contains([2])]", "[[1,2],[3]]"),
        "[true,false]\n"
    );
}

#[test]
fn flatten_transpose_and_combinations() {
    assert_eq!(run("flatten", "[1,[2,[3,[4]]]]"), "[1,2,3,4]\n");
    assert_eq!(run("flatten(1)", "[1,[2,[3]]]"), "[1,2,[3]]\n");
    assert_eq!(run("transpose", "[[1,2],[3,4]]"), "[[1,3],[2,4]]\n");
    assert_eq!(
        run("[combinations]", "[[1,2],[3,4]]"),
        "[[1,3],[1,4],[2,3],[2,4]]\n"
    );
}
