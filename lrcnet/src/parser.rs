use bex_core::lyrics::types::LyricsLine;

pub fn parse_lrc(lrc: &str) -> Vec<LyricsLine> {
    let mut lines = Vec::new();

    for line in lrc.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with("[ti:") || line.starts_with("[ar:") {
            continue;
        }

        if let Some(end_bracket) = line.find(']') {
            if line.starts_with('[') {
                let timestamp_str = &line[1..end_bracket];
                let content = line[end_bracket + 1..].trim();
                if let Some(start_ms) = parse_timestamp(timestamp_str) {
                    lines.push(LyricsLine {
                        start_ms,
                        duration_ms: None,
                        content: content.to_string(),
                        tokens: None,
                    });
                }
            }
        }
    }

    lines.sort_by_key(|l| l.start_ms);

    for i in 0..lines.len() {
        if i + 1 < lines.len() {
            lines[i].duration_ms = Some(lines[i + 1].start_ms - lines[i].start_ms);
        }
    }
    lines
}

fn parse_timestamp(ts: &str) -> Option<u32> {
    let parts: Vec<&str> = ts.split(':').collect();
    if parts.len() != 2 {
        return None;
    }

    let minutes = parts[0].parse::<u32>().ok()?;
    let seconds_parts: Vec<&str> = parts[1].split('.').collect();
    let seconds = seconds_parts[0].parse::<u32>().ok()?;
    let hundredths = if seconds_parts.len() > 1 {
        let h_str = seconds_parts[1];
        match h_str.len() {
            2 => h_str.parse::<u32>().ok()? * 10,
            3 => h_str.parse::<u32>().ok()?,
            _ => h_str.parse::<u32>().ok().unwrap_or(0),
        }
    } else {
        0
    };

    Some(minutes * 60000 + seconds * 1000 + hundredths)
}
