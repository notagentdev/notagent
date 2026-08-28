use notagent_protocol::{
    HelloErrorTag, ProtocolError, ProtocolErrorCode, ServerHelloError, ServerMessage,
    ServerMessageDecoder, encode_server_message,
};
use notagent_server::connection::ByteConnection;
use notagent_server::transports::unix::UnixByteConnection;
use tokio::io::AsyncReadExt;
use tokio::net::UnixStream;

#[tokio::test]
async fn queues_a_final_protocol_error_behind_pending_output_before_closing() {
    let (server_side, mut peer) = UnixStream::pair().expect("socket pair");
    let connection = UnixByteConnection::from_stream(server_side, 1_000, 64 * 1024);

    let pending_write = connection.send(vec![1, 2, 3]);
    let final_message = ServerMessage::HelloError(ServerHelloError {
        kind: HelloErrorTag,
        error: ProtocolError {
            code: ProtocolErrorCode::InvalidRequest,
            message: "Protocol violation".to_owned(),
            details: None,
        },
    });
    let final_frame = encode_server_message(&final_message, None).expect("encodes");
    pending_write.await.expect("writes");
    connection
        .close(Some(final_frame.clone()))
        .await
        .expect("closes");

    // The peer sees the pending output first and the final protocol error last.
    let mut received = Vec::new();
    peer.read_to_end(&mut received).await.expect("reads");
    assert_eq!(&received[..3], &[1, 2, 3]);
    let messages = ServerMessageDecoder::new(None)
        .expect("decoder")
        .push(&received[3..])
        .expect("decodes");
    assert_eq!(messages, vec![final_message]);
    assert!(connection.closed());
}
