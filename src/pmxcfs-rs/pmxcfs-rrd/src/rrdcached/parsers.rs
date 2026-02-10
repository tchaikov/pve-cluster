use nom::{
    character::complete::{i64 as parse_i64, newline, not_line_ending, space1},
    sequence::terminated,
    IResult, Parser,
};

use super::errors::RRDCachedClientError;

/// Parse response line from rrdcached in format: "code message\n"
///
/// # Arguments
/// * `input` - Response line from rrdcached
///
/// # Returns
/// * `Ok((code, message))` - Parsed code and message
/// * `Err(RRDCachedClientError::Parsing)` - If parsing fails
///
/// # Example
/// ```ignore
/// let (code, message) = parse_response_line("0 OK\n")?;
/// ```
pub fn parse_response_line(input: &str) -> Result<(i64, &str), RRDCachedClientError> {
    let parse_result: IResult<&str, (i64, &str)> = (
        terminated(parse_i64, space1),
        terminated(not_line_ending, newline),
    )
        .parse(input);

    match parse_result {
        Ok((_, (code, message))) => Ok((code, message)),
        Err(_) => Err(RRDCachedClientError::Parsing("parse error".to_string())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_response_line() {
        let input = "1234  hello world\n";
        let result = parse_response_line(input);
        assert_eq!(result.unwrap(), (1234, "hello world"));

        let input = "1234  hello world";
        let result = parse_response_line(input);
        assert!(result.is_err());

        let input = "0 PONG\n";
        let result = parse_response_line(input);
        assert_eq!(result.unwrap(), (0, "PONG"));

        let input = "-20 errors, a lot of errors\n";
        let result = parse_response_line(input);
        assert_eq!(result.unwrap(), (-20, "errors, a lot of errors"));

        let input = "";
        let result = parse_response_line(input);
        assert!(result.is_err());

        let input = "1234";
        let result = parse_response_line(input);
        assert!(result.is_err());
    }
}
