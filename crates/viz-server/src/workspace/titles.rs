const MAX: usize = 60;

pub(super) fn from_prompt(text: &str) -> String {
    let mut fenced = false;
    let mut fallback = None;
    for raw in text.lines() {
        let raw = raw.trim();
        if raw.starts_with("```") || raw.starts_with("~~~") {
            fenced = !fenced;
            continue;
        }
        let line = clean(raw);
        if line.is_empty() {
            continue;
        }
        if !fenced {
            return shorten(line);
        }
        if fallback.is_none() {
            fallback = Some(line);
        }
    }
    fallback.map_or_else(String::new, shorten)
}

fn clean(raw: &str) -> String {
    let mut line = raw;
    loop {
        if let Some(rest) = line.strip_prefix('>') {
            line = rest.trim_start();
            continue;
        }
        let Some((prefix, rest)) = line.split_once(char::is_whitespace) else {
            break;
        };
        let number = prefix
            .strip_suffix(['.', ')'])
            .filter(|number| !number.is_empty());
        if matches!(prefix, "-" | "*" | "+" | "[x]" | "[X]")
            || prefix.starts_with('#')
            || number.is_some_and(|number| number.bytes().all(|byte| byte.is_ascii_digit()))
        {
            line = rest.trim_start();
        } else {
            break;
        }
    }
    let mut title = String::new();
    for word in line.split_whitespace() {
        let word = word.trim_matches('`').trim_matches('*');
        if word.is_empty() {
            continue;
        }
        if !title.is_empty() {
            title.push(' ');
        }
        title.push_str(word);
    }
    title.truncate(title.trim_end_matches([' ', ':', ';', ',']).len());
    title
}

fn shorten(line: String) -> String {
    let Some((end, _)) = line.char_indices().nth(MAX) else {
        return line;
    };
    let head = &line[..end];
    let sentence = head.char_indices().skip(20).find_map(|(i, ch)| {
        let after = i + ch.len_utf8();
        (matches!(ch, '.' | '?' | '!') && head[after..].starts_with(char::is_whitespace))
            .then(|| &head[..after])
    });
    if let Some(sentence) = sentence {
        return sentence.trim_end_matches('.').to_owned();
    }
    let head = &head[..head.char_indices().next_back().map_or(0, |(i, _)| i)];
    let cut = head
        .rsplit_once(' ')
        .and_then(|(words, _)| (words.chars().count() >= MAX / 2).then_some(words))
        .unwrap_or(head);
    let mut title = cut
        .trim_end_matches([' ', '.', ',', ':', ';', '-'])
        .to_owned();
    title.push('…');
    title
}
