//! Hungarian algorithm (Kuhn-Munkres) for optimal assignment.
//!
//! Used by the multi-target tracker to find the optimal one-to-one assignment
//! of detections to tracks, minimizing the total cost (1 - IoU).
//!
//! ## Algorithm
//!
//! O(n³) implementation of the classic Hungarian algorithm. Given an n×m cost
//! matrix, finds the assignment that minimizes total cost.
//!
//! ## References
//!
//! - https://en.wikipedia.org/wiki/Hungarian_algorithm
//! - Based on the implementation by James M. Glattfelder (public domain).

/// Solve the assignment problem: given a cost matrix of size `n` rows × `m` cols,
/// find the assignment that minimizes total cost.
///
/// Returns a vector of length `n` where element `i` is the column index assigned
/// to row `i`, or `None` if no assignment was possible for that row.
///
/// If `n > m`, some rows will be unassigned. If `m > n`, some columns will be
/// unused (but all rows get assigned).
pub fn solve(cost_matrix: &[Vec<f32>]) -> Vec<Option<usize>> {
    let n = cost_matrix.len();
    if n == 0 {
        return Vec::new();
    }
    let m = cost_matrix[0].len();
    if m == 0 {
        return vec![None; n];
    }

    // We need a square matrix. Pad with large values to make it square.
    let size = n.max(m);
    let large_cost = 1e6_f32;

    let mut matrix: Vec<Vec<f64>> = vec![vec![large_cost as f64; size]; size];
    for i in 0..n {
        for j in 0..m {
            matrix[i][j] = cost_matrix[i][j] as f64;
        }
    }

    // Run the Hungarian algorithm on the square matrix.
    let assignment = hungarian(&mut matrix, size);

    // Convert back to Vec<Option<usize>>, only including assignments that
    // correspond to real (non-padding) cells.
    let mut result = Vec::with_capacity(n);
    for &assign in assignment.iter().take(n) {
        if assign < m {
            result.push(Some(assign));
        } else {
            result.push(None); // assigned to a padding column
        }
    }
    result
}

