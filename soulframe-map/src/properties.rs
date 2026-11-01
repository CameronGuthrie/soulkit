//! Small reader for the textual property blocks stored in level records.
//! Preserve brace nesting so child fields cannot override their parent's fields.
use std::collections::BTreeMap;

pub(crate) fn fields(text: &str) -> BTreeMap<String, String> {
    let bytes = text.as_bytes();
    let mut out = BTreeMap::new();
    let mut i = 0;
    while i < bytes.len() {
        while i < bytes.len() && (bytes[i].is_ascii_whitespace() || bytes[i] == b',') {
            i += 1;
        }
        let start = i;
        while i < bytes.len() && (bytes[i].is_ascii_alphanumeric() || bytes[i] == b'_') {
            i += 1;
        }
        if i == start || bytes.get(i) != Some(&b'=') {
            break;
        }
        let key = &text[start..i];
        i += 1;
        let start = i;
        let (mut depth, mut quoted, mut escaped) = (0usize, false, false);
        while i < bytes.len() {
            let ch = bytes[i];
            if quoted {
                if escaped {
                    escaped = false;
                } else if ch == b'\\' {
                    escaped = true;
                } else if ch == b'"' {
                    quoted = false;
                }
            } else {
                match ch {
                    b'"' => quoted = true,
                    b'{' => depth += 1,
                    b'}' if depth > 0 => depth -= 1,
                    b'\n' | b'\r' | b',' if depth == 0 => break,
                    _ => {}
                }
            }
            i += 1;
        }
        out.insert(
            key.to_owned(),
            text[start..i].trim().trim_matches('"').to_owned(),
        );
    }
    out
}

pub(crate) fn list(text: &str) -> Vec<&str> {
    let text = text.trim();
    let inner = text
        .strip_prefix('{')
        .and_then(|s| s.strip_suffix('}'))
        .unwrap_or(text);
    let bytes = inner.as_bytes();
    let (mut start, mut depth, mut quoted) = (0, 0, false);
    let mut out = Vec::new();
    for (i, ch) in bytes.iter().enumerate() {
        if *ch == b'"' && (i == 0 || bytes[i - 1] != b'\\') {
            quoted = !quoted;
        }
        if quoted {
            continue;
        }
        match ch {
            b'{' => depth += 1,
            b'}' => depth -= 1,
            b',' if depth == 0 => {
                out.push(inner[start..i].trim().trim_matches('"'));
                start = i + 1;
            }
            _ => {}
        }
    }
    out.push(inner[start..].trim().trim_matches('"'));
    out.into_iter().filter(|item| !item.is_empty()).collect()
}

pub(crate) fn nested_field<'a>(text: &'a str, name: &str) -> Option<&'a str> {
    // Script parameters such as _locTag can occur immediately after a '{'.
    let needle = format!("{name}=");
    text.match_indices(&needle).find_map(|(start, _)| {
        if start > 0
            && (text.as_bytes()[start - 1].is_ascii_alphanumeric()
                || text.as_bytes()[start - 1] == b'_')
        {
            return None;
        }
        let rest = &text[start + needle.len()..];
        Some(
            rest.split(['\n', '\r', '}', ','])
                .next()?
                .trim()
                .trim_matches('"'),
        )
    })
}

pub(crate) fn vector(text: &str) -> Option<[f32; 3]> {
    let values: Vec<f32> = list(text)
        .into_iter()
        .map(str::parse)
        .collect::<Result<_, _>>()
        .ok()?;
    let result: [f32; 3] = values.try_into().ok()?;
    result.iter().all(|n| n.is_finite()).then_some(result)
}

pub(crate) fn deep_field(text: &str, name: &str) -> Option<String> {
    let props = fields(text);
    if let Some(value) = props.get(name) {
        return Some(value.clone());
    }
    for value in props.values().filter(|s| s.starts_with('{')) {
        let inner = value.strip_prefix('{')?.strip_suffix('}')?;
        if let Some(found) = deep_field(inner, name) {
            return Some(found);
        }
        for item in list(value) {
            if let Some(inner) = item.strip_prefix('{').and_then(|s| s.strip_suffix('}')) {
                if let Some(found) = deep_field(inner, name) {
                    return Some(found);
                }
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn keeps_nested_fields_and_quoted_commas_in_their_scope() {
        let p = fields("Instance=0\nChildren={{Type=/SF/Child\nPosition={1,2,3}\nEnabled=0\n}}\nEnabled=1\nTag=\"a,b\"\n");
        assert_eq!(p["Enabled"], "1");
        assert_eq!(p["Tag"], "a,b");
        assert!(!p.contains_key("Position"));
        assert_eq!(list("{{Type=A\n},{Type=B\n}}").len(), 2);
    }
}
