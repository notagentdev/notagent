use std::sync::{Arc, Mutex};

use futures::FutureExt;
use futures::future::{BoxFuture, Shared};
use tokio::sync::oneshot;

use crate::errors::PiError;

pub(crate) type SettleResult<T> = Result<T, PiError>;
pub(crate) type SharedPromise<T> = Shared<BoxFuture<'static, SettleResult<T>>>;

/// Resolver of a promise. Settling more than once is a no-op, as in JS.
pub(crate) struct Resolver<T> {
    sender: Mutex<Option<oneshot::Sender<SettleResult<T>>>>,
}

impl<T> Resolver<T> {
    pub(crate) fn resolve(&self, value: T) {
        self.settle(Ok(value));
    }

    pub(crate) fn reject(&self, error: PiError) {
        self.settle(Err(error));
    }

    fn settle(&self, result: SettleResult<T>) {
        if let Some(sender) = self.sender.lock().expect("resolver mutex").take() {
            let _ = sender.send(result);
        }
    }
}

pub(crate) fn create_promise_resolvers<T: Clone + Send + 'static>()
-> (Arc<Resolver<T>>, SharedPromise<T>) {
    let (sender, receiver) = oneshot::channel::<SettleResult<T>>();
    let resolver = Arc::new(Resolver {
        sender: Mutex::new(Some(sender)),
    });
    let promise: BoxFuture<'static, SettleResult<T>> = Box::pin(async move {
        match receiver.await {
            Ok(result) => result,
            // The sender is only dropped during teardown, which is equivalent
            // to a disconnected client.
            Err(_) => Err(PiError::disconnected()),
        }
    });
    (resolver, promise.shared())
}

/// An already resolved promise.
pub(crate) fn resolved<T: Clone + Send + 'static>(value: T) -> SharedPromise<T> {
    let promise: BoxFuture<'static, SettleResult<T>> = Box::pin(async move { Ok(value) });
    promise.shared()
}

/// Builds a promise from a future.
pub(crate) fn promise<T: Clone + Send + 'static>(
    future: impl std::future::Future<Output = SettleResult<T>> + Send + 'static,
) -> SharedPromise<T> {
    let boxed: BoxFuture<'static, SettleResult<T>> = Box::pin(future);
    boxed.shared()
}

/// An already rejected promise.
pub(crate) fn rejected<T: Clone + Send + 'static>(error: PiError) -> SharedPromise<T> {
    let boxed: BoxFuture<'static, SettleResult<T>> = Box::pin(async move { Err(error) });
    boxed.shared()
}
