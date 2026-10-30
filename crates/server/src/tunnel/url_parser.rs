use std::borrow::Cow;
use std::time::Duration;

/// Strips standard ANSI terminal escape sequences using zero-copy Cow when clean.
pub fn strip_ansi(s: &str) -> Cow<'_, str> {
    if !s.contains('\x1b') {
        return Cow::Borrowed(s);
    }
    let mut out = String::with_capacity(s.len());
    let mut in_escape = false;
    for c in s.chars() {
        if c == '\x1b' {
            in_escape = true;
        } else if in_escape {
            if c.is_ascii_alphabetic() {
                in_escape = false;
            }
        } else {
            out.push(c);
        }
    }
    Cow::Owned(out)
}

/// Validates whether a subdomain segment contains only RFC-compliant hostname characters.
pub fn is_valid_subdomain(subdomain: &str) -> bool {
    if subdomain.is_empty() || subdomain.len() > 63 {
        return false;
    }
    if subdomain.starts_with('-') || subdomain.ends_with('-') {
        return false;
    }
    subdomain
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '-')
}

/// Extracts a dynamic `trycloudflare.com` tunnel ingress URL from a raw log or output line.
pub fn extract_tunnel_url(raw_line: &str) -> Option<String> {
    let clean = strip_ansi(raw_line);
    let prefix = "https://";
    let target_suffix = ".trycloudflare.com";

    let mut start_idx = 0;
    while let Some(pos) = clean[start_idx..].find(prefix) {
        let actual_pos = start_idx + pos;
        let rest = &clean[actual_pos..];

        let end_offset = rest
            .find(|c: char| {
                c.is_whitespace()
                    || c == '"'
                    || c == '\''
                    || c == '`'
                    || c == '|'
                    || c == '>'
                    || c == '<'
                    || c == ')'
                    || c == ']'
                    || c == '}'
                    || c == '\\'
            })
            .unwrap_or(rest.len());

        let candidate = &rest[..end_offset];
        if candidate.len() > prefix.len() {
            let after_prefix = &candidate[prefix.len()..];
            let host_part = if let Some(slash_pos) = after_prefix.find('/') {
                &after_prefix[..slash_pos]
            } else {
                after_prefix
            };

            let host_lower = host_part.to_lowercase();
            if host_lower.ends_with(target_suffix) {
                let subdomain = &host_part[..host_part.len() - target_suffix.len()];
                if is_valid_subdomain(subdomain) {
                    return Some(format!("https://{host_part}"));
                }
            }
        }

        start_idx = actual_pos + prefix.len();
    }
    None
}

/// Extracts a custom tunnel URL with a user-specified custom hostname/domain.
pub fn extract_custom_tunnel_url(raw_line: &str, expected_domain: &str) -> Option<String> {
    let clean = strip_ansi(raw_line);
    let prefix = "https://";
    let clean_domain = expected_domain.trim_start_matches('.').to_lowercase();
    let target_suffix = format!(".{clean_domain}");

    let mut start_idx = 0;
    while let Some(pos) = clean[start_idx..].find(prefix) {
        let actual_pos = start_idx + pos;
        let rest = &clean[actual_pos..];

        let end_offset = rest
            .find(|c: char| {
                c.is_whitespace()
                    || c == '"'
                    || c == '\''
                    || c == '`'
                    || c == '|'
                    || c == '>'
                    || c == '<'
                    || c == ')'
                    || c == ']'
                    || c == '}'
                    || c == '\\'
            })
            .unwrap_or(rest.len());

        let candidate = &rest[..end_offset];
        if candidate.len() > prefix.len() {
            let after_prefix = &candidate[prefix.len()..];
            let host_part = if let Some(slash_pos) = after_prefix.find('/') {
                &after_prefix[..slash_pos]
            } else {
                after_prefix
            };

            let host_lower = host_part.to_lowercase();
            if host_lower.ends_with(&target_suffix) || host_lower == clean_domain {
                return Some(format!("https://{host_part}"));
            }
        }

        start_idx = actual_pos + prefix.len();
    }
    None
}

/// Probes reachability of an assigned ingress URL over HTTPS.
pub async fn probe_url_reachability(
    url: &str,
    timeout_duration: Duration,
) -> Result<bool, reqwest::Error> {
    let client = reqwest::Client::builder()
        .timeout(timeout_duration)
        .redirect(reqwest::redirect::Policy::limited(5))
        .danger_accept_invalid_certs(true)
        .build()?;

    match client.get(url).send().await {
        Ok(resp) => Ok(resp.status().is_success()
            || resp.status().is_redirection()
            || resp.status().is_client_error()),
        Err(e) => {
            if e.is_status() {
                Ok(true)
            } else {
                Err(e)
            }
        }
    }
}
