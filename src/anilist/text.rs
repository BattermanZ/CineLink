pub(crate) fn clean_anilist_synopsis(input: &str) -> String {
    let without_tags = strip_html_with_breaks(input);
    let decoded = decode_basic_html_entities(&without_tags);
    let without_sources = remove_source_blocks(&decoded);
    normalize_newlines(&without_sources)
}

fn strip_html_with_breaks(input: &str) -> String {
    // Strips tags while converting <br> (and <br/>, <br />) into newlines.
    let mut out = String::with_capacity(input.len());
    let mut chars = input.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch != '<' {
            out.push(ch);
            continue;
        }
        let mut tag = String::new();
        for c in chars.by_ref() {
            if c == '>' {
                break;
            }
            tag.push(c);
        }
        let tag = tag.trim().trim_start_matches('/').trim();
        if tag.get(..2).is_some_and(|p| p.eq_ignore_ascii_case("br")) {
            out.push('\n');
        }
    }
    out
}

fn decode_basic_html_entities(input: &str) -> String {
    // Minimal entity decoding for AniList descriptions.
    // Supports common named entities and numeric (decimal/hex) entities.
    let mut out = String::with_capacity(input.len());
    let mut chars = input.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch != '&' {
            out.push(ch);
            continue;
        }
        let mut entity = String::new();
        while let Some(&c) = chars.peek() {
            chars.next();
            if c == ';' {
                break;
            }
            if entity.len() > 32 {
                entity.clear();
                break;
            }
            entity.push(c);
        }
        if entity.is_empty() {
            out.push('&');
            continue;
        }
        let decoded = match entity.as_str() {
            "amp" => Some('&'),
            "lt" => Some('<'),
            "gt" => Some('>'),
            "quot" => Some('"'),
            "apos" => Some('\''),
            "nbsp" => Some(' '),
            _ if entity.starts_with("#x") || entity.starts_with("#X") => {
                u32::from_str_radix(&entity[2..], 16)
                    .ok()
                    .and_then(char::from_u32)
            }
            _ if entity.starts_with('#') => {
                entity[1..].parse::<u32>().ok().and_then(char::from_u32)
            }
            _ => None,
        };
        if let Some(c) = decoded {
            out.push(c);
        } else {
            out.push('&');
            out.push_str(&entity);
            out.push(';');
        }
    }
    out
}

fn remove_source_blocks(input: &str) -> String {
    // Remove "(Source: ...)" blocks (case-insensitive), common in AniList blurbs.
    let lower = input.to_ascii_lowercase();
    let mut out = String::with_capacity(input.len());
    let mut idx = 0;
    while let Some(pos) = lower[idx..].find("(source:") {
        let start = idx + pos;
        out.push_str(&input[idx..start]);
        let rest = &lower[start..];
        if let Some(end_rel) = rest.find(')') {
            idx = start + end_rel + 1;
        } else {
            idx = input.len();
            break;
        }
    }
    out.push_str(&input[idx..]);
    out
}

fn normalize_newlines(input: &str) -> String {
    let input = input.replace("\r\n", "\n");
    let mut out = String::with_capacity(input.len());
    let mut nl_run = 0usize;

    for ch in input.chars() {
        if ch == '\n' {
            nl_run += 1;
            if nl_run <= 2 {
                out.push('\n');
            }
            continue;
        }
        nl_run = 0;
        out.push(ch);
    }
    out.trim().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cleans_anilist_synopsis_html_and_source() {
        let raw = "The third season of <i>One Punch Man</i>.<br><br>\n(Source: EMOTION Label YouTube Channel Description)<br><br>\n<i>Note: Excludes recap.</i>";
        let cleaned = clean_anilist_synopsis(raw);
        assert!(!cleaned.contains("<i>"));
        assert!(!cleaned.contains("<br"));
        assert!(!cleaned.to_ascii_lowercase().contains("source:"));
        assert!(cleaned.contains("The third season of One Punch Man."));
        assert!(cleaned.contains("Note: Excludes recap."));
    }
}
