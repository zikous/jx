//! Number handling: literals keep their text, computed numbers print as shortest round-trip doubles.

mod common;

use common::{exec, run};

#[test]
fn literals_pass_through_with_their_original_text() {
    assert_eq!(
        run(".", "[1.0, 1.50, 100000000000000000000, 0.10, -0, 1E2]"),
        "[1.0,1.50,100000000000000000000,0.10,-0,1E+2]\n"
    );
    assert_eq!(
        run("map(.)", "[12345678901234567890]"),
        "[12345678901234567890]\n"
    );
}

#[test]
fn computed_numbers_print_as_shortest_round_trip_doubles() {
    assert_eq!(run("[. + 0]", "0.1"), "[0.1]\n");
    assert_eq!(run("0.1 + 0.2", "null"), "0.30000000000000004\n");
    assert_eq!(run("1 / 3", "null"), "0.3333333333333333\n");
    assert_eq!(run("[3 * 1.5, 10 / 4, 2 - 0.5]", "null"), "[4.5,2.5,1.5]\n");
}

#[test]
fn exponent_form_switches_at_fixed_magnitudes() {
    assert_eq!(
        run(
            "[1e17 + 0, 1e15 + 0.5, 1.5e300 + 0, 1e-5 + 0, 0.0001 + 0]",
            "null"
        ),
        "[1e+17,1000000000000000.5,1.5e+300,1e-05,0.0001]\n"
    );
    assert_eq!(
        run("[100000 * 100000, 123456789 * 1000]", "null"),
        "[10000000000,123456789000]\n"
    );
}

#[test]
fn integers_valued_doubles_print_without_a_fraction() {
    assert_eq!(run("[2 * 3, 6 / 2, 7.0 + 1]", "null"), "[6,3,8]\n");
}

#[test]
fn special_values_follow_json_rules() {
    assert_eq!(
        run("[nan, infinite, -infinite] | map(tojson)", "null"),
        "[\"null\",\"1.7976931348623157e+308\",\"-1.7976931348623157e+308\"]\n"
    );
    assert_eq!(
        run("[nan] | sort, (nan < 1), (nan == nan)", "null"),
        "[null]\ntrue\nfalse\n"
    );
    assert_eq!(
        run(
            "[infinite, -infinite, nan] | map(isinfinite, isnan)",
            "null"
        ),
        "[true,false,true,false,false,true]\n"
    );
}

#[test]
fn modulo_truncates_toward_zero() {
    assert_eq!(
        run("[5 % 3, -5 % 3, 5 % -3, 5.9 % 3]", "null"),
        "[2,-2,2,2]\n"
    );
}

#[test]
fn rounding_functions() {
    assert_eq!(run("map(floor)", "[1.5,-1.5,2]"), "[1,-2,2]\n");
    assert_eq!(run("map(ceil)", "[1.5,-1.5,2]"), "[2,-1,2]\n");
    assert_eq!(run("map(round)", "[0.5,1.5,-0.5,2.4]"), "[1,2,-1,2]\n");
    assert_eq!(run("map(trunc)", "[1.9,-1.9]"), "[1,-1]\n");
    assert_eq!(run("map(fabs)", "[-1.5,2]"), "[1.5,2]\n");
}

#[test]
fn math_functions() {
    assert_eq!(run("[sqrt, cbrt]", "27"), "[5.196152422706632,3]\n");
    assert_eq!(
        run("[pow(2; 10), pow(2; 0.5), pow(10; -2)]", "null"),
        "[1024,1.4142135623730951,0.01]\n"
    );
    assert_eq!(
        run("[log2, log10, exp2, exp10]", "8"),
        "[3,0.9030899869919435,256,100000000]\n"
    );
    assert_eq!(
        run("[atan2(1; 1) * 4, (1 | atan * 4)]", "null"),
        "[3.141592653589793,3.141592653589793]\n"
    );
    assert_eq!(run("[10, 3] | fmod(.[0]; .[1])", "null"), "1\n");
}

#[test]
fn tonumber_and_tostring_round_trip() {
    assert_eq!(
        run("map(tonumber)", r#"["1"," 2 ","3.5","1e3"]"#),
        "[1,2,3.5,1E+3]\n"
    );
    assert_eq!(
        run("map(tostring)", "[1,1.5,100000000000000000000]"),
        "[\"1\",\"1.5\",\"100000000000000000000\"]\n"
    );
    assert_eq!(run("length", "-7"), "7\n");
}

#[test]
fn comparison_treats_equal_values_as_equal_regardless_of_spelling() {
    assert_eq!(run(".[0] == .[1]", "[1, 1.0]"), "true\n");
    assert_eq!(run(".[0] == .[1]", "[100, 1e2]"), "true\n");
    assert_eq!(run("unique", "[1, 1.0, 2]"), "[1,2]\n");
}

#[test]
fn division_by_zero_is_an_error_but_infinities_are_values() {
    assert_eq!(exec(&["1 / 0"], "null").code, 5);
    assert_eq!(run("1e1000 > 1e308", "null"), "true\n");
}
