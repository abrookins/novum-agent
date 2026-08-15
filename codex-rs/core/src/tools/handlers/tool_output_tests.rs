use super::*;

#[test]
fn rejects_invalid_search_arguments_before_reading_output() {
    let empty_pattern = validate_search_args(&SearchArgs {
        handle: "out_abc".to_string(),
        pattern: String::new(),
        context_lines: 0,
    })
    .expect_err("empty patterns should be rejected");
    assert_eq!(empty_pattern.to_string(), "pattern must not be empty");

    let too_many_context_lines = validate_search_args(&SearchArgs {
        handle: "out_abc".to_string(),
        pattern: "needle".to_string(),
        context_lines: MAX_CONTEXT_LINES + 1,
    })
    .expect_err("oversized context requests should be rejected");
    assert_eq!(
        too_many_context_lines.to_string(),
        format!("context_lines must be between 0 and {MAX_CONTEXT_LINES}")
    );
}

#[test]
fn clamps_read_to_the_active_policy() {
    assert_eq!(
        validate_max_tokens(10, TruncationPolicy::Tokens(4)).unwrap(),
        4
    );
}

#[test]
fn rejects_empty_read_windows_with_a_model_safe_error() {
    let error = validate_max_tokens(0, TruncationPolicy::Tokens(4))
        .expect_err("empty read windows should be rejected");
    assert_eq!(error.to_string(), "max_tokens must be greater than zero");
}
