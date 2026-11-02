//! Source lint for generated mod code (spec §5.3 gates):
//! - no `unsafe`
//! - only `horizon_client::mod_api` re-exports and `Host` — no internals
//! - `#![deny(warnings)]` expected at the top of lib.rs

/// Returns (fatal, message) pairs.
pub fn lint(source: &str) -> Vec<(bool, String)> {
    let mut out = Vec::new();

    for (i, line) in source.lines().enumerate() {
        let trimmed = trim_comments(line);
        if trimmed.contains("unsafe") {
            out.push((
                true,
                format!(
                    "line {}: `unsafe` is forbidden in native mod sources",
                    i + 1
                ),
            ));
        }
        for (idx, _) in trimmed.match_indices("horizon_client::") {
            let rest = &trimmed[idx + "horizon_client::".len()..];
            let allowed = rest.starts_with("mod_api") || rest.starts_with("Host");
            if !allowed {
                out.push((
                    true,
                    format!(
                        "line {}: `horizon_client::{}` — only `mod_api` re-exports and `Host` may be used",
                        i + 1,
                        rest.split(|c: char| !c.is_ascii_alphanumeric() && c != '_' && c != ':')
                            .next()
                            .unwrap_or(rest)
                    ),
                ));
                break;
            }
        }
    }

    out
}

fn trim_comments(line: &str) -> String {
    match line.find("//") {
        Some(i) => line[..i].to_string(),
        None => line.to_string(),
    }
}
