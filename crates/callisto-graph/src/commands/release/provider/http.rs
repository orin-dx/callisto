//! Raw HTTP response parsing for the forge lookups that go through
//! `gh api --include`.
//!
//! The response is kept whole -- status line, headers, body -- because the
//! retry policy needs `Retry-After` and the rate-limit headers, and the
//! classifier needs the exact status.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// One parsed HTTP response.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct HttpResponse {
    pub(crate) status: u16,
    pub(crate) headers: Vec<(String, String)>,
    pub(crate) body: String,
}

impl HttpResponse {
    pub(crate) fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(key, _)| key.eq_ignore_ascii_case(name))
            .map(|(_, value)| value.as_str())
    }

    /// The server's own wait instruction, in delta-seconds or as an HTTP-date.
    pub(crate) fn retry_after(&self) -> Option<Duration> {
        parse_retry_after(self.header("retry-after")?, now_unix_seconds())
    }

    /// Whether a 403 is a rate-limit refusal rather than a real authorization
    /// failure; only the former may be retried.
    pub(crate) fn is_rate_limited(&self) -> bool {
        self.header("retry-after").is_some()
            || self
                .header("x-ratelimit-remaining")
                .is_some_and(|value| value.trim() == "0")
    }

    /// A rate limit or server error may resolve on its own, unlike a client error or malformed body. Shared by
    /// every status-reading provider (`gh api`, PyPI) so they can't drift on which statuses retry.
    pub(crate) fn is_transient(&self) -> bool {
        matches!(self.status, 429 | 500..=599) || (self.status == 403 && self.is_rate_limited())
    }
}

fn now_unix_seconds() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_secs())
}

/// Splits at the first blank line, whichever line ending comes first.
fn split_head(raw: &str) -> Option<(&str, &str)> {
    let crlf = raw.find("\r\n\r\n").map(|at| (at, 4));
    let lf = raw.find("\n\n").map(|at| (at, 2));
    let (at, width) = match (crlf, lf) {
        (Some(crlf), Some(lf)) => crlf.min(lf),
        (crlf, lf) => crlf.or(lf)?,
    };
    Some((&raw[..at], &raw[at + width..]))
}

/// Parses a raw `gh api --include` response, header block first: the first blank line ends the headers and
/// everything after it is the body, blank lines included. An interim `1xx` or a header-less proxy preamble
/// followed by another status line is skipped, so it cannot be mistaken for the real status.
pub(crate) fn parse_http_response(raw: &str) -> Result<HttpResponse, String> {
    let mut rest = raw;
    loop {
        let (head, body) =
            split_head(rest).ok_or_else(|| "response has no blank line separating headers from body".to_owned())?;
        let mut lines = head.lines().map(|line| line.trim_end_matches('\r'));
        let status_line = lines
            .next()
            .filter(|line| line.starts_with("HTTP/"))
            .ok_or_else(|| "response headers contain no HTTP status line".to_owned())?;
        let status = status_line
            .split_whitespace()
            .nth(1)
            .ok_or_else(|| "HTTP status line has no status code".to_owned())?
            .parse::<u16>()
            .map_err(|error| format!("HTTP status code is not a valid number: {error}"))?;
        let headers: Vec<(String, String)> = lines
            .filter_map(|line| line.split_once(':'))
            .map(|(name, value)| (name.trim().to_owned(), value.trim().to_owned()))
            .collect();
        if (status < 200 || headers.is_empty()) && body.starts_with("HTTP/") {
            rest = body;
            continue;
        }
        return Ok(HttpResponse {
            status,
            headers,
            body: body.to_owned(),
        });
    }
}

/// `Retry-After` is either delta-seconds or an HTTP-date; a date already past
/// means "retry now", never a negative wait.
fn parse_retry_after(raw: &str, now: u64) -> Option<Duration> {
    let raw = raw.trim();
    if let Ok(seconds) = raw.parse::<u64>() {
        return Some(Duration::from_secs(seconds));
    }
    let at = parse_http_date(raw)?;
    Some(Duration::from_secs(at.saturating_sub(now)))
}

