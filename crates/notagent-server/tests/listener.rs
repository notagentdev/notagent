mod support;

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use notagent_server::errors::ServerError;
use notagent_server::listener::{PiServerListener, SharedListener};
use notagent_server::testing::{TestServerOptions, create_test_server};

struct TestListener {
    address: Mutex<Option<String>>,
    accepted: Mutex<bool>,
    start_count: AtomicUsize,
    close_count: AtomicUsize,
    start_error: Option<String>,
}

impl TestListener {
    fn new(address: &str, start_error: Option<&str>) -> Arc<Self> {
        Arc::new(Self {
            address: Mutex::new(Some(address.to_owned())),
            accepted: Mutex::new(false),
            start_count: AtomicUsize::new(0),
            close_count: AtomicUsize::new(0),
            start_error: start_error.map(str::to_owned),
        })
    }
}

#[async_trait]
impl PiServerListener for TestListener {
    fn address(&self) -> Option<String> {
        self.address.lock().expect("listener mutex").clone()
    }

    async fn start(
        &self,
        _accept: notagent_server::ByteConnectionAcceptor,
    ) -> Result<(), ServerError> {
        self.start_count.fetch_add(1, Ordering::SeqCst);
        *self.accepted.lock().expect("listener mutex") = true;
        match &self.start_error {
            Some(error) => Err(ServerError::other(error.clone())),
            None => Ok(()),
        }
    }

    async fn close(&self) -> Result<(), ServerError> {
        self.close_count.fetch_add(1, Ordering::SeqCst);
        *self.address.lock().expect("listener mutex") = None;
        Ok(())
    }
}

#[tokio::test]
async fn starts_and_closes_every_configured_listener() {
    let first = TestListener::new("first", None);
    let second = TestListener::new("second", None);
    let listeners: Vec<SharedListener> = vec![
        Arc::clone(&first) as SharedListener,
        Arc::clone(&second) as SharedListener,
    ];
    let test_server = create_test_server(TestServerOptions::new(listeners)).expect("server");

    test_server.server.start().await.expect("starts");
    assert_eq!(
        test_server.server.addresses(),
        vec!["first".to_owned(), "second".to_owned()]
    );
    assert!(*first.accepted.lock().unwrap());
    assert!(*second.accepted.lock().unwrap());

    test_server.server.close().await.expect("closes");
    assert_eq!(first.close_count.load(Ordering::SeqCst), 1);
    assert_eq!(second.close_count.load(Ordering::SeqCst), 1);
    assert!(test_server.server.addresses().is_empty());
}

#[tokio::test]
async fn closes_previously_started_listeners_when_startup_fails() {
    let first = TestListener::new("first", None);
    let second = TestListener::new("second", Some("listener failed"));
    let listeners: Vec<SharedListener> = vec![
        Arc::clone(&first) as SharedListener,
        Arc::clone(&second) as SharedListener,
    ];
    let test_server = create_test_server(TestServerOptions::new(listeners)).expect("server");

    let error = test_server.server.start().await.expect_err("fails");
    assert_eq!(error.to_string(), "listener failed");
    assert_eq!(first.close_count.load(Ordering::SeqCst), 1);
    assert_eq!(second.close_count.load(Ordering::SeqCst), 0);
}