/// The classic O(n³) Hungarian algorithm.
/// `matrix` is a size×size cost matrix. Returns a vector of length `size`
/// where element `i` is the column assigned to row `i`.
fn hungarian(matrix: &mut [Vec<f64>], size: usize) -> Vec<usize> {
    // Implementation based on the standard algorithm.
    // We use 1-indexed arrays internally (matching the classic pseudocode).

    if size == 0 {
        return Vec::new();
    }

    // Convert to 1-indexed
    let mut cost: Vec<Vec<f64>> = vec![vec![0.0; size + 1]; size + 1];
    for i in 0..size {
        for j in 0..size {
            cost[i + 1][j + 1] = matrix[i][j];
        }
    }

    let mut u = vec![0.0_f64; size + 1];
    let mut v = vec![0.0_f64; size + 1];
    let mut p = vec![0_usize; size + 1]; // p[j] = row assigned to column j
    let mut way = vec![0_usize; size + 1];

    for i in 1..=size {
        p[0] = i;
        let mut j0 = 0_usize;
        let mut minv = vec![f64::INFINITY; size + 1];
        let mut used = vec![false; size + 1];

        loop {
            used[j0] = true;
            let i0 = p[j0];
            let mut delta = f64::INFINITY;
            let mut j1 = 0_usize;

            for j in 1..=size {
                if !used[j] {
                    let cur = cost[i0][j] - u[i0] - v[j];
                    if cur < minv[j] {
                        minv[j] = cur;
                        way[j] = j0;
                    }
                    if minv[j] < delta {
                        delta = minv[j];
                        j1 = j;
                    }
                }
            }

            for j in 0..=size {
                if used[j] {
                    u[p[j]] += delta;
                    v[j] -= delta;
                } else {
                    minv[j] -= delta;
                }
            }

            j0 = j1;
            if p[j0] == 0 {
                break;
            }
        }

        // Augmenting path
        loop {
            let j1 = way[j0];
            p[j0] = p[j1];
            j0 = j1;
            if j0 == 0 {
                break;
            }
        }
    }

    // Convert p (column → row) to result (row → column)
    let mut result = vec![0_usize; size];
    for j in 1..=size {
        if p[j] > 0 {
            result[p[j] - 1] = j - 1;
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_matrix() {
        let result = solve(&[]);
        assert!(result.is_empty());
    }

    #[test]
    fn single_element() {
        let matrix = vec![vec![1.0]];
        let result = solve(&matrix);
        assert_eq!(result, vec![Some(0)]);
    }

    #[test]
    fn two_by_two_optimal() {
        // Cost matrix:
        //   [1, 2]
        //   [3, 4]
        // Optimal: row 0 → col 0, row 1 → col 1 (total cost 1+4=5)
        let matrix = vec![vec![1.0, 2.0], vec![3.0, 4.0]];
        let result = solve(&matrix);
        assert_eq!(result, vec![Some(0), Some(1)]);
    }

    #[test]
    fn two_by_two_swap() {
        // Cost matrix:
        //   [4, 1]
        //   [2, 3]
        // Optimal: row 0 → col 1, row 1 → col 0 (total cost 1+2=3)
        let matrix = vec![vec![4.0, 1.0], vec![2.0, 3.0]];
        let result = solve(&matrix);
        assert_eq!(result, vec![Some(1), Some(0)]);
    }

    #[test]
    fn three_by_three_identity() {
        // Diagonal is cheapest
        let matrix = vec![
            vec![0.0, 1.0, 1.0],
            vec![1.0, 0.0, 1.0],
            vec![1.0, 1.0, 0.0],
        ];
        let result = solve(&matrix);
        assert_eq!(result, vec![Some(0), Some(1), Some(2)]);
    }

    #[test]
    fn rectangular_wide() {
        // 2 rows, 3 cols — each row should get assigned
        let matrix = vec![vec![1.0, 0.0, 1.0], vec![1.0, 1.0, 0.0]];
        let result = solve(&matrix);
        assert_eq!(result.len(), 2);
        // Row 0 should get col 1 (cost 0), row 1 should get col 2 (cost 0)
        assert_eq!(result[0], Some(1));
        assert_eq!(result[1], Some(2));
    }

    #[test]
    fn rectangular_tall() {
        // 3 rows, 2 cols — one row should be unassigned
        let matrix = vec![vec![0.0, 1.0], vec![1.0, 0.0], vec![1.0, 1.0]];
        let result = solve(&matrix);
        assert_eq!(result.len(), 3);
        // Rows 0 and 1 should get the two columns
        assert!(result[0].is_some());
        assert!(result[1].is_some());
        // Row 2 should be unassigned (or assigned to padding)
        // Note: with padding, it might get assigned to a padding column
    }

    #[test]
    fn iou_based_assignment() {
        // Simulate IoU-based cost matrix (cost = 1 - IoU)
        // 3 tracks, 3 detections
        let matrix = vec![
            vec![0.1, 0.9, 0.8], // track 0 → detection 0 (high IoU, low cost)
            vec![0.9, 0.2, 0.7], // track 1 → detection 1
            vec![0.8, 0.7, 0.3], // track 2 → detection 2
        ];
        let result = solve(&matrix);
        assert_eq!(result, vec![Some(0), Some(1), Some(2)]);
    }

    #[test]
    fn all_high_cost() {
        // No good matches — should still produce a valid assignment
        let matrix = vec![vec![0.9, 0.8], vec![0.8, 0.9]];
        let result = solve(&matrix);
        assert_eq!(result.len(), 2);
        // Both rows should be assigned
        assert!(result.iter().all(|r| r.is_some()));
    }

    #[test]
    fn handles_zero_cost() {
        let matrix = vec![vec![0.0, 0.0], vec![0.0, 0.0]];
        let result = solve(&matrix);
        // Any assignment is optimal — just verify it's valid
        assert_eq!(result.len(), 2);
        assert!(result.iter().all(|r| r.is_some()));
        // Both columns should be used exactly once
        let cols: Vec<_> = result.iter().map(|r| r.unwrap()).collect();
        assert_eq!(cols.len(), 2);
        assert_ne!(cols[0], cols[1]);
    }

    #[test]
    fn larger_matrix() {
        // 4×4 with a known optimal assignment
        let matrix = vec![
            vec![10.0, 5.0, 13.0, 15.0],
            vec![3.0, 9.0, 18.0, 13.0],
            vec![13.0, 7.0, 4.0, 15.0],
            vec![12.0, 11.0, 14.0, 8.0],
        ];
        let result = solve(&matrix);
        // Verify all rows are assigned distinct columns
        assert_eq!(result.len(), 4);
        let cols: Vec<_> = result.iter().map(|r| r.unwrap()).collect();
        let mut sorted = cols.clone();
        sorted.sort();
        assert_eq!(sorted, vec![0, 1, 2, 3]);
    }
}
