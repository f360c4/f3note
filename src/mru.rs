//! Most-recently-used ordering for tab switching.
//!
//! Ctrl+Tab walks this list, not the order tabs appear in the bar. With a
//! handful of tabs the difference is invisible; with a lot of them, positional
//! order is useless and recency is the only ordering anyone can predict. It is
//! the same reason Alt+Tab works the way it does.

pub struct Mru<T: Copy + PartialEq> {
    order: Vec<T>,
}

impl<T: Copy + PartialEq> Default for Mru<T> {
    fn default() -> Self {
        Mru { order: Vec::new() }
    }
}

impl<T: Copy + PartialEq> Mru<T> {
    /// Move an item to the front, inserting it if it is new.
    pub fn touch(&mut self, item: T) {
        self.order.retain(|x| *x != item);
        self.order.insert(0, item);
    }

    pub fn remove(&mut self, item: T) {
        self.order.retain(|x| *x != item);
    }

    /// The item `steps` places back from the front, wrapping around.
    ///
    /// One step from the front is the previous document, which is what a single
    /// Ctrl+Tab should reach.
    pub fn nth(&self, steps: usize) -> Option<T> {
        if self.order.is_empty() {
            return None;
        }
        self.order.get(steps % self.order.len()).copied()
    }

    pub fn front(&self) -> Option<T> {
        self.order.first().copied()
    }

    pub fn as_slice(&self) -> &[T] {
        &self.order
    }

    pub fn len(&self) -> usize {
        self.order.len()
    }

    pub fn is_empty(&self) -> bool {
        self.order.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn most_recent_comes_first() {
        let mut m = Mru::default();
        m.touch(1);
        m.touch(2);
        m.touch(3);
        assert_eq!(m.as_slice(), &[3, 2, 1]);
    }

    #[test]
    fn touching_an_existing_item_moves_it_without_duplicating() {
        let mut m = Mru::default();
        m.touch(1);
        m.touch(2);
        m.touch(1);
        assert_eq!(m.as_slice(), &[1, 2]);
        assert_eq!(m.len(), 2);
    }

    #[test]
    fn one_step_back_is_the_previously_used_item() {
        let mut m = Mru::default();
        m.touch(10);
        m.touch(20);
        assert_eq!(m.front(), Some(20));
        assert_eq!(m.nth(1), Some(10));
    }

    #[test]
    fn stepping_past_the_end_wraps_around() {
        let mut m = Mru::default();
        m.touch(1);
        m.touch(2);
        assert_eq!(m.nth(2), Some(2));
        assert_eq!(m.nth(3), Some(1));
    }

    #[test]
    fn removing_keeps_the_rest_in_order() {
        let mut m = Mru::default();
        m.touch(1);
        m.touch(2);
        m.touch(3);
        m.remove(2);
        assert_eq!(m.as_slice(), &[3, 1]);
    }

    #[test]
    fn an_empty_list_has_nothing_to_switch_to() {
        let m: Mru<u64> = Mru::default();
        assert_eq!(m.front(), None);
        assert_eq!(m.nth(0), None);
        assert!(m.is_empty());
    }
}
