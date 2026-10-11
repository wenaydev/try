//! Fuzzy subsequence matching with scoring and highlight positions.
//!
//! A query matches when its characters appear in order in the candidate.
//! Each matched character earns a point, plus bonuses for landing on a word
//! boundary or right after the previous match. The total is scaled by how
//! tightly the match is packed and by candidate length (shorter wins).

#[derive(Debug)]
struct Candidate {
    /// Lower-cased, one `char` per original `char` so indices line up.
    lower: Vec<char>,
    base_score: f64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Match {
    /// Index into the slice the matcher was built from.
    pub index: usize,
    pub score: f64,
    /// Char indices of matched characters (ascending). Empty for an empty query.
    pub positions: Vec<usize>,
}

#[derive(Debug)]
pub struct Fuzzy {
    cands: Vec<Candidate>,
}

fn lower(c: char) -> char {
    c.to_lowercase().next().unwrap_or(c)
}

impl Fuzzy {
    pub fn new<'a>(items: impl IntoIterator<Item = (&'a str, f64)>) -> Self {
        let cands = items
            .into_iter()
            .map(|(text, base_score)| Candidate {
                lower: text.chars().map(lower).collect(),
                base_score,
            })
            .collect();
        Fuzzy { cands }
    }

    /// All matches, best first. Ties keep input order.
    pub fn matches(&self, query: &str) -> Vec<Match> {
        let q: Vec<char> = query.chars().map(lower).collect();
        let mut out: Vec<Match> = self
            .cands
            .iter()
            .enumerate()
            .filter_map(|(index, c)| {
                score_one(c, &q).map(|(score, positions)| Match {
                    index,
                    score,
                    positions,
                })
            })
            .collect();
        out.sort_by(|a, b| b.score.total_cmp(&a.score));
        out
    }
}

fn is_boundary(c: char) -> bool {
    !(c.is_ascii_lowercase() || c.is_ascii_digit())
}

fn score_one(c: &Candidate, q: &[char]) -> Option<(f64, Vec<usize>)> {
    if q.is_empty() {
        return Some((c.base_score, Vec::new()));
    }

    let text = &c.lower;
    let mut score = c.base_score;
    let mut positions = Vec::with_capacity(q.len());
    let mut from = 0usize;
    let mut last: Option<usize> = None;

    for &qc in q {
        let found = from + text.get(from..)?.iter().position(|&t| t == qc)?;
        positions.push(found);

        score += 1.0;
        if found == 0 || is_boundary(text[found - 1]) {
            score += 1.0;
        }
        if let Some(l) = last {
            let gap = (found - l - 1) as f64;
            score += 2.0 / (gap + 1.0).sqrt();
        }
        last = Some(found);
        from = found + 1;
    }

    // Density: prefer matches packed into a short span from the start.
    score *= q.len() as f64 / (last.unwrap() + 1) as f64;
    // Length: shorter candidates win ties.
    score *= 10.0 / (text.len() as f64 + 10.0);
    Some((score, positions))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(items: &[(&str, f64)], q: &str) -> Vec<(String, Vec<usize>)> {
        let f = Fuzzy::new(items.iter().copied());
        f.matches(q)
            .into_iter()
            .map(|m| (items[m.index].0.to_string(), m.positions))
            .collect()
    }

    #[test]
    fn subsequence_with_positions() {
        let r = run(&[("redis-server", 0.0), ("postgres", 0.0)], "rds");
        assert_eq!(r, vec![("redis-server".to_string(), vec![0, 2, 4])]);
        assert!(run(&[("abc", 0.0)], "abd").is_empty());
        // query longer than text must not panic
        assert!(run(&[("a", 0.0)], "aaa").is_empty());
    }

    #[test]
    fn case_insensitive() {
        assert_eq!(run(&[("Redis", 0.0)], "REDIS").len(), 1);
    }

    #[test]
    fn empty_query_orders_by_base_score() {
        let r = run(&[("old", 1.0), ("new", 5.0)], "");
        assert_eq!(r[0].0, "new");
        assert!(r[0].1.is_empty());
    }

    #[test]
    fn tighter_and_shorter_wins() {
        let r = run(
            &[
                ("connection-pool-extra", 0.0),
                ("pool", 0.0),
                ("p-o-o-l", 0.0),
            ],
            "pool",
        );
        assert_eq!(r[0].0, "pool");
        assert_eq!(r.last().unwrap().0, "connection-pool-extra".to_string());
    }

    #[test]
    fn word_boundary_beats_mid_word() {
        // Same length, same gap: only the boundary bonus differs.
        let r = run(&[("axb", 0.0), ("a-b", 0.0)], "ab");
        assert_eq!(r[0].0, "a-b");
    }

    #[test]
    fn stable_for_equal_scores() {
        let r = run(&[("aa", 0.0), ("aa", 0.0)], "a");
        assert_eq!(r.len(), 2);
    }

    #[test]
    fn unicode_positions_are_char_indices() {
        let r = run(&[("é-redis", 0.0)], "redis");
        assert_eq!(r[0].1, vec![2, 3, 4, 5, 6]);
    }
}
