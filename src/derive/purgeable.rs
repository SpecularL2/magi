use crate::derive::async_iterator::AsyncIterator;
use async_trait::async_trait;

/// AsyncIterator that can purge itself
#[async_trait]
pub trait PurgeableAsyncIterator: AsyncIterator {
    async fn purge(&mut self);
}
