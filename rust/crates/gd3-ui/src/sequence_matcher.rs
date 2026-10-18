//! `difflib.SequenceMatcher` 的最小移植。
//!
//! 只覆盖 `TaskDraft.setUrls` 用到的那一种配置：`isjunk=None` + `autojunk=False`。
//! 在这个前提下 `__chain_b` 退化成「元素 → 下标列表」的普通映射（不剔除 junk、也不
//! 剔除「流行元素」），`find_longest_match` 里两条 junk 扩展循环是死代码，所以本模块
//! 只保留「最长公共块 + 队列切分 + 相邻块合并」三步，逐行对应 CPython 3.13 的
//! `/usr/lib/python3.13/difflib.py`。
//!
//! 与 CPython 的差异只有一处：Python 的 `get_opcodes()` 会缓存结果（`self.opcodes`），
//! 本模块每次现算——调用方本来就是一次性 diff，没有复用的场景。

use std::collections::HashMap;
use std::hash::Hash;

/// `get_opcodes()` 的标签。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OpTag {
    /// `a[i1:i2] == b[j1:j2]`
    Equal,
    /// `a[i1:i2]` 换成 `b[j1:j2]`
    Replace,
    /// 删掉 `a[i1:i2]`（此时 `j1 == j2`）
    Delete,
    /// 在 `a[i1:i1]` 处插入 `b[j1:j2]`（此时 `i1 == i2`）
    Insert,
}

/// 一条 opcode：`(tag, i1, i2, j1, j2)`。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Opcode {
    pub tag: OpTag,
    pub i1: usize,
    pub i2: usize,
    pub j1: usize,
    pub j2: usize,
}

/// `difflib.Match`：`a[a..a+size] == b[b..b+size]`。
///
/// `Ord` 的顺序就是 Python 元组 `(a, b, size)` 的排序口径。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct Match {
    a: usize,
    b: usize,
    size: usize,
}

/// 把 `b` 里每个元素出现过的下标收集起来（`__chain_b` 在 `isjunk=None` 时的全部工作）。
fn index_b<T: Eq + Hash>(b: &[T]) -> HashMap<&T, Vec<usize>> {
    let mut b2j: HashMap<&T, Vec<usize>> = HashMap::new();
    for (i, elt) in b.iter().enumerate() {
        b2j.entry(elt).or_default().push(i);
    }
    b2j
}

/// `find_longest_match(alo, ahi, blo, bhi)`。
///
/// 在 `a[alo..ahi]` 与 `b[blo..bhi]` 里找最长的公共块；同样长时取 `a` 里最靠前的，
/// 再同样靠前时取 `b` 里最靠前的（靠内层循环「只在 `k > bestsize` 时更新」实现）。
fn find_longest_match<T: Eq + Hash>(
    a: &[T],
    b: &[T],
    b2j: &HashMap<&T, Vec<usize>>,
    alo: usize,
    ahi: usize,
    blo: usize,
    bhi: usize,
) -> Match {
    let mut besti = alo;
    let mut bestj = blo;
    let mut bestsize = 0usize;
    // j2len[j] = 「以 a[i-1] 与 b[j] 结尾」的最长公共块长度
    let mut j2len: HashMap<usize, usize> = HashMap::new();
    for (i, av) in a.iter().enumerate().take(ahi).skip(alo) {
        let mut newj2len: HashMap<usize, usize> = HashMap::new();
        if let Some(indices) = b2j.get(av) {
            for &j in indices {
                if j < blo {
                    continue;
                }
                if j >= bhi {
                    break;
                }
                let previous = j.checked_sub(1).and_then(|p| j2len.get(&p)).copied();
                let k = previous.unwrap_or(0) + 1;
                newj2len.insert(j, k);
                if k > bestsize {
                    besti = i + 1 - k;
                    bestj = j + 1 - k;
                    bestsize = k;
                }
            }
        }
        j2len = newj2len;
    }

    // 向两侧把「非 junk 的相邻相等元素」吸进来。`isjunk=None` 时
    // Python 的两条 junk 扩展循环是死代码，这里直接省略。
    while besti > alo && bestj > blo && a[besti - 1] == b[bestj - 1] {
        besti -= 1;
        bestj -= 1;
        bestsize += 1;
    }
    while besti + bestsize < ahi
        && bestj + bestsize < bhi
        && a[besti + bestsize] == b[bestj + bestsize]
    {
        bestsize += 1;
    }

    Match {
        a: besti,
        b: bestj,
        size: bestsize,
    }
}

