//! Command palette ranking: a small, deterministic subsequence fuzzy matcher.

// Per-character scoring weights. Word-start matches dominate so that a query
// like "ci" prefers "Compose inbox" (two word starts) over an incidental
// subsequence.
const MATCH_BASE: f64 = 1.0;
const WORD_START_BONUS: f64 = 10.0;
const CONSECUTIVE_BONUS: f64 = 5.0;
// Small penalty per character before the first match, rewarding earlier
// positions without ever overturning a word-start advantage.
const LEADING_PENALTY: f64 = 0.5;

/// A position is a word start at 0, after a non-alphanumeric, or on a
/// camelCase hump.
fn is_word_start(chars: &[char], i: usize) -> bool {
    if i == 0 {
        return true;
    }
    let (prev, cur) = (chars[i - 1], chars[i]);
    !prev.is_ascii_alphanumeric() || (prev.is_ascii_lowercase() && cur.is_ascii_uppercase())
}

/// Case-insensitive subsequence score of `query` against `text`; `None` when
/// `text` does not contain `query` as a subsequence. An empty query scores 0.
pub fn fuzzy_score(query: &str, text: &str) -> Option<f64> {
    let q: Vec<char> = query.to_lowercase().chars().collect();
    if q.is_empty() {
        return Some(0.0);
    }
    let chars: Vec<char> = text.chars().collect();
    let lower: Vec<char> = chars
        .iter()
        .map(|c| c.to_lowercase().next().unwrap_or(*c))
        .collect();

    let mut score = 0.0;
    let mut qi = 0;
    let mut prev_match: Option<usize> = None;
    let mut first_match: Option<usize> = None;
    for (ti, &c) in lower.iter().enumerate() {
        if qi == q.len() {
            break;
        }
        if c != q[qi] {
            continue;
        }
        first_match.get_or_insert(ti);
        let mut char_score = MATCH_BASE;
        if is_word_start(&chars, ti) {
            char_score += WORD_START_BONUS;
        }
        if prev_match.is_some_and(|p| p + 1 == ti) {
            char_score += CONSECUTIVE_BONUS;
        }
        score += char_score;
        prev_match = Some(ti);
        qi += 1;
    }
    if qi < q.len() {
        return None;
    }
    Some(score - first_match.unwrap_or(0) as f64 * LEADING_PENALTY)
}

/// Something the palette can rank: a title plus extra search keywords.
pub trait Rankable {
    fn title(&self) -> &str;
    fn keywords(&self) -> &[String];
}

/// Indices of `items` ranked against `query` (best of title and keywords).
/// Non-matches drop out; ties keep their original order. A blank query keeps
/// every item in order.
pub fn rank<T: Rankable>(query: &str, items: &[T]) -> Vec<usize> {
    let q = query.trim();
    if q.is_empty() {
        return (0..items.len()).collect();
    }
    let mut scored: Vec<(usize, f64)> = items
        .iter()
        .enumerate()
        .filter_map(|(i, item)| {
            let best = std::iter::once(fuzzy_score(q, item.title()))
                .chain(item.keywords().iter().map(|k| fuzzy_score(q, k)))
                .flatten()
                .fold(None, |best: Option<f64>, s| {
                    Some(best.map_or(s, |b| b.max(s)))
                });
            best.map(|s| (i, s))
        })
        .collect();
    // Stable sort: equal scores keep their original order.
    scored.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
    scored.into_iter().map(|(i, _)| i).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Cmd(&'static str, &'static str, Vec<String>);
    impl Rankable for Cmd {
        fn title(&self) -> &str {
            self.1
        }
        fn keywords(&self) -> &[String] {
            &self.2
        }
    }
    fn cmd(id: &'static str, title: &'static str) -> Cmd {
        Cmd(id, title, Vec::new())
    }
    fn ids(query: &str, cmds: &[Cmd]) -> Vec<&'static str> {
        rank(query, cmds).into_iter().map(|i| cmds[i].0).collect()
    }

    #[test]
    fn exact_prefix_beats_a_scattered_subsequence() {
        let exact = fuzzy_score("arch", "Archive message").unwrap();
        let scattered = fuzzy_score("arch", "Search all mail").unwrap();
        assert!(exact > scattered);
    }

    #[test]
    fn substring_and_case_insensitive_matches() {
        assert!(fuzzy_score("inbox", "Open inbox — harry@pluscosmic.dev").is_some());
        assert!(fuzzy_score("SYNC", "sync current folder").is_some());
        assert!(fuzzy_score("gtf", "Go to folder — Archive").is_some());
    }

    #[test]
    fn no_match_and_empty_query() {
        assert_eq!(fuzzy_score("zzz", "Compose"), None);
        assert_eq!(fuzzy_score("cx", "Compose"), None);
        assert_eq!(fuzzy_score("", "anything"), Some(0.0));
    }

    #[test]
    fn word_start_preference() {
        let cmds = [
            cmd("music", "Music library"),
            cmd("compose", "Compose inbox"),
        ];
        assert_eq!(ids("ci", &cmds)[0], "compose");
    }

    #[test]
    fn filters_non_matches_and_keeps_order_for_blank_queries() {
        let cmds = [
            cmd("a", "Compose"),
            cmd("b", "Add account"),
            cmd("c", "Sync current folder"),
        ];
        assert_eq!(ids("sync", &cmds), vec!["c"]);
        assert_eq!(ids("", &cmds), vec!["a", "b", "c"]);
        assert_eq!(ids("   ", &cmds), vec!["a", "b", "c"]);
    }

    #[test]
    fn keywords_match_when_the_title_does_not() {
        let cmds = [
            Cmd(
                "toggle",
                "Toggle read",
                vec!["unread".into(), "seen".into(), "mark".into()],
            ),
            cmd("compose", "Compose"),
        ];
        assert_eq!(ids("unread", &cmds), vec!["toggle"]);
    }

    #[test]
    fn stable_tie_break() {
        let cmds = [
            cmd("first", "Refresh"),
            cmd("second", "Refresh"),
            cmd("third", "Refresh"),
        ];
        assert_eq!(ids("refresh", &cmds), vec!["first", "second", "third"]);
    }
}
