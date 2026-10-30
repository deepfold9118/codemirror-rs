//! Line heights for vertical layout: estimated for lines not yet drawn,
//! measured for lines that were. A Fenwick tree gives the top of a line and
//! the line at a height in O(log n). A simpler stand-in for CodeMirror's
//! height map (which also handles block widgets and folding); edits rebuild
//! the tree, which is O(n).

pub(crate) struct LineHeights {
    heights: Vec<f32>,
    tree: Vec<f32>,
}

impl LineHeights {
    pub fn new(heights: Vec<f32>) -> LineHeights {
        let mut lh = LineHeights { heights, tree: Vec::new() };
        lh.rebuild();
        lh
    }

    fn rebuild(&mut self) {
        let n = self.heights.len();
        self.tree = vec![0.0; n + 1];
        for i in 0..n {
            let j = i + 1;
            self.tree[j] += self.heights[i];
            let k = j + (j & j.wrapping_neg());
            if k <= n {
                let v = self.tree[j];
                self.tree[k] += v;
            }
        }
    }

    pub fn len(&self) -> usize {
        self.heights.len()
    }

    pub fn height(&self, line: usize) -> f32 {
        self.heights[line]
    }

    /// The top of the line with the given (0-based) index.
    pub fn top(&self, line: usize) -> f32 {
        let mut i = line.min(self.heights.len());
        let mut sum = 0.0;
        while i > 0 {
            sum += self.tree[i];
            i &= i - 1;
        }
        sum
    }

    pub fn total(&self) -> f32 {
        self.top(self.heights.len())
    }

    /// The index of the line at the given height (clamped to the document).
    pub fn line_at(&self, y: f32) -> usize {
        let n = self.heights.len();
        if n == 0 || y <= 0.0 {
            return 0;
        }
        let mut pos = 0;
        let mut rem = y;
        let mut step = n.next_power_of_two();
        while step > 0 {
            let next = pos + step;
            if next <= n && self.tree[next] <= rem {
                pos = next;
                rem -= self.tree[next];
            }
            step >>= 1;
        }
        pos.min(n - 1)
    }

    /// Set a measured height. Returns true when it changed.
    pub fn set(&mut self, line: usize, height: f32) -> bool {
        let delta = height - self.heights[line];
        if delta.abs() < 0.01 {
            return false;
        }
        self.heights[line] = height;
        let n = self.heights.len();
        let mut i = line + 1;
        while i <= n {
            self.tree[i] += delta;
            i += i & i.wrapping_neg();
        }
        true
    }

    /// Replace lines `from..to` (old indices) with the given estimated
    /// heights.
    pub fn splice(&mut self, from: usize, to: usize, new: Vec<f32>) {
        self.heights.splice(from..to, new);
        self.rebuild();
    }

    /// Replace every height with a new estimate (after the wrap width or
    /// font changed).
    pub fn reestimate(&mut self, estimate: impl Fn(usize) -> f32) {
        for i in 0..self.heights.len() {
            self.heights[i] = estimate(i);
        }
        self.rebuild();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_tops_and_lines() {
        let mut h = LineHeights::new(vec![10.0; 100]);
        assert_eq!(h.top(0), 0.0);
        assert_eq!(h.top(5), 50.0);
        assert_eq!(h.total(), 1000.0);
        assert_eq!(h.line_at(0.0), 0);
        assert_eq!(h.line_at(9.9), 0);
        assert_eq!(h.line_at(10.0), 1);
        assert_eq!(h.line_at(5000.0), 99);
        h.set(3, 30.0);
        assert_eq!(h.top(4), 60.0);
        assert_eq!(h.line_at(45.0), 3);
        assert_eq!(h.line_at(60.0), 4);
        h.splice(1, 3, vec![5.0]);
        assert_eq!(h.len(), 99);
        assert_eq!(h.top(2), 15.0);
        assert_eq!(h.height(2), 30.0);
    }
}
