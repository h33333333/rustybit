use std::ops::Deref;

pub struct CircularBuffer<T> {
    pos: usize,
    inner: Vec<T>,
}

impl<T> CircularBuffer<T> {
    pub fn new(size: usize) -> Self {
        CircularBuffer {
            pos: 0,
            inner: Vec::with_capacity(size),
        }
    }

    pub fn push_back(&mut self, item: T) {
        if self.pos == self.inner.capacity() - 1 {
            self.pos = 0
        }

        if self.inner.len() == self.inner.capacity() {
            self.inner[self.pos] = item;
        } else {
            self.inner.push(item);
        }

        self.pos += 1;
    }
}

impl<T> Deref for CircularBuffer<T> {
    type Target = [T];

    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}
