//! Lyrics as text: LRC parsing and "which line is sung now". Shared by the
//! terminal app (sidecar `.lrc`, tags, LRCLIB) and the iOS app (LRCLIB),
//! and drawn by `cw_render::lyrics`. Synced when the text has `[mm:ss.xx]`
//! timestamps; plain lines otherwise.

#[derive(Debug, Clone, PartialEq)]
pub struct LyricLine {
    /// Seconds into the track; `None` for unsynced lyrics.
    pub time: Option<f64>,
    pub text: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Lyrics {
    pub lines: Vec<LyricLine>,
    pub synced: bool,
    /// Where they came from, for display ("lrc file", "tags").
    pub source: &'static str,
}

impl Lyrics {
    /// The line being sung at `secs`: the last one whose time has passed.
    pub fn index_at(&self, secs: f64) -> Option<usize> {
        if !self.synced {
            return None;
        }
        let n = self
            .lines
            .partition_point(|l| l.time.is_some_and(|t| t <= secs));
        n.checked_sub(1)
    }
}

/// Parses LRC text. Lines may carry several timestamps; `[offset:+/-ms]`
/// shifts them (positive means earlier, per the LRC convention). Metadata
/// tags (`[ar:...]`) are skipped. Text without any timestamp gives
/// unsynced lyrics.
pub fn parse_lrc(text: &str, source: &'static str) -> Option<Lyrics> {
    let mut offset = 0.0;
    let mut timed = Vec::new();
    let mut plain = Vec::new();
    for raw in text.trim_start_matches('\u{feff}').lines() {
        let mut rest = raw.trim();
        let mut times = Vec::new();
        while let Some(after) = rest.strip_prefix('[') {
            let Some(end) = after.find(']') else { break };
            let tag = &after[..end];
            if let Some(t) = parse_time(tag) {
                times.push(t);
            } else if let Some(v) = tag.strip_prefix("offset:") {
                offset = v.trim().parse::<f64>().unwrap_or(0.0) / 1000.0;
            }
            rest = after[end + 1..].trim_start();
        }
        let line = rest.trim().to_string();
        if times.is_empty() {
            if !line.is_empty() && !raw.trim_start().starts_with('[') {
                plain.push(line);
            }
        } else {
            for t in times {
                timed.push((t, line.clone()));
            }
        }
    }
    if !timed.is_empty() {
        timed.sort_by(|a, b| a.0.total_cmp(&b.0));
        let lines = timed
            .into_iter()
            .map(|(t, text)| LyricLine {
                time: Some((t - offset).max(0.0)),
                text,
            })
            .collect();
        return Some(Lyrics {
            lines,
            synced: true,
            source,
        });
    }
    (!plain.is_empty()).then(|| Lyrics {
        lines: plain
            .into_iter()
            .map(|text| LyricLine { time: None, text })
            .collect(),
        synced: false,
        source,
    })
}

/// `mm:ss`, `mm:ss.xx` or `mm:ss:xx` to seconds.
fn parse_time(tag: &str) -> Option<f64> {
    let (m, s) = tag.split_once(':')?;
    let m: u32 = m.trim().parse().ok()?;
    let s = s.trim().replacen(':', ".", 1);
    let s: f64 = s.parse().ok()?;
    (s < 60.0).then_some(m as f64 * 60.0 + s)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_synced_lrc_with_repeats_and_offset() {
        let text = "[ar:Someone]\n[offset:500]\n[00:01.50]First\n[00:05.00][00:20.00]Chorus\n[00:10:25]Third\n\n";
        let l = parse_lrc(text, "test").unwrap();
        assert!(l.synced);
        let got: Vec<(f64, &str)> = l
            .lines
            .iter()
            .map(|x| (x.time.unwrap(), x.text.as_str()))
            .collect();
        assert_eq!(
            got,
            [
                (1.0, "First"),
                (4.5, "Chorus"),
                (9.75, "Third"),
                (19.5, "Chorus")
            ]
        );
        assert_eq!(l.index_at(0.5), None);
        assert_eq!(l.index_at(1.0), Some(0));
        assert_eq!(l.index_at(9.0), Some(1));
        assert_eq!(l.index_at(500.0), Some(3));
    }

    #[test]
    fn plain_text_is_unsynced_and_empty_is_none() {
        let l = parse_lrc("line one\nline two\n", "tags").unwrap();
        assert!(!l.synced);
        assert_eq!(l.lines.len(), 2);
        assert_eq!(l.index_at(3.0), None);
        assert_eq!(parse_lrc("[ar:x]\n\n", "tags"), None);
    }
}
