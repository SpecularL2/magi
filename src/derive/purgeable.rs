/// Iterator that can purge itself
use tokio_stream::Stream;

//pub trait PurgeableIterator: Iterator {
    //fn purge(&mut self);
//}

pub trait PurgeableStream: Stream {
    fn purge(&mut self);
}
