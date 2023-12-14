use async_trait::async_trait;
use crate::derive::async_iterator::AsyncIterator;

/// Iterator that can purge itself
pub trait PurgeableIterator: Iterator {
    fn purge(&mut self);
}

/// AsyncIterator that can purge itself
#[async_trait]
pub trait PurgeableAsyncIterator: AsyncIterator {
    async fn purge(&mut self);
}
