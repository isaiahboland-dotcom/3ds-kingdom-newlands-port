//! Just enough Unity-YAML reading for the fields we need. Unity's YAML is
//! machine-written and regular, so line-based scanning is reliable here.

/// Value after `key:` on the first line (trimmed) that starts with `key:`.
pub fn field<'a>(text: &'a str, key: &str) -> Option<&'a str> {
    text.lines().find_map(|l| {
        let l = l.trim_start();
        l.strip_prefix(key)?.strip_prefix(':').map(str::trim)
    })
}

/// Parses `{x: 1, y: 2.5, z: 0}` into the values for `keys`, in order.
pub fn inline_map(s: &str, keys: &[&str]) -> Option<Vec<f32>> {
    let inner = s.trim().strip_prefix('{')?.strip_suffix('}')?;
    let mut out = Vec::with_capacity(keys.len());
    for k in keys {
        let v = inner.split(',').find_map(|kv| {
            let (a, b) = kv.split_once(':')?;
            (a.trim() == *k).then(|| b.trim())
        })?;
        out.push(parse_float(v)?);
    }
    Some(out)
}

/// Unity writes `Infinity` / `-Infinity` for stepped tangents.
pub fn parse_float(s: &str) -> Option<f32> {
    match s {
        "Infinity" => Some(f32::INFINITY),
        "-Infinity" => Some(f32::NEG_INFINITY),
        _ => s.parse().ok(),
    }
}

/// The GUID in a `{fileID: ..., guid: ..., type: ...}` reference.
pub fn guid_of(s: &str) -> Option<&str> {
    let start = s.find("guid: ")? + 6;
    let rest = &s[start..];
    Some(rest[..rest.find(',')?].trim())
}

pub fn hex_bytes(s: &str) -> Option<Vec<u8>> {
    let s = s.trim();
    if !s.len().is_multiple_of(2) {
        return None;
    }
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).ok())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_inline_maps_and_refs() {
        assert_eq!(
            inline_map("{x: 32, y: 917, z: 32, w: 458}", &["y", "w"]),
            Some(vec![917.0, 458.0])
        );
        assert_eq!(
            inline_map("{x: 0, y: Infinity, z: 0}", &["y"]),
            Some(vec![f32::INFINITY])
        );
        assert_eq!(
            guid_of("{fileID: 2800000, guid: 16cca6cc8033caa48a575d227e38e4b2, type: 3}"),
            Some("16cca6cc8033caa48a575d227e38e4b2")
        );
        assert_eq!(
            field("  m_Name: archer_idle_0\n", "m_Name"),
            Some("archer_idle_0")
        );
    }
}