/// `get_matching_blocks()`：切成互不重叠的公共块，按 `(a, b)` 升序，末尾带哨兵。
fn matching_blocks<T: Eq + Hash>(a: &[T], b: &[T]) -> Vec<Match> {
    let b2j = index_b(b);
    let mut queue = vec![(0usize, a.len(), 0usize, b.len())];
    let mut blocks: Vec<Match> = Vec::new();
    while let Some((alo, ahi, blo, bhi)) = queue.pop() {
        let found = find_longest_match(a, b, &b2j, alo, ahi, blo, bhi);
        // size == 0 说明这一段没有公共元素，不必再切
        if found.size == 0 {
            continue;
        }
        blocks.push(found);
        if alo < found.a && blo < found.b {
            queue.push((alo, found.a, blo, found.b));
        }
        if found.a + found.size < ahi && found.b + found.size < bhi {
            queue.push((found.a + found.size, ahi, found.b + found.size, bhi));
        }
    }
    blocks.sort();

    // 合并首尾相接的块（CPython 2.5 起的行为）
    let mut i1 = 0usize;
    let mut j1 = 0usize;
    let mut k1 = 0usize;
    let mut non_adjacent: Vec<Match> = Vec::new();
    for block in blocks {
        if i1 + k1 == block.a && j1 + k1 == block.b {
            k1 += block.size;
        } else {
            if k1 != 0 {
                non_adjacent.push(Match {
                    a: i1,
                    b: j1,
                    size: k1,
                });
            }
            i1 = block.a;
            j1 = block.b;
            k1 = block.size;
        }
    }
    if k1 != 0 {
        non_adjacent.push(Match {
            a: i1,
            b: j1,
            size: k1,
        });
    }
    // 哨兵：把「最后一个公共块之后的部分」也纳入 opcodes 的遍历
    non_adjacent.push(Match {
        a: a.len(),
        b: b.len(),
        size: 0,
    });
    non_adjacent
}