/// IMF-fixdate (`Sun, 06 Nov 1994 08:49:37 GMT`) as Unix seconds. The two
/// obsolete formats RFC 7231 tolerates are not accepted: a `Retry-After` we
/// cannot read falls back to the exponential schedule, which is safe.
fn parse_http_date(raw: &str) -> Option<u64> {
    let rest = raw.split_once(", ").map(|(_, rest)| rest)?;
    let mut fields = rest.split_whitespace();
    let day: i64 = fields.next()?.parse().ok()?;
    let month = match fields.next()? {
        "Jan" => 1,
        "Feb" => 2,
        "Mar" => 3,
        "Apr" => 4,
        "May" => 5,
        "Jun" => 6,
        "Jul" => 7,
        "Aug" => 8,
        "Sep" => 9,
        "Oct" => 10,
        "Nov" => 11,
        "Dec" => 12,
        _ => return None,
    };
    let year: i64 = fields.next()?.parse().ok()?;
    let mut clock = fields.next()?.split(':');
    let hour: i64 = clock.next()?.parse().ok()?;
    let minute: i64 = clock.next()?.parse().ok()?;
    let second: i64 = clock.next()?.parse().ok()?;
    if fields.next() != Some("GMT") || !(1..=31).contains(&day) {
        return None;
    }
    let seconds = days_from_civil(year, month, day) * 86_400 + hour * 3_600 + minute * 60 + second;
    u64::try_from(seconds).ok()
}

/// Howard Hinnant's days-from-civil: days since 1970-01-01, proleptic Gregorian.
fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let year = year - i64::from(month <= 2);
    let era = if year >= 0 { year } else { year - 399 } / 400;
    let year_of_era = year - era * 400;
    let day_of_year = (153 * (month + if month > 2 { -3 } else { 9 }) + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    era * 146_097 + day_of_era - 719_468
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_response_is_split_at_the_last_header_block_and_keeps_its_headers() {
        let parsed = parse_http_response(
            "HTTP/1.1 429 Too Many Requests\r\nRetry-After: 7\r\nX-RateLimit-Remaining: 0\r\n\r\nslow down",
        )
        .unwrap();
        assert_eq!(parsed.status, 429);
        assert_eq!(parsed.body, "slow down");
        assert_eq!(parsed.retry_after(), Some(Duration::from_secs(7)));
        assert!(parsed.is_rate_limited());
    }

    #[test]
    fn a_body_containing_blank_lines_is_kept_whole() {
        let body = "{\n  \"a\": 1,\n\n  \"b\": 2\n}\n\n";
        let crlf = parse_http_response(&format!("HTTP/2 200\r\ncontent-type: application/json\r\n\r\n{body}")).unwrap();
        assert_eq!(crlf.body, body);
        let lf = parse_http_response(&format!("HTTP/2 200\ncontent-type: application/json\n\n{body}")).unwrap();
        assert_eq!(lf.body, body);
        assert_eq!(lf.header("content-type"), Some("application/json"));
    }

    #[test]
    fn an_interim_continue_block_is_skipped() {
        let parsed =
            parse_http_response("HTTP/1.1 100 Continue\r\n\r\nHTTP/1.1 201 Created\r\nX-A: b\r\n\r\nok\r\n\r\nmore")
                .unwrap();
        assert_eq!(parsed.status, 201);
        assert_eq!(parsed.body, "ok\r\n\r\nmore");
    }

    #[test]
    fn a_response_without_a_status_line_or_header_break_is_malformed() {
        assert!(parse_http_response("not a response").is_err());
        assert!(parse_http_response("garbage\r\n\r\nbody").is_err());
    }

    #[test]
    fn a_plain_forbidden_response_is_not_a_rate_limit() {
        let parsed = parse_http_response("HTTP/2 403\r\ncontent-type: application/json\r\n\r\n{}").unwrap();
        assert_eq!(parsed.status, 403);
        assert!(!parsed.is_rate_limited());
        assert_eq!(parsed.retry_after(), None);
    }

    #[test]
    fn retry_after_reads_both_delta_seconds_and_an_http_date() {
        assert_eq!(parse_retry_after("120", 0), Some(Duration::from_secs(120)));
        // 1994-11-06T08:49:37Z is 784111777 seconds after the epoch.
        assert_eq!(
            parse_retry_after("Sun, 06 Nov 1994 08:49:37 GMT", 784_111_700),
            Some(Duration::from_secs(77))
        );
        assert_eq!(
            parse_retry_after("Sun, 06 Nov 1994 08:49:37 GMT", 784_111_900),
            Some(Duration::ZERO),
            "a date already past must never produce a negative or huge wait"
        );
        assert_eq!(parse_retry_after("whenever", 0), None);
    }
}
