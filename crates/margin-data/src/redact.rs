use std::collections::BTreeMap;

const SENSITIVE_KEYS: &[&str] = &[
    "password",
    "secret",
    "token",
    "database_url",
    "authorization",
    "api_key",
];

pub fn redact(message: &str, secrets: &[&str]) -> String {
    let mut output = message.to_owned();
    for secret in secrets {
        if secret.len() >= 4 {
            output = output.replace(secret, "[redacted]");
        }
    }
    output
}

pub fn sanitize_audit_fields(fields: BTreeMap<String, String>) -> BTreeMap<String, String> {
    fields
        .into_iter()
        .map(|(key, value)| {
            let sensitive = SENSITIVE_KEYS
                .iter()
                .any(|needle| key.to_ascii_lowercase().contains(needle));
            if sensitive {
                (key, "[redacted]".to_owned())
            } else {
                (key, value)
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::{redact, sanitize_audit_fields};
    use std::collections::BTreeMap;

    #[test]
    fn secrets_are_removed_from_messages_and_audit_fields() {
        let message = redact(
            "connect postgres://margin:change-me@localhost/margin_engine failed",
            &["postgres://margin:change-me@localhost/margin_engine"],
        );
        assert!(!message.contains("change-me"));
        assert!(message.contains("[redacted]"));

        let mut fields = BTreeMap::new();
        fields.insert("database_url".to_owned(), "postgres://secret".to_owned());
        fields.insert("dataset_id".to_owned(), "abc".to_owned());
        let clean = sanitize_audit_fields(fields);
        assert_eq!(
            clean.get("database_url").map(String::as_str),
            Some("[redacted]")
        );
        assert_eq!(clean.get("dataset_id").map(String::as_str), Some("abc"));
    }
}