/// `get_opcodes()`：把 `a` 变成 `b` 的操作序列。
///
/// 首条 opcode 一定满足 `i1 == j1 == 0`，后续每条都满足 `i1 == 上一条的 i2`、
/// `j1 == 上一条的 j2`。
pub fn opcodes<T: Eq + Hash>(a: &[T], b: &[T]) -> Vec<Opcode> {
    let mut i = 0usize;
    let mut j = 0usize;
    let mut answer: Vec<Opcode> = Vec::new();
    for block in matching_blocks(a, b) {
        let tag = if i < block.a && j < block.b {
            Some(OpTag::Replace)
        } else if i < block.a {
            Some(OpTag::Delete)
        } else if j < block.b {
            Some(OpTag::Insert)
        } else {
            None
        };
        if let Some(tag) = tag {
            answer.push(Opcode {
                tag,
                i1: i,
                i2: block.a,
                j1: j,
                j2: block.b,
            });
        }
        i = block.a + block.size;
        j = block.b + block.size;
        if block.size != 0 {
            answer.push(Opcode {
                tag: OpTag::Equal,
                i1: block.a,
                i2: i,
                j1: block.b,
                j2: j,
            });
        }
    }
    answer
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 期望值全部由 CPython 3.13 现算：
    /// `SequenceMatcher(a=a, b=b, autojunk=False).get_opcodes()`。
    fn check(a: &[&str], b: &[&str], expected: &[(OpTag, usize, usize, usize, usize)]) {
        let actual: Vec<_> = opcodes(a, b)
            .into_iter()
            .map(|op| (op.tag, op.i1, op.i2, op.j1, op.j2))
            .collect();
        assert_eq!(actual, expected, "a={a:?} b={b:?}");
    }

    #[test]
    fn empty_sequences_produce_no_opcodes() {
        check(&[], &[], &[]);
        check(&[], &["a"], &[(OpTag::Insert, 0, 0, 0, 1)]);
        check(&["a"], &[], &[(OpTag::Delete, 0, 1, 0, 0)]);
    }

    #[test]
    fn identical_sequences_produce_one_equal_opcode() {
        check(
            &["a", "b", "c"],
            &["a", "b", "c"],
            &[(OpTag::Equal, 0, 3, 0, 3)],
        );
    }

    #[test]
    fn a_single_substitution_is_a_replace_between_two_equals() {
        check(
            &["a", "b", "c"],
            &["a", "x", "c"],
            &[
                (OpTag::Equal, 0, 1, 0, 1),
                (OpTag::Replace, 1, 2, 1, 2),
                (OpTag::Equal, 2, 3, 2, 3),
            ],
        );
    }

    #[test]
    fn reversed_sequences_anchor_on_the_longest_block() {
        // 三个元素全反序：最长的公共块是单个 "a"，且取 a 里最靠前的那一个
        check(
            &["a", "b", "c"],
            &["c", "b", "a"],
            &[
                (OpTag::Insert, 0, 0, 0, 2),
                (OpTag::Equal, 0, 1, 2, 3),
                (OpTag::Delete, 1, 3, 3, 3),
            ],
        );
    }

    #[test]
    fn a_contiguous_slice_in_the_middle_is_one_delete_either_side() {
        check(
            &["a", "b", "c", "d"],
            &["b", "c"],
            &[
                (OpTag::Delete, 0, 1, 0, 0),
                (OpTag::Equal, 1, 3, 0, 2),
                (OpTag::Delete, 3, 4, 2, 2),
            ],
        );
        check(
            &["a", "b"],
            &["a", "b", "c", "d"],
            &[(OpTag::Equal, 0, 2, 0, 2), (OpTag::Insert, 2, 2, 2, 4)],
        );
    }

    #[test]
    fn shifting_a_window_produces_delete_equal_insert() {
        // `setUrls` 最常见的形状：删掉头部一条、尾部加一条
        check(
            &["x", "a", "b"],
            &["a", "b", "y"],
            &[
                (OpTag::Delete, 0, 1, 0, 0),
                (OpTag::Equal, 1, 3, 0, 2),
                (OpTag::Insert, 3, 3, 2, 3),
            ],
        );
        check(
            &["u1", "u2", "u3"],
            &["u2", "u3", "u4"],
            &[
                (OpTag::Delete, 0, 1, 0, 0),
                (OpTag::Equal, 1, 3, 0, 2),
                (OpTag::Insert, 3, 3, 2, 3),
            ],
        );
    }

    #[test]
    fn repeated_elements_keep_the_leftmost_longest_block() {
        check(
            &["a", "a", "a"],
            &["a", "a"],
            &[(OpTag::Equal, 0, 2, 0, 2), (OpTag::Delete, 2, 3, 2, 2)],
        );
        check(
            &["a", "b", "a", "b"],
            &["b", "a", "b", "a"],
            &[
                (OpTag::Insert, 0, 0, 0, 1),
                (OpTag::Equal, 0, 3, 1, 4),
                (OpTag::Delete, 3, 4, 4, 4),
            ],
        );
    }

    #[test]
    fn alternating_keeps_and_inserts_are_all_equal_length() {
        check(
            &["1", "2", "3", "4", "5"],
            &["1", "3", "5", "7"],
            &[
                (OpTag::Equal, 0, 1, 0, 1),
                (OpTag::Delete, 1, 2, 1, 1),
                (OpTag::Equal, 2, 3, 1, 2),
                (OpTag::Delete, 3, 4, 2, 2),
                (OpTag::Equal, 4, 5, 2, 3),
                (OpTag::Insert, 5, 5, 3, 4),
            ],
        );
    }

    #[test]
    fn adjacent_blocks_are_merged_into_one_equal_opcode() {
        // 构造「切分出来但首尾相接」的公共块：靠队列切分能自然出现相邻块
        check(
            &["a", "b", "c", "d", "e"],
            &["a", "b", "x", "d", "e"],
            &[
                (OpTag::Equal, 0, 2, 0, 2),
                (OpTag::Replace, 2, 3, 2, 3),
                (OpTag::Equal, 3, 5, 3, 5),
            ],
        );
    }

    #[test]
    fn opcodes_are_contiguous_by_construction() {
        let a = ["a", "b", "c", "d", "e", "f"];
        let b = ["c", "d", "a", "f", "g"];
        let ops = opcodes(&a, &b);
        assert_eq!(ops.first().map(|op| (op.i1, op.j1)), Some((0, 0)));
        for pair in ops.windows(2) {
            assert_eq!(pair[0].i2, pair[1].i1);
            assert_eq!(pair[0].j2, pair[1].j1);
        }
        let last = ops.last().expect("非空");
        assert_eq!((last.i2, last.j2), (a.len(), b.len()));
    }

    #[test]
    fn works_on_owned_strings_too() {
        let a = vec!["u1".to_string(), "u2".to_string()];
        let b = vec!["u2".to_string(), "u3".to_string()];
        let ops = opcodes(&a, &b);
        assert_eq!(ops.len(), 3);
        assert_eq!(ops[1].tag, OpTag::Equal);
    }
}
