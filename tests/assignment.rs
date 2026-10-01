//! Updating values in place: `=`, `|=`, arithmetic updates, deletion.

mod common;

use common::run;

const DOC: &str = r#"{"name":"web","port":80,"limits":{"cpu":2,"mem":512},"hosts":["a","b","c"]}"#;

#[test]
fn set_replaces_or_creates_a_value() {
    assert_eq!(run(".port = 8080 | .port", DOC), "8080\n");
    assert_eq!(
        run(".limits.disk = 10 | .limits", DOC),
        "{\"cpu\":2,\"mem\":512,\"disk\":10}\n"
    );
    assert_eq!(run(".a.b.c = 1", "{}"), "{\"a\":{\"b\":{\"c\":1}}}\n");
    assert_eq!(
        run(".list[2] = \"x\"", "{}"),
        "{\"list\":[null,null,\"x\"]}\n"
    );
    assert_eq!(run(".copy = .name | .copy", DOC), "\"web\"\n");
}

#[test]
fn update_applies_a_filter_to_the_current_value() {
    assert_eq!(run(".port |= . + 1 | .port", DOC), "81\n");
    assert_eq!(
        run(".hosts |= map(ascii_upcase) | .hosts", DOC),
        "[\"A\",\"B\",\"C\"]\n"
    );
    assert_eq!(
        run(".limits[] |= . * 2 | .limits", DOC),
        "{\"cpu\":4,\"mem\":1024}\n"
    );
    assert_eq!(
        run(".hosts[1] |= \"B\" | .hosts", DOC),
        "[\"a\",\"B\",\"c\"]\n"
    );
}

#[test]
fn arithmetic_updates() {
    assert_eq!(run(".port += 1 | .port", DOC), "81\n");
    assert_eq!(run(".limits.mem -= 12 | .limits.mem", DOC), "500\n");
    assert_eq!(run(".limits.cpu *= 4 | .limits.cpu", DOC), "8\n");
    assert_eq!(run(".limits.mem /= 2 | .limits.mem", DOC), "256\n");
    assert_eq!(run(".port %= 7 | .port", DOC), "3\n");
    assert_eq!(run(".hosts += [\"d\"] | .hosts | length", DOC), "4\n");
    assert_eq!(run(".name += \"-1\" | .name", DOC), "\"web-1\"\n");
}

#[test]
fn alternative_update_only_fills_in_missing_values() {
    assert_eq!(run(".timeout //= 30 | .timeout", DOC), "30\n");
    assert_eq!(run(".port //= 30 | .port", DOC), "80\n");
    assert_eq!(run(".flag //= true | .flag", r#"{"flag":false}"#), "true\n");
}

#[test]
fn updating_many_paths_at_once() {
    assert_eq!(
        run("(.port, .limits.cpu) |= . + 1 | [.port, .limits.cpu]", DOC),
        "[81,3]\n"
    );
    assert_eq!(
        run("(.. | numbers) |= . * 2", "[1,[2,{\"a\":3}]]"),
        "[2,[4,{\"a\":6}]]\n"
    );
    assert_eq!(run("(.[] | select(. > 1)) |= 0", "[1,2,3]"), "[1,0,0]\n");
    assert_eq!(run(".[] += 1", r#"{"a":1,"b":2}"#), "{\"a\":2,\"b\":3}\n");
}

#[test]
fn updating_to_empty_deletes() {
    assert_eq!(run(".[] |= empty", "[1,2,3]"), "[]\n");
    assert_eq!(run("(.[] | select(. > 1)) |= empty", "[1,2,3]"), "[1]\n");
    assert_eq!(run(".a |= empty", r#"{"a":1,"b":2}"#), "{\"b\":2}\n");
}

#[test]
fn del_removes_keys_and_elements() {
    assert_eq!(
        run("del(.port) | keys", DOC),
        "[\"hosts\",\"limits\",\"name\"]\n"
    );
    assert_eq!(
        run("del(.hosts[0, 2])", DOC),
        "{\"name\":\"web\",\"port\":80,\"limits\":{\"cpu\":2,\"mem\":512},\"hosts\":[\"b\"]}\n"
    );
    assert_eq!(run("del(.[] | select(. % 2 == 0))", "[1,2,3,4]"), "[1,3]\n");
    assert_eq!(
        run(
            "del(.. | select(. == null))",
            r#"{"a":null,"b":[1,null,2]}"#
        ),
        "{\"b\":[1,2]}\n"
    );
    assert_eq!(run("del(.a, .b)", r#"{"a":1,"b":2,"c":3}"#), "{\"c\":3}\n");
}

#[test]
fn slices_can_be_assigned_and_deleted() {
    assert_eq!(run(".[1:3] = [\"x\"]", "[1,2,3,4]"), "[1,\"x\",4]\n");
    assert_eq!(run(".[2:] |= map(. * 10)", "[1,2,3,4]"), "[1,2,30,40]\n");
    assert_eq!(run("del(.[:2])", "[1,2,3,4]"), "[3,4]\n");
}

#[test]
fn setpath_getpath_and_delpaths_work_with_computed_paths() {
    assert_eq!(
        run("setpath([\"a\", 1]; \"x\")", "{}"),
        "{\"a\":[null,\"x\"]}\n"
    );
    assert_eq!(run("getpath([\"limits\", \"cpu\"])", DOC), "2\n");
    assert_eq!(
        run("delpaths([[\"limits\", \"cpu\"], [\"name\"]]) | keys", DOC),
        "[\"hosts\",\"limits\",\"port\"]\n"
    );
    assert_eq!(run("[paths] | length", "{\"a\":[1,{\"b\":2}]}"), "4\n");
}

#[test]
fn the_right_side_is_evaluated_against_the_original_input() {
    assert_eq!(
        run(".total = (.a + .b) | .total", r#"{"a":1,"b":2}"#),
        "3\n"
    );
    assert_eq!(run(".a = (.a, 5)", r#"{"a":1}"#), "{\"a\":1}\n{\"a\":5}\n");
}
