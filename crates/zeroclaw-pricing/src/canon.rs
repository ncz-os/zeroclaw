const KNOWN_PREFIXES: &[&str] = &[
    "openai/",
    "anthropic/",
    "nvidia/",
    "azure/",
    "google/",
    "meta/",
    "mistralai/",
    "bedrock/",
    "sakana/",
];

pub fn canonicalize(model_id: &str) -> (Option<String>, String) {
    let mut s = model_id.trim().to_ascii_lowercase();
    if let Some(rest) = s.strip_prefix('~') {
        s = rest.to_string();
    }

    for prefix in KNOWN_PREFIXES {
        if let Some(rest) = s.strip_prefix(prefix) {
            return (
                Some(
                    prefix
                        .trim_end_matches('/')
                        .trim_end_matches('.')
                        .to_string(),
                ),
                rest.to_string(),
            );
        }
    }

    if let Some((provider, rest)) = s.split_once('/') {
        return (Some(provider.to_string()), rest.to_string());
    }

    (None, s)
}
