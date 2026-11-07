//! Snapshot comparison. Labels describe display inference, never writer actions.
use crate::{HistoryRecord, Variable};
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EntryChange {
    Unchanged,
    Added,
    Edited,
    Deleted,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    pub number: usize,
    pub text: String,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ComparisonRow {
    pub before: Option<Entry>,
    pub after: Option<Entry>,
    pub change: EntryChange,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Comparison {
    pub before_present: bool,
    pub after_present: bool,
    pub rows: Vec<ComparisonRow>,
    /// Unsupported raw data remains available through the record, never coerced.
    pub before_raw: Option<Vec<u8>>,
    pub after_raw: Option<Vec<u8>>,
}
fn entries(value: &Option<Variable>) -> Vec<String> {
    value
        .as_ref()
        .and_then(Variable::entries)
        .unwrap_or_default()
}
impl HistoryRecord {
    pub fn comparison(&self) -> Comparison {
        let before = entries(&self.before);
        let after = entries(&self.after);
        let mut matches = Vec::new();
        // Bound quadratic work/allocation. Prefix/suffix trimming also makes large
        // single insertions exact. The fallback uses bounded lookahead; any
        // unmatched block is a snapshot inference, not an operation sequence.
        let mut start = 0;
        while start < before.len().min(after.len()) && before[start] == after[start] {
            matches.push((start, start));
            start += 1;
        }
        let mut end_b = before.len();
        let mut end_a = after.len();
        while end_b > start && end_a > start && before[end_b - 1] == after[end_a - 1] {
            end_b -= 1;
            end_a -= 1;
        }
        let n = end_b - start;
        let m = end_a - start;
        if (n + 1)
            .checked_mul(m + 1)
            .is_some_and(|cells| cells <= 262_144)
        {
            let mut lengths = vec![0u32; (n + 1) * (m + 1)];
            for i in (0..n).rev() {
                for j in (0..m).rev() {
                    lengths[i * (m + 1) + j] = if before[start + i] == after[start + j] {
                        1 + lengths[(i + 1) * (m + 1) + j + 1]
                    } else {
                        lengths[(i + 1) * (m + 1) + j].max(lengths[i * (m + 1) + j + 1])
                    };
                }
            }
            let (mut i, mut j) = (0, 0);
            while i < n && j < m {
                if before[start + i] == after[start + j] {
                    matches.push((start + i, start + j));
                    i += 1;
                    j += 1;
                } else if lengths[(i + 1) * (m + 1) + j] >= lengths[i * (m + 1) + j + 1] {
                    i += 1;
                } else {
                    j += 1;
                }
            }
        } else {
            let (mut i, mut j) = (start, start);
            while i < end_b && j < end_a {
                if before[i] == after[j] {
                    matches.push((i, j));
                    i += 1;
                    j += 1;
                    continue;
                }
                let insert = (j + 1..end_a.min(j + 65)).find(|a| after[*a] == before[i]);
                let delete = (i + 1..end_b.min(i + 65)).find(|b| before[*b] == after[j]);
                match (insert, delete) {
                    (Some(a), Some(b)) if a - j <= b - i => j = a,
                    (Some(a), None) => j = a,
                    (_, Some(b)) => i = b,
                    _ => {
                        i += 1;
                        j += 1;
                    }
                }
            }
        }
        matches.extend((end_b..before.len()).zip(end_a..after.len()));
        let mut rows = Vec::new();
        let (mut b, mut a) = (0, 0);
        for (bi, ai) in matches.into_iter().chain(Some((before.len(), after.len()))) {
            while b < bi || a < ai {
                let left = (b < bi).then(|| Entry {
                    number: b + 1,
                    text: before[b].clone(),
                });
                let right = (a < ai).then(|| Entry {
                    number: a + 1,
                    text: after[a].clone(),
                });
                let change = match (left.is_some(), right.is_some()) {
                    (true, true) => EntryChange::Edited,
                    (true, false) => EntryChange::Deleted,
                    _ => EntryChange::Added,
                };
                if left.is_some() {
                    b += 1;
                }
                if right.is_some() {
                    a += 1;
                }
                rows.push(ComparisonRow {
                    before: left,
                    after: right,
                    change,
                });
            }
            if bi < before.len() && ai < after.len() {
                rows.push(ComparisonRow {
                    before: Some(Entry {
                        number: bi + 1,
                        text: before[bi].clone(),
                    }),
                    after: Some(Entry {
                        number: ai + 1,
                        text: after[ai].clone(),
                    }),
                    change: EntryChange::Unchanged,
                });
                b = bi + 1;
                a = ai + 1;
            }
        }
        Comparison {
            before_present: self.before.is_some(),
            after_present: self.after.is_some(),
            rows,
            before_raw: self
                .before
                .as_ref()
                .filter(|v| v.raw_text().is_none())
                .map(|v| v.bytes.clone()),
            after_raw: self
                .after
                .as_ref()
                .filter(|v| v.raw_text().is_none())
                .map(|v| v.bytes.clone()),
        }
    }
}
