#[test]
fn the_handler_state_is_checked_at_compile_time() {
    let cases = trybuild::TestCases::new();

    cases.compile_fail("tests/ui/fail/*.rs");
    cases.pass("tests/ui/pass/*.rs");
}
