//! Port von `packages/client/src/promise.ts`.
//!
//! Abweichung Klasse 3 (Tech-Substitution): `Promise.withResolvers()` wird zu
//! einem `oneshot`-Kanal; mehrfach abwartbare Promises werden zu
//! `futures::future::Shared`.

use std::sync::{Arc, Mutex};

use futures::FutureExt;
use futures::future::{BoxFuture, Shared};
use tokio::sync::oneshot;

use crate::errors::PiError;

pub(crate) type SettleResult<T> = Result<T, PiError>;
pub(crate) type SharedPromise<T> = Shared<BoxFuture<'static, SettleResult<T>>>;

/// Auflöser eines Versprechens. Mehrfaches Auflösen ist wie in JS wirkungslos.
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
            // Der Sender wird nur beim Abräumen verworfen; das entspricht einem
            // getrennten Client.
            Err(_) => Err(PiError::disconnected()),
        }
    });
    (resolver, promise.shared())
}

/// Ein bereits aufgelöstes Versprechen.
pub(crate) fn resolved<T: Clone + Send + 'static>(value: T) -> SharedPromise<T> {
    let promise: BoxFuture<'static, SettleResult<T>> = Box::pin(async move { Ok(value) });
    promise.shared()
}

/// Baut ein Versprechen aus einem Future.
pub(crate) fn promise<T: Clone + Send + 'static>(
    future: impl std::future::Future<Output = SettleResult<T>> + Send + 'static,
) -> SharedPromise<T> {
    let boxed: BoxFuture<'static, SettleResult<T>> = Box::pin(future);
    boxed.shared()
}

/// Ein bereits abgelehntes Versprechen.
pub(crate) fn rejected<T: Clone + Send + 'static>(error: PiError) -> SharedPromise<T> {
    let boxed: BoxFuture<'static, SettleResult<T>> = Box::pin(async move { Err(error) });
    boxed.shared()
}
