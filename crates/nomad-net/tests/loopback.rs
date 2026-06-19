//! Test d'intégration : un serveur + un client sur 127.0.0.1, sans mDNS.

use std::net::SocketAddr;
use std::time::Duration;

use nomad_core::layout::Screen;
use nomad_core::{Message, NodeId, Os};
use nomad_net::{client, server, ServerEvent};

#[tokio::test]
async fn client_joins_and_messages_relay() {
    let server_id = NodeId::random();
    let (mut srv, port) = server::start(server_id, 0).await.unwrap();
    let addr: SocketAddr = ([127, 0, 0, 1], port).into();

    let client_id = NodeId::random();
    let hello = Message::Hello {
        node_id: client_id,
        name: "client-test".into(),
        os: Os::Linux,
        screen: Screen::new(1920, 1080),
    };
    let mut cli = client::connect(addr, hello).await.unwrap();

    // 1) Le serveur voit le client arriver.
    match timeout(srv.recv()).await {
        Some(ServerEvent::Joined { node, name, .. }) => {
            assert_eq!(node, client_id);
            assert_eq!(name, "client-test");
        }
        other => panic!("attendu Joined, reçu {other:?}"),
    }

    // 2) Serveur → client.
    srv.send_to(client_id, Message::Ping);
    assert_eq!(timeout(cli.recv()).await, Some(Message::Ping));

    // 3) Client → serveur.
    cli.send(Message::Pong);
    match timeout(srv.recv()).await {
        Some(ServerEvent::Message { from, msg }) => {
            assert_eq!(from, client_id);
            assert_eq!(msg, Message::Pong);
        }
        other => panic!("attendu Message, reçu {other:?}"),
    }
}

async fn timeout<F: std::future::Future>(fut: F) -> F::Output {
    tokio::time::timeout(Duration::from_secs(5), fut)
        .await
        .expect("timeout en attendant un événement réseau")
}
